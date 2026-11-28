//! Host-compiled chunks: compile once, instantiate in any state ([`CompiledChunk`]).
//!
//! A chunk is exported from the compiling state's prototypes into a form that holds no
//! heap reference: constants as owned values and bytes, nested functions as indices into
//! the chunk's own list. Instructions name constants and nested functions by index only,
//! so they carry over unchanged. Instantiating interns the constants and makes the
//! prototypes in the target state, charging memory exactly as the compiler does.
//!
//! There is no way to make a chunk from bytes: it only ever comes from source text that
//! the host compiled with [`Lua::compile`], so it needs no verifier, and `load` keeps
//! refusing binary chunks.

use std::sync::Arc;

use crate::bytecode::{Instr, LocVar, Proto, UpvalDesc};
use crate::heap::{self, Func};
use crate::{Dialect, Error, LimitKind, Lua, Root, Value};

/// A chunk compiled once by the host, to instantiate in any number of states with
/// [`Lua::load_compiled`]. Cloning is cheap (an `Arc`), and it is `Send` and `Sync`.
#[derive(Clone)]
pub struct CompiledChunk(Arc<Chunk>);

impl std::fmt::Debug for CompiledChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledChunk")
            .field("name", &self.0.name)
            .field("functions", &self.0.protos.len())
            .field("bytes", &self.0.bytes)
            .finish()
    }
}

impl CompiledChunk {
    /// The name it was compiled under.
    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// The memory one instantiation takes in a state (prototypes and constant strings),
    /// before the strings that state shares with others are counted once.
    pub fn instance_bytes(&self) -> usize {
        self.0.bytes
    }
}

struct Chunk {
    name: String,
    /// Children before parents; the main function last.
    protos: Vec<Flat>,
    bytes: usize,
}

enum Const {
    Nil,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(Box<[u8]>),
}

struct Flat {
    code: Vec<Instr>,
    lines: Vec<u32>,
    consts: Vec<Const>,
    /// Indices into `Chunk::protos`, each below this one's.
    protos: Vec<u32>,
    upvals: Vec<UpvalDesc>,
    locvars: Vec<LocVar>,
    num_params: u32,
    is_vararg: bool,
    max_stack: u16,
    frame: u32,
    source: Arc<str>,
    line_defined: u32,
    last_line_defined: u32,
    upval_writes: u64,
}

fn internal(what: &str) -> Error {
    Error::Host(format!("internal: compiled chunk {what}"))
}

impl Lua {
    /// Compiles `source` as [`Lua::load`] does, into a chunk that any state can
    /// instantiate with [`Lua::load_compiled`] without compiling again: for a prelude that
    /// every fresh sandbox runs. Errors are `load`'s. The compile runs in this state
    /// (its memory limit and interrupt apply; what it builds here is garbage afterwards).
    pub fn compile(&mut self, source: impl AsRef<[u8]>, name: &str) -> Result<CompiledChunk, Error> {
        let dialect = Dialect::for_name(name.trim_start_matches(['=', '@']));
        self.compile_with(source, name, dialect)
    }

    /// [`Lua::compile`] in a given dialect.
    pub fn compile_with(
        &mut self,
        source: impl AsRef<[u8]>,
        name: &str,
        dialect: Dialect,
    ) -> Result<CompiledChunk, Error> {
        let root = self.load_with(source, name, dialect)?;
        let Value::Func(fr) = root.value() else { return Err(internal("is not a function")) };
        let Some(Func::Lua { proto, .. }) = self.heap.funcs.get(fr.idx) else {
            return Err(internal("is not a Lua function"));
        };
        let main = *proto;
        // Post-order, without recursion: nesting is bounded only by memory.
        let mut out: Vec<Flat> = Vec::new();
        let mut placed: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let mut todo: Vec<(u32, bool)> = vec![(main, false)];
        let mut bytes = 0usize;
        while let Some((i, children_done)) = todo.pop() {
            let p = self.heap.protos.get(i).ok_or_else(|| internal("lost a prototype"))?;
            if !children_done {
                todo.push((i, true));
                todo.extend(p.protos.iter().rev().map(|&c| (c, false)));
                continue;
            }
            let mut consts = Vec::with_capacity(p.consts.len());
            for c in &p.consts {
                consts.push(match *c {
                    Value::Nil => Const::Nil,
                    Value::Bool(b) => Const::Bool(b),
                    Value::Int(n) => Const::Int(n),
                    Value::Num(n) => Const::Num(n),
                    Value::Str(s) => {
                        let b = self.heap.str_bytes(s);
                        bytes = bytes.saturating_add(heap::boxed(b.len()));
                        Const::Str(b.into())
                    }
                    _ => return Err(internal("has a constant that is not a scalar or string")),
                });
            }
            let mut kids = Vec::with_capacity(p.protos.len());
            for c in &p.protos {
                kids.push(*placed.get(c).ok_or_else(|| internal("has a child out of order"))?);
            }
            bytes = bytes.saturating_add(p.mem_size());
            let at = u32::try_from(out.len()).map_err(|_| Error::Limit(LimitKind::Memory))?;
            placed.insert(i, at);
            out.push(Flat {
                code: p.code.clone(),
                lines: p.lines.clone(),
                consts,
                protos: kids,
                upvals: p.upvals.clone(),
                locvars: p.locvars.clone(),
                num_params: p.num_params,
                is_vararg: p.is_vararg,
                max_stack: p.max_stack,
                frame: p.frame,
                source: Arc::clone(&p.source),
                line_defined: p.line_defined,
                last_line_defined: p.last_line_defined,
                upval_writes: p.upval_writes,
            });
        }
        drop(root);
        Ok(CompiledChunk(Arc::new(Chunk { name: name.to_owned(), protos: out, bytes })))
    }

    /// Instantiates `chunk` in this state: the chunk's main function, as [`Lua::load`] of
    /// its source would give it (its environment this state's globals). Charges the
    /// memory the prototypes and constants take, and, inside a call, the fuel of copying
    /// them. Out of memory, it collects once and tries again, like `load`.
    pub fn load_compiled(&mut self, chunk: &CompiledChunk) -> Result<Root, Error> {
        if self.poisoned {
            return Err(Error::Host("a panic in an earlier call left this state unusable".into()));
        }
        self.charge_copy(chunk.0.bytes)?;
        let f = match self.instantiate(&chunk.0) {
            Err(Error::Limit(LimitKind::Memory)) => {
                self.collect_garbage()?;
                self.instantiate(&chunk.0)?
            }
            r => r?,
        };
        let root = self.root(f);
        if let Ok(n) = self.new_str(chunk.0.name.as_bytes()) {
            self.set_chunk_name(f, n);
        }
        Ok(root)
    }

    fn instantiate(&mut self, chunk: &Chunk) -> Result<Value, Error> {
        let oom = |()| Error::Limit(LimitKind::Memory);
        // What is made here is only reachable once the closure is: nothing below collects
        // (charges and the heap's own allocations never do), as in the compiler.
        let mut made: Vec<u32> = Vec::with_capacity(chunk.protos.len());
        for f in &chunk.protos {
            let mut consts = Vec::with_capacity(f.consts.len());
            for c in &f.consts {
                consts.push(match c {
                    Const::Nil => Value::Nil,
                    Const::Bool(b) => Value::Bool(*b),
                    Const::Int(n) => Value::Int(*n),
                    Const::Num(n) => Value::Num(*n),
                    Const::Str(b) => Value::Str(self.heap.intern(b).map_err(oom)?),
                });
            }
            let mut protos = Vec::with_capacity(f.protos.len());
            for &k in &f.protos {
                protos.push(*made.get(k as usize).ok_or_else(|| internal("has a child out of order"))?);
            }
            let p = Proto {
                code: f.code.clone(),
                lines: f.lines.clone(),
                consts,
                protos,
                upvals: f.upvals.clone(),
                locvars: f.locvars.clone(),
                num_params: f.num_params,
                is_vararg: f.is_vararg,
                max_stack: f.max_stack,
                frame: f.frame,
                source: Arc::clone(&f.source),
                line_defined: f.line_defined,
                last_line_defined: f.last_line_defined,
                chunk_name: Value::Nil,
                hints: vec![std::cell::Cell::new(0); f.code.len()],
                upval_writes: f.upval_writes,
            };
            self.heap.mem.charge(p.mem_size()).map_err(oom)?;
            made.push(self.heap.new_proto_charged(p));
        }
        let proto = *made.last().ok_or_else(|| internal("is empty"))?;
        let env = Value::Table(self.globals);
        self.heap.blocked += 1;
        let r = (|| {
            self.heap.mem.charge(heap::UPVAL_SIZE + heap::FUNC_BASE + heap::boxed(8)).map_err(oom)?;
            let up = self.heap.new_upval_charged(heap::Upval::Closed(env));
            Ok(self.heap.new_func_charged(Func::Lua { proto, upvals: vec![up].into_boxed_slice() }))
        })();
        self.heap.blocked -= 1;
        let f = Value::Func(r?);
        self.hold(f);
        Ok(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Libs, Limits};

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn compiled_chunk_is_send_and_sync() {
        assert_send_sync::<CompiledChunk>();
    }

    #[test]
    fn deeply_nested_functions_export_without_recursion() {
        let n = 3_000;
        let src = format!("return {}1{}", "(function() return ".repeat(n), " end)()".repeat(n));
        let mut a = Lua::new(Limits::default(), Libs::BASE);
        let c = a.compile(&src, "=deep").unwrap();
        let mut b = Lua::new(Limits::default(), Libs::BASE);
        let f = b.load_compiled(&c).unwrap();
        let r = b.call(f.value(), &[]).unwrap();
        assert_eq!(r, vec![Value::Int(1)]);
    }
}
