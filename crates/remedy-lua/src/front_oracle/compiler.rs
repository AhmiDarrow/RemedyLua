//! The recursive compiler, kept as the oracle the iterative one is checked against
//! (feature `front-oracle`). Unchanged.
//!
//! The compiler: syntax tree to register-machine bytecode.
//!
//! One pass over the tree per function. Locals live in registers from the bottom of the
//! frame; temporaries are allocated above them stack-wise. Conditions compile to compare
//! and test instructions followed by jumps, patched once their targets are known. `goto`,
//! `break` and `continue` share one mechanism: pending jumps resolved against labels, with
//! the same scope rules as Lua 5.4.

use std::collections::HashMap;
use std::sync::Arc;

use crate::ast::*;
use crate::bytecode::{Instr, LocVar, MAX_REGS, MULTI, Proto, Reg, Rk, SETLIST_MULTI, UpvalDesc};
use crate::diag::{Diagnostic, Pos};
use crate::heap::Heap;
use crate::value::Value;

type CResult<T> = Result<T, Diagnostic>;

use crate::compiler::{
    BREAK, BlockScope, CONTINUE, LIST_FLUSH, LabelDesc, Want, arith_op, block_ends_in_jump,
    decided_by_constant, specialize,
};

#[derive(Clone)]
enum VarKind {
    Reg(Reg),
    /// A `<const>` local with a constant value: no register.
    Const(Value),
}

#[derive(Clone)]
struct ActiveVar {
    name: Arc<str>,
    kind: VarKind,
    attrib: Attrib,
    locvar: usize,
}

struct GotoDesc {
    name: String,
    pc: usize,
    line: u32,
    nactvar: usize,
    /// Register level to close when the jump is taken (leaves scopes with upvalues).
    close: Option<Reg>,
}

#[derive(Hash, PartialEq, Eq)]
enum ConstKey {
    Int(i64),
    Num(u64),
    Str(Vec<u8>),
    Bool(bool),
    Nil,
}

struct FuncState {
    code: Vec<Instr>,
    lines: Vec<u32>,
    /// Above the last pc a jump goes to (C Lua's `lasttarget`): only past it may the last
    /// instruction be changed in place.
    last_target: usize,
    consts: Vec<Value>,
    const_map: HashMap<ConstKey, u32>,
    protos: Vec<u32>,
    upvals: Vec<UpvalDesc>,
    locvars: Vec<LocVar>,
    actives: Vec<ActiveVar>,
    blocks: Vec<BlockScope>,
    labels: Vec<LabelDesc>,
    gotos: Vec<GotoDesc>,
    freereg: usize,
    max_stack: usize,
    num_params: Reg,
    is_vararg: bool,
    line_defined: u32,
    /// A to-be-closed variable is active (no tail calls then).
    tbc_active: usize,
}

pub(crate) struct Compiler<'h> {
    heap: &'h mut Heap,
    chunk: Arc<str>,
    fs: Vec<FuncState>,
    line: u32,
    /// The argument count field `call_setup` worked out.
    call_args_count: Reg,
    stack_base: usize,
    stack_bytes: usize,
}

/// Compiles a chunk: a variadic function with `_ENV` as its only upvalue.
pub(crate) fn compile(block: &Block, chunk: &str, heap: &mut Heap, stack_bytes: usize) -> CResult<u32> {
    heap.blocked += 1;
    let mut c = Compiler {
        heap,
        chunk: Arc::from(chunk),
        fs: Vec::new(),
        line: 1,
        call_args_count: 0,
        stack_base: crate::stack_pos(),
        stack_bytes,
    };
    let r = c.main(block);
    c.heap.blocked -= 1;
    r
}

impl FuncState {
    fn new(line_defined: u32) -> Self {
        FuncState {
            code: Vec::new(),
            lines: Vec::new(),
            consts: Vec::new(),
            const_map: HashMap::new(),
            protos: Vec::new(),
            upvals: Vec::new(),
            locvars: Vec::new(),
            actives: Vec::new(),
            blocks: Vec::new(),
            labels: Vec::new(),
            gotos: Vec::new(),
            freereg: 0,
            max_stack: 2,
            num_params: 0,
            is_vararg: false,
            line_defined,
            tbc_active: 0,
            last_target: 0,
        }
    }

    /// Registers the active locals occupy.
    fn nvarstack(&self) -> usize {
        self.reglevel(self.actives.len())
    }

    /// Registers the first `n` active locals occupy.
    fn reglevel(&self, n: usize) -> usize {
        for v in self.actives.iter().take(n).rev() {
            if let VarKind::Reg(r) = v.kind {
                return usize::from(r) + 1;
            }
        }
        0
    }
}

impl<'h> Compiler<'h> {
    fn fs(&mut self) -> &mut FuncState {
        // There is always a function being compiled while compiling.
        let n = self.fs.len();
        #[allow(clippy::indexing_slicing, reason = "fs is never empty while compiling")]
        &mut self.fs[n - 1]
    }

    fn fsr(&self) -> &FuncState {
        let n = self.fs.len();
        #[allow(clippy::indexing_slicing, reason = "fs is never empty while compiling")]
        &self.fs[n - 1]
    }

    fn err(&self, pos: Pos, msg: impl Into<String>) -> Diagnostic {
        Diagnostic::error(&self.chunk, pos, msg)
    }

    fn at(&mut self, pos: Pos) {
        if pos.line > 0 {
            self.line = pos.line;
        }
    }

    fn move_to(&mut self, dst: Reg, src: Reg) {
        if src != dst {
            self.emit(Instr::Move { a: dst, b: src });
        }
    }

    fn emit(&mut self, i: Instr) -> usize {
        let i = specialize(i, &self.fsr().consts);
        let line = self.line;
        let fs = self.fs();
        fs.code.push(i);
        fs.lines.push(line);
        fs.code.len() - 1
    }

    fn pc(&self) -> usize {
        self.fsr().code.len()
    }

    fn set_instr(&mut self, pc: usize, i: Instr) {
        if let Some(slot) = self.fs().code.get_mut(pc) {
            *slot = i;
        }
    }

    fn instr(&self, pc: usize) -> Option<Instr> {
        self.fsr().code.get(pc).copied()
    }

    // ---- registers ----

    fn alloc_reg(&mut self, pos: Pos) -> CResult<Reg> {
        self.reserve(1, pos)?;
        Ok((self.fsr().freereg - 1) as Reg)
    }

    fn reserve(&mut self, n: usize, pos: Pos) -> CResult<()> {
        let fs = self.fs();
        fs.freereg += n;
        if fs.freereg > MAX_REGS {
            return Err(self.err(pos, "function or expression needs too many registers"));
        }
        let fs = self.fs();
        if fs.freereg > fs.max_stack {
            fs.max_stack = fs.freereg;
        }
        Ok(())
    }

    fn freereg(&self) -> Reg {
        self.fsr().freereg as Reg
    }

    fn set_freereg(&mut self, r: usize) {
        self.fs().freereg = r;
    }

    // ---- constants ----

    fn konst(&mut self, key: ConstKey, v: Value, pos: Pos) -> CResult<u32> {
        if let Some(&i) = self.fsr().const_map.get(&key) {
            return Ok(i);
        }
        let fs = self.fs();
        let i = fs.consts.len() as u32;
        if i >= (1 << 24) {
            return Err(self.err(pos, "function has too many constants"));
        }
        let fs = self.fs();
        fs.consts.push(v);
        fs.const_map.insert(key, i);
        Ok(i)
    }

    fn str_const(&mut self, s: &[u8], pos: Pos) -> CResult<u32> {
        if let Some(&i) = self.fsr().const_map.get(&ConstKey::Str(s.to_vec())) {
            return Ok(i);
        }
        let r = self.heap.intern(s).map_err(|()| self.err(pos, "not enough memory"))?;
        self.konst(ConstKey::Str(s.to_vec()), Value::Str(r), pos)
    }

    fn num_const(&mut self, v: Value, pos: Pos) -> CResult<u32> {
        match v {
            Value::Int(i) => self.konst(ConstKey::Int(i), v, pos),
            Value::Num(f) => self.konst(ConstKey::Num(f.to_bits()), v, pos),
            Value::Bool(b) => self.konst(ConstKey::Bool(b), v, pos),
            Value::Nil => self.konst(ConstKey::Nil, v, pos),
            _ => self.konst(ConstKey::Nil, Value::Nil, pos),
        }
    }

    /// A constant index usable in a 16-bit field, or None.
    fn k16(k: u32) -> Option<u16> {
        u16::try_from(k).ok().filter(|&k| k < Rk::CONST)
    }

    // ---- functions ----

    fn main(&mut self, block: &Block) -> CResult<u32> {
        let mut fs = FuncState::new(0);
        fs.is_vararg = true;
        fs.upvals.push(UpvalDesc { in_stack: true, idx: 0, name: Arc::from("_ENV") });
        self.fs.push(fs);
        self.enter_block(false);
        self.block_body(block, false)?;
        self.line = block.end.line.max(self.line);
        self.leave_block(block.end)?;
        let line = block.end.line;
        self.line = line;
        self.emit(Instr::Return { a: 0, b: 0 });
        let fs = self.fs.pop().ok_or_else(|| self.err(block.end, "internal: no function"))?;
        if let Some(g) = fs.gotos.first() {
            return Err(self.undefined_goto(g, block.end.line));
        }
        self.finish(fs, block.end, 0)
    }

    /// A goto left pending when its function ends (at line `at`, where C Lua reports it).
    fn undefined_goto(&self, g: &GotoDesc, at: u32) -> Diagnostic {
        let pos = Pos { line: at, col: 1 };
        if g.name == BREAK {
            self.err(pos, format!("break outside loop at line {}", g.line))
        } else if g.name == CONTINUE {
            self.err(pos, format!("continue outside loop at line {}", g.line))
        } else {
            self.err(pos, format!("no visible label '{}' for <goto> at line {}", g.name, g.line))
        }
    }

    /// Makes the prototype, in the heap (charged to the memory account).
    fn finish(&mut self, fs: FuncState, pos: Pos, last_line: u32) -> CResult<u32> {
        let ncode = fs.code.len();
        let mut p = Proto {
            code: fs.code,
            lines: fs.lines,
            consts: fs.consts,
            protos: fs.protos,
            upvals: fs.upvals,
            locvars: fs.locvars,
            num_params: u32::from(fs.num_params),
            is_vararg: fs.is_vararg,
            max_stack: fs.max_stack as u16,
            frame: 0,
            source: Arc::clone(&self.chunk),
            line_defined: fs.line_defined,
            last_line_defined: last_line,
            chunk_name: Value::Nil,
            hints: vec![std::cell::Cell::new(0); ncode],
            upval_writes: 0,
        };
        crate::bytecode::settle_upvals(&mut p, &mut self.heap.protos);
        self.heap.mem.charge(p.mem_size()).map_err(|()| self.err(pos, "not enough memory"))?;
        Ok(self.heap.new_proto_charged(p))
    }

    fn function(&mut self, f: &FuncBody) -> CResult<u32> {
        let mut fs = FuncState::new(f.pos.line);
        fs.is_vararg = f.vararg;
        self.fs.push(fs);
        self.enter_block(false);
        let mut params: Vec<(String, Pos)> = Vec::new();
        if f.is_method {
            params.push(("self".into(), f.pos));
        }
        params.extend(f.params.iter().map(|p| (p.name.clone(), p.pos)));
        if params.len() > MAX_REGS {
            return Err(self.err(f.pos, "too many parameters"));
        }
        for (name, pos) in &params {
            let r = self.alloc_reg(*pos)?;
            self.add_local(name, VarKind::Reg(r), Attrib::None);
        }
        self.fs().num_params = params.len() as Reg;
        self.block_body(&f.body, false)?;
        // The function's closing happens at its `end` (as C Lua's return there).
        self.line = f.end_line;
        self.leave_block(f.body.end)?;
        self.emit(Instr::Return { a: 0, b: 0 });
        let fs = self.fs.pop().ok_or_else(|| self.err(f.pos, "internal: no function"))?;
        if let Some(g) = fs.gotos.first() {
            return Err(self.undefined_goto(g, f.close_line));
        }
        let p = self.finish(fs, f.pos, f.end_line)?;
        let parent = self.fs();
        parent.protos.push(p);
        Ok((parent.protos.len() - 1) as u32)
    }

    // ---- scopes ----

    fn add_local(&mut self, name: &str, kind: VarKind, attrib: Attrib) {
        let pc = self.pc() as u32;
        let fs = self.fs();
        let name: Arc<str> = Arc::from(name);
        let locvar = fs.locvars.len();
        if let VarKind::Reg(r) = kind {
            fs.locvars.push(LocVar { name: Arc::clone(&name), reg: r, start: pc, end: u32::MAX });
        }
        fs.actives.push(ActiveVar { name, kind, attrib, locvar });
    }

    fn remove_locals_to(&mut self, n: usize) {
        let pc = self.pc() as u32;
        let fs = self.fs();
        while fs.actives.len() > n {
            if let Some(v) = fs.actives.pop() {
                if matches!(v.kind, VarKind::Reg(_))
                    && let Some(lv) = fs.locvars.get_mut(v.locvar)
                {
                    lv.end = pc;
                }
                if v.attrib == Attrib::Close {
                    fs.tbc_active = fs.tbc_active.saturating_sub(1);
                }
            }
        }
    }

    fn enter_block(&mut self, is_loop: bool) {
        let fs = self.fs();
        fs.blocks.push(BlockScope {
            nactvar: fs.actives.len(),
            first_label: fs.labels.len(),
            first_goto: fs.gotos.len(),
            is_loop,
            has_upval: false,
        });
    }

    fn leave_block(&mut self, end: Pos) -> CResult<()> {
        let Some(bl) = self.fs().blocks.pop() else { return Ok(()) };
        let level = self.fsr().reglevel(bl.nactvar);
        let has_locals = self.fsr().actives.len() > bl.nactvar;
        // A loop's break label sits right after it, and resolves only its own breaks.
        if bl.is_loop {
            self.create_label(BREAK, end.line, Some(bl.nactvar), bl.first_goto, None)?;
        }
        if bl.has_upval && has_locals {
            self.emit(Instr::Close { a: level as Reg });
        }
        self.remove_locals_to(bl.nactvar);
        self.set_freereg(level);
        let fs = self.fs();
        fs.labels.truncate(bl.first_label);
        // Pending gotos leave the block: they no longer see its locals, and must close its
        // upvalues when taken.
        for g in fs.gotos.iter_mut().skip(bl.first_goto) {
            if g.nactvar > bl.nactvar {
                if bl.has_upval {
                    g.close = Some(g.close.map_or(level as Reg, |c| c.min(level as Reg)));
                }
                g.nactvar = bl.nactvar;
            }
        }
        Ok(())
    }

    /// Defines a label at the current position and resolves pending gotos to it (those
    /// from `first_goto` on). `at_end`: the label ends its block, so it is outside the
    /// scope of the block's locals (number of locals then).
    /// A label statement. C Lua checks it (and resolves the gotos waiting for it) after
    /// reading the labels and `;` that follow it, so its errors give the line of the next
    /// token: `report`.
    fn label_stat(&mut self, name: &str, pos: Pos, at_end: bool, report: u32) -> CResult<()> {
        if let Some(l) = self.fsr().labels.iter().find(|l| l.name == name) {
            let line = l.line;
            return Err(self
                .err(pos, format!("label '{name}' already defined on line {line}"))
                .lua_line(report));
        }
        let bl_nact = self.fsr().blocks.last().map_or(0, |b| b.nactvar);
        let first_goto = self.fsr().blocks.last().map_or(0, |b| b.first_goto);
        self.create_label(name, pos.line, at_end.then_some(bl_nact), first_goto, Some(report))
    }

    /// The labels from `i` on in `b` (a run of them; `i` is the first): checked last to
    /// first, as C Lua's `labelstat` reads the following ones before its own check. The
    /// index after the run.
    fn label_run(&mut self, b: &Block, i: usize, repeat: bool) -> CResult<usize> {
        let mut j = i;
        while b.stats.get(j).is_some_and(|s| matches!(s.kind, StatKind::Label(_))) {
            j += 1;
        }
        let report = b.stats.get(j).map_or(b.end.line, |s| s.pos.line);
        let at_end = !repeat && j == b.stats.len();
        for k in (i..j).rev() {
            if let Some(Stat { kind: StatKind::Label(name), pos }) = b.stats.get(k) {
                self.label_stat(name, *pos, at_end, report)?;
            }
        }
        Ok(j)
    }

    fn create_label(
        &mut self,
        name: &str,
        line: u32,
        at_end: Option<usize>,
        first_goto: usize,
        report: Option<u32>,
    ) -> CResult<()> {
        let pc = self.pc();
        let nactvar = at_end.unwrap_or(self.fsr().actives.len());
        let mut i = first_goto;
        while i < self.fsr().gotos.len() {
            let matches = self.fsr().gotos.get(i).is_some_and(|g| g.name == name);
            if !matches {
                i += 1;
                continue;
            }
            let g = self.fs().gotos.remove(i);
            if g.nactvar < nactvar {
                let var = self.fsr().actives.get(g.nactvar).map(|v| v.name.to_string()).unwrap_or_default();
                let what = if name == CONTINUE { "continue".to_owned() } else { format!("goto {name}") };
                return Err(self
                    .err(
                        Pos { line: g.line, col: 1 },
                        format!("<{what}> at line {} jumps into the scope of local '{var}'", g.line),
                    )
                    .lua_line(report.unwrap_or(g.line)));
            }
            self.patch_jump(g.pc, pc, g.close);
        }
        self.fs().labels.push(LabelDesc { name: name.to_owned(), pc, nactvar, line });
        Ok(())
    }

    fn patch_jump(&mut self, jmp: usize, target: usize, close: Option<Reg>) {
        let fs = self.fs();
        fs.last_target = fs.last_target.max(target);
        let off = target as i64 - jmp as i64 - 1;
        if let Some(Instr::Jmp { close: old, .. }) = self.instr(jmp) {
            let close = match (close, old) {
                (Some(c), 0) => u16::from(c) + 1,
                (Some(c), o) => (u16::from(c) + 1).min(o),
                (None, o) => o,
            };
            self.set_instr(jmp, Instr::Jmp { off: off as i32, close });
        }
    }

    /// A concatenation of `n` values from `a` whose last value was itself just concatenated
    /// into its register: that instruction takes them all (C Lua's `codeconcat`).
    fn concat(&mut self, a: Reg, n: usize) {
        let fs = self.fs();
        let last = usize::from(a) + n - 1;
        if fs.code.len() > fs.last_target
            && let Some(Instr::Concat { a: la, n: ln }) = fs.code.last_mut()
            && usize::from(*la) == last
        {
            (*la, *ln) = (a, (n - 1 + usize::from(*ln)) as u16);
            return;
        }
        self.emit(Instr::Concat { a, n: n as u16 });
    }

    fn jump(&mut self) -> usize {
        self.emit(Instr::Jmp { off: 0, close: 0 })
    }

    fn patch_here(&mut self, jumps: &[usize]) {
        let here = self.pc();
        for &j in jumps {
            self.patch_jump(j, here, None);
        }
    }

    fn jump_back(&mut self, target: usize) {
        let pc = self.pc() as i64;
        self.emit(Instr::Jmp { off: (target as i64 - pc - 1) as i32, close: 0 });
    }

    fn goto(&mut self, name: &str, pos: Pos) -> CResult<()> {
        let line = pos.line;
        let nactvar = self.fsr().actives.len();
        // A visible label: a backward jump, closing what it leaves.
        let found = self.fsr().labels.iter().rev().find(|l| l.name == name).map(|l| (l.pc, l.nactvar));
        if let Some((target, lnact)) = found {
            let level = self.fsr().reglevel(lnact);
            let close = if self.fsr().nvarstack() > level { u16::from(level as Reg) + 1 } else { 0 };
            let pc = self.pc() as i64;
            self.emit(Instr::Jmp { off: (target as i64 - pc - 1) as i32, close });
            return Ok(());
        }
        let pc = self.jump();
        self.fs().gotos.push(GotoDesc { name: name.to_owned(), pc, line, nactvar, close: None });
        Ok(())
    }

    // ---- variables ----

    /// Finds a variable by name: a local of this function, an upvalue, or None (global).
    fn resolve(&mut self, name: &str) -> Option<VarRef> {
        let depth = self.fs.len();
        self.resolve_in(depth - 1, name)
    }

    fn resolve_in(&mut self, level: usize, name: &str) -> Option<VarRef> {
        let fs = self.fs.get(level)?;
        if let Some(i) = fs.actives.iter().rposition(|v| &*v.name == name) {
            let v = fs.actives.get(i)?.clone();
            return Some(match v.kind {
                VarKind::Reg(r) => VarRef::Local(r, i, v.attrib),
                VarKind::Const(c) => VarRef::Const(c),
            });
        }
        if let Some(u) = fs.upvals.iter().position(|u| &*u.name == name) {
            return Some(VarRef::Upval(u as Reg));
        }
        if level == 0 {
            return None;
        }
        // Not here: look in the enclosing function and capture.
        let outer = self.resolve_in(level - 1, name)?;
        let (in_stack, idx) = match outer {
            VarRef::Const(c) => return Some(VarRef::Const(c)),
            VarRef::Local(r, i, _) => {
                let ofs = self.fs.get_mut(level - 1)?;
                // Mark the block that declared it as having upvalues.
                let decl_block = ofs.blocks.iter_mut().rev().find(|b| b.nactvar <= i);
                if let Some(b) = decl_block {
                    b.has_upval = true;
                }
                (true, r)
            }
            VarRef::Upval(u) => (false, u),
        };
        let fs = self.fs.get_mut(level)?;
        if fs.upvals.len() >= usize::from(u16::MAX) {
            return None;
        }
        fs.upvals.push(UpvalDesc { in_stack, idx: u32::from(idx), name: Arc::from(name) });
        Some(VarRef::Upval((fs.upvals.len() - 1) as Reg))
    }

    /// Where `_ENV` is.
    fn env(&mut self, pos: Pos) -> CResult<VarRef> {
        match self.resolve("_ENV") {
            Some(v) => Ok(v),
            None => Err(self.err(pos, "too many upvalues")),
        }
    }

    // ---- statements ----

    fn block_body(&mut self, b: &Block, repeat_body: bool) -> CResult<()> {
        let n = b.stats.len();
        let mut i = 0;
        while let Some(s) = b.stats.get(i) {
            if matches!(s.kind, StatKind::Label(_)) {
                i = self.label_run(b, i, repeat_body)?;
                continue;
            }
            // A label is "at the end" when only labels follow it (and the block is not a
            // repeat body, whose condition still sees its locals).
            let at_end =
                !repeat_body && b.stats.iter().skip(i + 1).all(|s| matches!(s.kind, StatKind::Label(_)));
            self.statement(s, at_end && i < n)?;
            // Temporaries never outlive a statement.
            let lvl = self.fsr().nvarstack();
            self.set_freereg(lvl);
            i += 1;
        }
        Ok(())
    }

    fn statement(&mut self, s: &Stat, label_at_end: bool) -> CResult<()> {
        self.deep(s.pos)?;
        self.at(s.pos);
        match &s.kind {
            StatKind::Call(e) => {
                self.call(e, Want::Fixed(0))?;
            }
            StatKind::Local(names, exprs) => self.local_stat(names, exprs, s.pos)?,
            StatKind::Assign(targets, exprs, follow) => self.assign(targets, exprs, follow, s.pos)?,
            StatKind::OpAssign(b) => self.op_assign(&b.0, b.1, &b.2, s.pos)?,
            StatKind::Do(b) => {
                self.enter_block(false);
                self.block_body(b, false)?;
                self.leave_block(b.end)?;
            }
            StatKind::While(cond, body) => {
                self.enter_block(true);
                let start = self.pc();
                let exits = self.cond_jump(cond, false)?;
                self.enter_block(false);
                self.block_body(body, false)?;
                self.leave_block(body.end)?;
                let first_goto = self.fsr().blocks.last().map_or(0, |b| b.first_goto);
                self.create_label(CONTINUE, body.end.line, None, first_goto, None)?;
                self.at(body.end);
                self.jump_back(start);
                self.patch_here(&exits);
                self.leave_block(body.end)?;
            }
            StatKind::Repeat(body, cond) => {
                self.enter_block(true);
                let start = self.pc();
                self.enter_block(false);
                self.block_body(body, true)?;
                let first_goto = self.fsr().blocks.iter().rev().nth(1).map_or(0, |b| b.first_goto);
                self.create_label(CONTINUE, body.end.line, None, first_goto, None)?;
                self.at(cond.pos);
                let back = self.cond_jump(cond, false)?;
                let has_upval = self.fsr().blocks.last().is_some_and(|b| b.has_upval);
                let nact = self.fsr().blocks.last().map_or(0, |b| b.nactvar);
                let level = self.fsr().reglevel(nact) as Reg;
                for j in back {
                    self.patch_jump(j, start, has_upval.then_some(level));
                }
                self.leave_block(body.end)?;
                self.leave_block(body.end)?;
            }
            StatKind::If(conds, els) => self.if_stat(conds, els.as_ref())?,
            StatKind::NumFor(f) => {
                self.num_for(&f.var, &f.start, &f.limit, f.step.as_ref(), &f.body, s.pos, f.do_line)?
            }
            StatKind::GenFor { names, exprs, body, line, do_line } => {
                self.gen_for(names, exprs, body, s.pos, *line, *do_line)?
            }
            StatKind::Function { path, method, body } => {
                self.func_stat(path, method.as_deref(), body, s.pos)?
            }
            StatKind::LocalFunction(name, body) => {
                let r = self.alloc_reg(s.pos)?;
                self.add_local(&name.name, VarKind::Reg(r), Attrib::None);
                let p = self.function(body)?;
                self.at(s.pos);
                self.emit(Instr::Closure { a: r, p });
            }
            StatKind::Return(exprs) => self.ret(exprs, s.pos)?,
            StatKind::Break => self.goto_loop(BREAK, s.pos)?,
            StatKind::Continue => self.goto_loop(CONTINUE, s.pos)?,
            StatKind::Goto(name, line) => self.goto(name, Pos { line: *line, col: s.pos.col })?,
            StatKind::Label(name) => self.label_stat(name, s.pos, label_at_end, s.pos.line)?,
            StatKind::TypeDef(..) => {}
        }
        Ok(())
    }

    fn goto_loop(&mut self, name: &str, pos: Pos) -> CResult<()> {
        // Outside any loop it stays pending, reported when the function ends (as C Lua).
        let pc = self.jump();
        let nactvar = self.fsr().actives.len();
        self.fs().gotos.push(GotoDesc { name: name.to_owned(), pc, line: pos.line, nactvar, close: None });
        Ok(())
    }

    fn local_stat(&mut self, names: &[LocalName], exprs: &[Expr], pos: Pos) -> CResult<()> {
        // `local x <const> = constant`: a compile-time constant, no register. As in C Lua's
        // `localstat`, the last name of a statement with as many values as names.
        let mut konst = None;
        if names.len() == exprs.len()
            && let (Some(n), Some(e)) = (names.last(), exprs.last())
            && n.attrib == Attrib::Const
            && let Some(v) = self.const_value(e)?
        {
            if names.len() == 1 {
                self.add_local(&n.name, VarKind::Const(v), Attrib::Const);
                return Ok(());
            }
            konst = Some(v);
        }
        let regs = names.len() - usize::from(konst.is_some());
        let base = self.freereg();
        let given = if konst.is_some() { exprs.get(..regs).unwrap_or_default() } else { exprs };
        self.explist(given, Want::Fixed(regs), pos)?;
        self.set_freereg(usize::from(base) + regs);
        for (i, n) in names.iter().take(regs).enumerate() {
            let r = base + i as Reg;
            self.add_local(&n.name, VarKind::Reg(r), n.attrib);
            if n.attrib == Attrib::Close {
                // C Lua's line: the statement's last token (the end of its values).
                self.at(exprs.last().map_or(n.pos, |e| e.end));
                self.emit(Instr::Tbc { a: r });
                let fs = self.fs();
                fs.tbc_active += 1;
                if let Some(b) = fs.blocks.last_mut() {
                    b.has_upval = true;
                }
            }
        }
        if let (Some(v), Some(n)) = (konst, names.last()) {
            self.add_local(&n.name, VarKind::Const(v), Attrib::Const);
        }
        Ok(())
    }

    /// The value of a constant expression (for `<const>` locals).
    /// The value of a compile-time constant expression, as Lua's `luaK_exp2const` sees it
    /// after its code generator folds (`ast::fold_bin`, `ast::fold_un`): literals, `<const>`
    /// locals, and operators on them (S5COR-13: only literals and names counted, so `local b
    /// <const> = a * 2` with `a` constant got a register, and `t[b]` was no constant key).
    /// A loop over an explicit stack, not recursion.
    fn const_value(&mut self, e: &Expr) -> CResult<Option<Value>> {
        enum Step<'e> {
            Eval(&'e Expr),
            Un(UnOp),
            Bin(BinOp),
        }
        let mut work = vec![Step::Eval(e)];
        let mut vals: Vec<Value> = Vec::new();
        while let Some(step) = work.pop() {
            match step {
                Step::Eval(x) => match &x.kind {
                    ExprKind::Nil => vals.push(Value::Nil),
                    ExprKind::True => vals.push(Value::Bool(true)),
                    ExprKind::False => vals.push(Value::Bool(false)),
                    ExprKind::Int(i) => vals.push(Value::Int(*i)),
                    ExprKind::Num(f) => vals.push(Value::Num(*f)),
                    ExprKind::Str(s) => {
                        let k = self.str_const(s, x.pos)?;
                        vals.push(self.fsr().consts.get(k as usize).copied().unwrap_or_default());
                    }
                    ExprKind::Name(n) => match self.resolve(n) {
                        Some(VarRef::Const(v)) => vals.push(v),
                        _ => return Ok(None),
                    },
                    ExprKind::Paren(i) => work.push(Step::Eval(i)),
                    ExprKind::Un(op @ (UnOp::Neg | UnOp::BNot | UnOp::Not), i) => {
                        work.push(Step::Un(*op));
                        work.push(Step::Eval(i));
                    }
                    ExprKind::Bin(op, l, r) if arith_op(*op).is_some() => {
                        work.push(Step::Bin(*op));
                        work.push(Step::Eval(r));
                        work.push(Step::Eval(l));
                    }
                    _ => return Ok(None),
                },
                Step::Un(op) => {
                    let Some(v) = vals.pop().and_then(|v| crate::ast::fold_un(op, v)) else {
                        return Ok(None);
                    };
                    vals.push(v);
                }
                Step::Bin(op) => {
                    let (Some(b), Some(a)) = (vals.pop(), vals.pop()) else { return Ok(None) };
                    let Some(v) = crate::ast::fold_bin(op, a, b) else { return Ok(None) };
                    vals.push(v);
                }
            }
        }
        Ok(vals.pop())
    }

    /// Refuses an assignment to a const variable, reported at `line` (C Lua's: the token
    /// after the target, S6-FRONT-7).
    fn check_assignable(&self, target: &Expr, line: u32, v: &Option<VarRef>, name: &str) -> CResult<()> {
        match v {
            Some(VarRef::Const(_)) | Some(VarRef::Local(_, _, Attrib::Const | Attrib::Close)) => {}
            // An upvalue made from a const local is const too: look it up outward.
            Some(VarRef::Upval(_)) if self.upval_is_const(name) => {}
            _ => return Ok(()),
        }
        Err(self.err(target.pos, format!("attempt to assign to const variable '{name}'")).lua_line(line))
    }

    fn upval_is_const(&self, name: &str) -> bool {
        for fs in self.fs.iter().rev().skip(1) {
            if let Some(v) = fs.actives.iter().rev().find(|v| &*v.name == name) {
                return matches!(v.attrib, Attrib::Const | Attrib::Close);
            }
        }
        false
    }

    fn assign(&mut self, targets: &[Expr], exprs: &[Expr], follow: &[u32], pos: Pos) -> CResult<()> {
        // `x = e` to a local: evaluate straight into its register.
        if let ([t], [e]) = (targets, exprs)
            && let ExprKind::Name(n) = &t.kind
        {
            let v = self.resolve(n);
            self.check_assignable(t, crate::ast::follow_line(targets, follow, 0), &v, n)?;
            if let Some(VarRef::Local(r, _, _)) = v {
                return self.expr_to_reg(e, r);
            }
        }
        let mut places = Vec::new();
        for (i, t) in targets.iter().enumerate() {
            let p = self.place(t, crate::ast::follow_line(targets, follow, i))?;
            places.push(p);
            self.check_conflict(&mut places, pos)?;
        }
        // C Lua numbers the stores where the values end (the last token read).
        let store_at = exprs.last().map_or(pos, |e| e.end);
        if let ([p], [e]) = (places.as_slice(), exprs) {
            let v = self.expr_rk(e)?;
            self.at(store_at);
            self.store(p, v)?;
            return Ok(());
        }
        let base = self.freereg();
        self.explist(exprs, Want::Fixed(places.len()), pos)?;
        self.at(store_at);
        for (i, p) in places.iter().enumerate().rev() {
            self.store(p, Rk::reg(base + i as Reg))?;
        }
        Ok(())
    }

    /// C Lua's `check_conflict`, when a target has just been read (the last of `places`):
    /// a local or upvalue that an earlier target's table (or key register) is gets copied
    /// now, before the targets after it and the values run, and those earlier targets use
    /// the copy (S5COR-9: the copy was made after every target had been read, so a later
    /// target's key that reassigned the local changed where the earlier store went).
    fn check_conflict(&mut self, places: &mut [Place], pos: Pos) -> CResult<()> {
        let Some((last, earlier)) = places.split_last_mut() else { return Ok(()) };
        let extra = self.freereg();
        let mut conflict = false;
        match *last {
            Place::Local(r) => {
                for p in earlier.iter_mut() {
                    if let Place::Index(t, k) = p {
                        if *t == r {
                            *t = extra;
                            conflict = true;
                        }
                        if let KeyRef::Reg(kr) = k
                            && *kr == r
                        {
                            *kr = extra;
                            conflict = true;
                        }
                    }
                }
            }
            Place::Upval(u) => {
                for p in earlier.iter_mut() {
                    if let Place::Global(t, k) = *p
                        && t == u
                    {
                        *p = Place::Index(extra, KeyRef::K(k));
                        conflict = true;
                    }
                }
            }
            _ => {}
        }
        if conflict {
            let tmp = self.alloc_reg(pos)?;
            match *last {
                Place::Local(r) => self.emit(Instr::Move { a: tmp, b: r }),
                Place::Upval(u) => self.emit(Instr::GetUpval { a: tmp, u }),
                _ => 0,
            };
        }
        Ok(())
    }

    /// `name.key` as an assignment target where `name` is an upvalue and `key` a short
    /// string constant: C Lua stores it with `SETTABUP`, which reads the upvalue when it
    /// stores, after the values (S5COR-9: it was read into a register first, so a value
    /// that reassigned the upvalue saw its store go to the old table). C Lua's own rule:
    /// short strings (40 bytes) among the first 256 constants.
    fn upval_field(&mut self, obj: &Expr, key: &Expr) -> CResult<Option<Place>> {
        let (ExprKind::Name(n), ExprKind::Str(s)) = (&obj.kind, &key.kind) else { return Ok(None) };
        if s.len() > 40 {
            return Ok(None);
        }
        let Some(VarRef::Upval(u)) = self.resolve(n) else { return Ok(None) };
        let k = self.str_const(s, key.pos)?;
        Ok((k <= 255).then_some(Place::Global(u, k)))
    }

    /// Prepares an assignment target: evaluates its table and key.
    fn place(&mut self, t: &Expr, line: u32) -> CResult<Place> {
        match &t.kind {
            ExprKind::Name(n) => {
                let v = self.resolve(n);
                self.check_assignable(t, line, &v, n)?;
                Ok(match v {
                    Some(VarRef::Local(r, _, _)) => Place::Local(r),
                    Some(VarRef::Upval(u)) => Place::Upval(u),
                    Some(VarRef::Const(_)) => Place::Local(0),
                    None => {
                        let k = self.str_const(n.as_bytes(), t.pos)?;
                        match self.env(t.pos)? {
                            VarRef::Upval(u) => Place::Global(u, k),
                            VarRef::Local(r, _, _) => Place::Index(r, KeyRef::K(k)),
                            VarRef::Const(_) => return Err(self.err(t.pos, "_ENV is a constant")),
                        }
                    }
                })
            }
            ExprKind::Index(obj, key) => {
                if let Some(p) = self.upval_field(obj, key)? {
                    return Ok(p);
                }
                let r = self.expr_any_reg(obj)?;
                let k = self.key_ref(key)?;
                Ok(Place::Index(r, k))
            }
            _ => Err(self.err(t.pos, "syntax error (cannot assign to this expression)")),
        }
    }

    fn key_ref(&mut self, key: &Expr) -> CResult<KeyRef> {
        let k = match &key.kind {
            ExprKind::Str(s) => Some(self.str_const(s, key.pos)?),
            ExprKind::Int(i) => Some(self.num_const(Value::Int(*i), key.pos)?),
            ExprKind::Num(f) if !f.is_nan() => Some(self.num_const(Value::Num(*f), key.pos)?),
            ExprKind::True => Some(self.num_const(Value::Bool(true), key.pos)?),
            ExprKind::False => Some(self.num_const(Value::Bool(false), key.pos)?),
            // A `<const>` local, or operators on constants, is the constant it folds to, as
            // in C Lua (`t[k]` with `k <const> = 5` is `t[5]`; S5COR-13).
            ExprKind::Name(_) | ExprKind::Paren(_) | ExprKind::Un(..) | ExprKind::Bin(..) => {
                match self.const_value(key)? {
                    Some(Value::Str(r)) => {
                        let bytes = self.heap.str_bytes(r).to_vec();
                        Some(self.str_const(&bytes, key.pos)?)
                    }
                    Some(v @ (Value::Int(_) | Value::Bool(_))) => Some(self.num_const(v, key.pos)?),
                    Some(Value::Num(f)) if !f.is_nan() => Some(self.num_const(Value::Num(f), key.pos)?),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(k) = k
            && Self::k16(k).is_some()
        {
            return Ok(KeyRef::K(k));
        }
        Ok(KeyRef::Reg(self.expr_any_reg(key)?))
    }

    fn store(&mut self, p: &Place, v: Rk) -> CResult<()> {
        match *p {
            Place::Local(r) => {
                if v.is_const() {
                    self.emit(Instr::LoadK { a: r, k: v.index() as u32 });
                } else if v.index() != usize::from(r) {
                    self.emit(Instr::Move { a: r, b: v.index() as Reg });
                }
            }
            Place::Upval(u) => {
                let r = self.rk_to_reg(v)?;
                self.emit(Instr::SetUpval { a: r, u });
            }
            Place::Global(u, k) => match Self::k16(k) {
                Some(k) => {
                    self.emit(Instr::SetTabUp { u, k, v });
                }
                None => {
                    let t = self.alloc_reg(Pos::default())?;
                    self.emit(Instr::GetUpval { a: t, u });
                    let kr = self.alloc_reg(Pos::default())?;
                    self.emit(Instr::LoadK { a: kr, k });
                    self.emit(Instr::SetTable { a: t, b: kr, c: v });
                }
            },
            Place::Index(t, KeyRef::K(k)) => match Self::k16(k) {
                Some(k) => {
                    self.emit(Instr::SetTableK { a: t, k, c: v });
                }
                None => {
                    let kr = self.alloc_reg(Pos::default())?;
                    self.emit(Instr::LoadK { a: kr, k });
                    self.emit(Instr::SetTable { a: t, b: kr, c: v });
                }
            },
            Place::Index(t, KeyRef::Reg(k)) => {
                self.emit(Instr::SetTable { a: t, b: k, c: v });
            }
        }
        Ok(())
    }

    fn rk_to_reg(&mut self, v: Rk) -> CResult<Reg> {
        if v.is_const() {
            let r = self.alloc_reg(Pos::default())?;
            self.emit(Instr::LoadK { a: r, k: v.index() as u32 });
            Ok(r)
        } else {
            Ok(v.index() as Reg)
        }
    }

    fn op_assign(&mut self, target: &Expr, op: BinOp, e: &Expr, pos: Pos) -> CResult<()> {
        let p = self.place(target, target.pos.line)?;
        // The current value.
        let cur = match &p {
            Place::Local(r) => *r,
            _ => {
                let tmp = self.alloc_reg(pos)?;
                self.load_place(&p, tmp)?;
                tmp
            }
        };
        let dst = cur;
        self.at(pos);
        if op == BinOp::Concat {
            let base = self.freereg();
            let a = self.alloc_reg(pos)?;
            self.emit(Instr::Move { a, b: cur });
            let b = self.alloc_reg(pos)?;
            self.expr_to_reg(e, b)?;
            self.at(pos);
            // Merged with a concatenation `e` ends with, as `x = x .. e` is (LR-6).
            self.concat(a, 2);
            self.emit(Instr::Move { a: dst, b: a });
            self.set_freereg(usize::from(base));
        } else {
            let v = self.expr_rk(e)?;
            self.at(pos);
            let aop = arith_op(op).ok_or_else(|| self.err(pos, "bad compound operator"))?;
            self.emit(Instr::Arith { op: aop, a: dst, b: Rk::reg(cur), c: v });
        }
        if !matches!(p, Place::Local(_)) {
            self.store(&p, Rk::reg(dst))?;
        }
        Ok(())
    }

    fn load_place(&mut self, p: &Place, dst: Reg) -> CResult<()> {
        match *p {
            Place::Local(r) => {
                self.emit(Instr::Move { a: dst, b: r });
            }
            Place::Upval(u) => {
                self.emit(Instr::GetUpval { a: dst, u });
            }
            Place::Global(u, k) => match Self::k16(k) {
                Some(k) => {
                    self.emit(Instr::GetTabUp { a: dst, u, k });
                }
                None => {
                    self.emit(Instr::GetUpval { a: dst, u });
                    let kr = self.alloc_reg(Pos::default())?;
                    self.emit(Instr::LoadK { a: kr, k });
                    self.emit(Instr::GetTable { a: dst, b: dst, c: kr });
                }
            },
            Place::Index(t, KeyRef::K(k)) => match Self::k16(k) {
                Some(k) => {
                    self.emit(Instr::GetTableK { a: dst, b: t, k });
                }
                None => {
                    let kr = self.alloc_reg(Pos::default())?;
                    self.emit(Instr::LoadK { a: kr, k });
                    self.emit(Instr::GetTable { a: dst, b: t, c: kr });
                }
            },
            Place::Index(t, KeyRef::Reg(k)) => {
                self.emit(Instr::GetTable { a: dst, b: t, c: k });
            }
        }
        Ok(())
    }

    fn if_stat(&mut self, conds: &[(Expr, Block)], els: Option<&Block>) -> CResult<()> {
        let mut to_end = Vec::new();
        let n = conds.len();
        for (i, (cond, block)) in conds.iter().enumerate() {
            self.at(cond.pos);
            let skip = self.cond_jump(cond, false)?;
            self.enter_block(false);
            self.block_body(block, false)?;
            self.leave_block(block.end)?;
            let last = i + 1 == n && els.is_none();
            if !last && !block_ends_in_jump(block) {
                to_end.push(self.jump());
            }
            self.patch_here(&skip);
        }
        if let Some(b) = els {
            self.enter_block(false);
            self.block_body(b, false)?;
            self.leave_block(b.end)?;
        }
        self.patch_here(&to_end);
        Ok(())
    }

    fn num_for(
        &mut self,
        var: &LocalName,
        start: &Expr,
        limit: &Expr,
        step: Option<&Expr>,
        body: &Block,
        pos: Pos,
        do_line: u32,
    ) -> CResult<()> {
        self.enter_block(true);
        let base = self.freereg();
        self.expr_next(start)?;
        self.expr_next(limit)?;
        match step {
            Some(s) => {
                self.expr_next(s)?;
            }
            None => {
                let r = self.alloc_reg(pos)?;
                self.emit(Instr::LoadInt { a: r, i: 1 });
            }
        }
        // Hidden control registers, then the loop variable.
        self.add_local("(for state)", VarKind::Reg(base), Attrib::None);
        self.add_local("(for state)", VarKind::Reg(base + 1), Attrib::None);
        self.add_local("(for state)", VarKind::Reg(base + 2), Attrib::None);
        self.at(Pos { line: do_line, col: 1 });
        let prep = self.emit(Instr::ForPrep { a: base, off: 0 });
        let body_start = self.pc();
        self.enter_block(false);
        let r = self.alloc_reg(var.pos)?;
        self.add_local(&var.name, VarKind::Reg(r), Attrib::None);
        self.block_body(body, false)?;
        self.leave_block(body.end)?;
        let first_goto = self.fsr().blocks.last().map_or(0, |b| b.first_goto);
        self.create_label(CONTINUE, body.end.line, None, first_goto, None)?;
        self.at(pos);
        let lp = self.pc();
        self.emit(Instr::ForLoop { a: base, off: (body_start as i64 - lp as i64 - 1) as i32 });
        let after = self.pc();
        self.set_instr(prep, Instr::ForPrep { a: base, off: (after as i64 - prep as i64 - 1) as i32 });
        self.leave_block(body.end)?;
        Ok(())
    }

    fn gen_for(
        &mut self,
        names: &[LocalName],
        exprs: &[Expr],
        body: &Block,
        pos: Pos,
        line: u32,
        do_line: u32,
    ) -> CResult<()> {
        self.enter_block(true);
        let base = self.freereg();
        self.explist(exprs, Want::Fixed(4), pos)?;
        self.set_freereg(usize::from(base) + 4);
        // S5COR-4: the closing value (the fourth) is to-be-closed, so the loop is "inside a
        // to-be-closed variable" (C Lua's `marktobeclosed`) until its end: no tail call in
        // its body, else `return f()` closed it before `f` ran (and `__close` saw no error).
        for i in 0..4 {
            let attrib = if i == 3 { Attrib::Close } else { Attrib::None };
            self.add_local("(for state)", VarKind::Reg(base + i), attrib);
        }
        self.fs().tbc_active += 1;
        // The closing value is to-be-closed: leaving the loop closes it.
        if let Some(b) = self.fs().blocks.last_mut() {
            b.has_upval = true;
        }
        self.at(Pos { line: do_line, col: 1 });
        let prep = self.emit(Instr::TForPrep { a: base, off: 0 });
        let body_start = self.pc();
        self.enter_block(false);
        for n in names {
            let r = self.alloc_reg(n.pos)?;
            self.add_local(&n.name, VarKind::Reg(r), Attrib::None);
        }
        self.block_body(body, false)?;
        self.leave_block(body.end)?;
        let first_goto = self.fsr().blocks.last().map_or(0, |b| b.first_goto);
        self.create_label(CONTINUE, body.end.line, None, first_goto, None)?;
        self.at(Pos { line, col: 1 });
        let call = self.emit(Instr::TForCall { a: base, c: names.len() as Reg });
        self.set_instr(prep, Instr::TForPrep { a: base, off: (call as i64 - prep as i64 - 1) as i32 });
        if usize::from(base) + 4 + names.len() > self.fsr().max_stack {
            self.fs().max_stack = usize::from(base) + 4 + names.len();
        }
        let lp = self.pc();
        self.emit(Instr::TForLoop { a: base, off: (body_start as i64 - lp as i64 - 1) as i32 });
        self.leave_block(body.end)?;
        Ok(())
    }

    fn func_stat(&mut self, path: &[String], method: Option<&str>, body: &FuncBody, pos: Pos) -> CResult<()> {
        let Some(first) = path.first() else { return Ok(()) };
        if path.len() == 1 && method.is_none() {
            let target = Expr { kind: ExprKind::Name(first.clone()), pos, end: pos };
            // A const `x`: checked after the body, at the token after `end` (S6-FRONT-7).
            let v = self.resolve(first);
            if let Err(d) = self.check_assignable(&target, body.close_line, &v, first) {
                self.function(body)?;
                return Err(d);
            }
            let place = self.place(&target, pos.line)?;
            let r = if let Place::Local(r) = place { r } else { self.alloc_reg(pos)? };
            let p = self.function(body)?;
            self.at(pos);
            self.emit(Instr::Closure { a: r, p });
            if !matches!(place, Place::Local(_)) {
                self.store(&place, Rk::reg(r))?;
            }
            return Ok(());
        }
        // a.b.c[:m]: evaluate a.b, then store the closure under the last name.
        let mut obj = Expr { kind: ExprKind::Name(first.clone()), pos, end: pos };
        let (last, prefix) = match method {
            Some(m) => (m.to_owned(), path),
            None => {
                (path.last().cloned().unwrap_or_default(), path.get(..path.len() - 1).unwrap_or_default())
            }
        };
        for p in prefix.iter().skip(1) {
            let k = Expr { kind: ExprKind::Str(p.clone().into_bytes()), pos, end: pos };
            obj = Expr { kind: ExprKind::Index(Box::new(obj), Box::new(k)), pos, end: pos };
        }
        let t = self.expr_any_reg(&obj)?;
        let k = self.str_const(last.as_bytes(), pos)?;
        let r = self.alloc_reg(pos)?;
        let p = self.function(body)?;
        self.at(pos);
        self.emit(Instr::Closure { a: r, p });
        self.store(&Place::Index(t, KeyRef::K(k)), Rk::reg(r))?;
        Ok(())
    }

    fn ret(&mut self, exprs: &[Expr], pos: Pos) -> CResult<()> {
        // `return f(x)`: a tail call, unless a to-be-closed variable must close first.
        if let [e] = exprs
            && matches!(e.kind, ExprKind::Call(..) | ExprKind::Method(..))
            && self.fsr().tbc_active == 0
        {
            let base = self.freereg();
            self.call_setup(e, base)?;
            let nargs = self.call_args_count;
            self.at(e.call_at());
            self.emit(Instr::TailCall { a: base, b: nargs });
            // Reached when the callee is not a Lua function (it ran as a plain call).
            self.emit(Instr::Return { a: base, b: MULTI });
            return Ok(());
        }
        let base = self.freereg();
        if let [e] = exprs
            && !e.kind.is_multi()
        {
            // One value: return it from where it is.
            let r = self.expr_any_reg(e)?;
            self.at(e.end);
            self.emit(Instr::Return { a: r, b: 1 });
            return Ok(());
        }
        let n = self.explist(exprs, Want::Multi, pos)?;
        // C Lua numbers the return where its values end (the last token read), S5COR-15.
        self.at(exprs.last().map_or(pos, |e| e.end));
        self.emit(Instr::Return { a: base, b: n });
        Ok(())
    }

    // ---- expressions ----

    /// Evaluates `exprs` into consecutive registers from the first free one. Returns the
    /// count field for the consumer (MULTI when the last expression gave a variable number).
    fn explist(&mut self, exprs: &[Expr], want: Want, pos: Pos) -> CResult<Reg> {
        let base = usize::from(self.freereg());
        let n = exprs.len();
        for (i, e) in exprs.iter().enumerate() {
            let last = i + 1 == n;
            if last && e.kind.is_multi() {
                match want {
                    Want::Multi => {
                        self.multi(e, MULTI)?;
                        return Ok(MULTI);
                    }
                    Want::Fixed(w) => {
                        let needed = w.saturating_sub(n - 1);
                        if needed == 0 {
                            self.expr_next(e)?;
                        } else {
                            self.multi(e, needed as Reg)?;
                            self.set_freereg(usize::from(self.freereg()) + needed);
                            if usize::from(self.freereg()) > self.fsr().max_stack {
                                let f = usize::from(self.freereg());
                                self.fs().max_stack = f;
                            }
                        }
                    }
                }
            } else {
                self.expr_next(e)?;
            }
        }
        let got = usize::from(self.freereg()) - base;
        match want {
            Want::Fixed(w) => {
                if got < w {
                    let first = self.freereg();
                    self.reserve(w - got, pos)?;
                    self.emit(Instr::LoadNil { a: first, n: (w - got - 1) as Reg });
                }
                Ok(w.min(usize::from(MULTI) - 1) as Reg)
            }
            Want::Multi => {
                if got >= usize::from(MULTI) {
                    return Err(self.err(pos, "too many values"));
                }
                Ok(got as Reg)
            }
        }
    }

    /// A call or `...` producing `n` results (MULTI: all, setting the top) from the first
    /// free register (which it does not reserve).
    fn multi(&mut self, e: &Expr, n: Reg) -> CResult<()> {
        match &e.kind {
            ExprKind::Dots => {
                let a = self.freereg();
                self.at(e.pos);
                self.emit(Instr::VarArg { a, n });
                if n != MULTI {
                    let need = usize::from(a) + usize::from(n);
                    if need > self.fsr().max_stack {
                        self.fs().max_stack = need;
                    }
                }
            }
            _ => {
                let base = self.freereg();
                self.call_setup(e, base)?;
                let nargs = self.call_args_count;
                self.at(e.call_at());
                self.emit(Instr::Call { a: base, b: nargs, c: n });
                self.set_freereg(usize::from(base));
                if n != MULTI {
                    let need = usize::from(base) + usize::from(n);
                    if need > self.fsr().max_stack {
                        self.fs().max_stack = need;
                    }
                }
            }
        }
        Ok(())
    }

    /// A call, as a statement or for `want` results placed from the first free register.
    fn call(&mut self, e: &Expr, want: Want) -> CResult<Reg> {
        let base = self.freereg();
        self.call_setup(e, base)?;
        let nargs = self.call_args_count;
        let c = match want {
            Want::Fixed(n) => n as Reg,
            Want::Multi => MULTI,
        };
        self.at(e.call_at());
        self.emit(Instr::Call { a: base, b: nargs, c });
        self.set_freereg(usize::from(base));
        Ok(base)
    }

    /// Puts the function and its arguments in registers from `base`; leaves the argument
    /// count field in `call_args_count`.
    fn call_setup(&mut self, e: &Expr, base: Reg) -> CResult<()> {
        match &e.kind {
            ExprKind::Call(f, args) => {
                self.expr_next(f)?;
                let n = self.explist(args, Want::Multi, e.pos)?;
                self.call_args_count = n;
            }
            ExprKind::Method(obj, name, npos, args) => {
                let o = self.expr_any_reg(obj)?;
                self.set_freereg(usize::from(base));
                self.reserve(2, e.pos)?;
                let k = self.str_const(name.as_bytes(), e.pos)?;
                self.at(*npos);
                match Self::k16(k) {
                    Some(k) => {
                        self.emit(Instr::SelfK { a: base, b: o, k });
                    }
                    None => {
                        self.emit(Instr::Move { a: base + 1, b: o });
                        let kr = self.alloc_reg(e.pos)?;
                        self.emit(Instr::LoadK { a: kr, k });
                        self.emit(Instr::GetTable { a: base, b: base + 1, c: kr });
                        self.set_freereg(usize::from(base) + 2);
                    }
                }
                let n = self.explist(args, Want::Multi, e.pos)?;
                self.call_args_count = if n == MULTI { MULTI } else { n + 1 };
            }
            _ => return Err(self.err(e.pos, "internal: not a call")),
        }
        Ok(())
    }

    /// The value of `e` in the next free register (reserved).
    fn expr_next(&mut self, e: &Expr) -> CResult<Reg> {
        if matches!(e.kind, ExprKind::Call(..) | ExprKind::Method(..)) {
            let base = self.call(e, Want::Fixed(1))?;
            self.reserve(1, e.pos)?;
            return Ok(base);
        }
        if let ExprKind::Dots = e.kind {
            let a = self.freereg();
            self.at(e.pos);
            self.emit(Instr::VarArg { a, n: 1 });
            self.reserve(1, e.pos)?;
            return Ok(a);
        }
        let r = self.alloc_reg(e.pos)?;
        self.expr_to_reg(e, r)?;
        Ok(r)
    }

    /// A register holding `e`: a local's own register, or a new temporary.
    fn expr_any_reg(&mut self, e: &Expr) -> CResult<Reg> {
        if let ExprKind::Name(n) = &e.kind
            && let Some(VarRef::Local(r, _, _)) = self.resolve(n)
        {
            return Ok(r);
        }
        if let ExprKind::Paren(inner) = &e.kind
            && let ExprKind::Name(_) = inner.kind
        {
            return self.expr_any_reg(inner);
        }
        self.expr_next(e)
    }

    /// A constant slot or a register holding `e`.
    fn expr_rk(&mut self, e: &Expr) -> CResult<Rk> {
        let k = match &e.kind {
            ExprKind::Str(s) => Some(self.str_const(s, e.pos)?),
            ExprKind::Int(i) => Some(self.num_const(Value::Int(*i), e.pos)?),
            ExprKind::Num(f) if !f.is_nan() => Some(self.num_const(Value::Num(*f), e.pos)?),
            ExprKind::Nil => Some(self.num_const(Value::Nil, e.pos)?),
            ExprKind::True => Some(self.num_const(Value::Bool(true), e.pos)?),
            ExprKind::False => Some(self.num_const(Value::Bool(false), e.pos)?),
            ExprKind::Name(n) => match self.resolve(n) {
                Some(VarRef::Const(v)) => Some(self.value_const(v, e.pos)?),
                _ => None,
            },
            _ => None,
        };
        if let Some(k) = k
            && let Some(k) = Self::k16(k)
        {
            return Ok(Rk::konst(k));
        }
        Ok(Rk::reg(self.expr_any_reg(e)?))
    }

    fn value_const(&mut self, v: Value, pos: Pos) -> CResult<u32> {
        match v {
            Value::Str(s) => {
                let bytes = self.heap.str_bytes(s).to_vec();
                self.str_const(&bytes, pos)
            }
            other => self.num_const(other, pos),
        }
    }

    fn load_const(&mut self, k: u32, dst: Reg) {
        self.emit(Instr::LoadK { a: dst, k });
    }

    /// Evaluates `e` into register `dst` (one value).
    fn deep(&self, pos: Pos) -> CResult<()> {
        if self.stack_base.abs_diff(crate::stack_pos()) > self.stack_bytes {
            return Err(self.err(pos, "chunk has too many syntax levels (out of stack)"));
        }
        Ok(())
    }

    fn expr_to_reg(&mut self, e: &Expr, dst: Reg) -> CResult<()> {
        self.deep(e.pos)?;
        self.at(e.pos);
        let save = self.freereg();
        match &e.kind {
            ExprKind::Nil => {
                self.emit(Instr::LoadNil { a: dst, n: 0 });
            }
            ExprKind::True => {
                self.emit(Instr::LoadBool { a: dst, v: true });
            }
            ExprKind::False => {
                self.emit(Instr::LoadBool { a: dst, v: false });
            }
            ExprKind::Int(i) => match i32::try_from(*i) {
                Ok(i) => {
                    self.emit(Instr::LoadInt { a: dst, i });
                }
                Err(_) => {
                    let k = self.num_const(Value::Int(*i), e.pos)?;
                    self.load_const(k, dst);
                }
            },
            ExprKind::Num(f) => {
                let k = self.num_const(Value::Num(*f), e.pos)?;
                self.load_const(k, dst);
            }
            ExprKind::Str(s) => {
                let k = self.str_const(s, e.pos)?;
                self.load_const(k, dst);
            }
            ExprKind::Dots => {
                self.emit(Instr::VarArg { a: dst, n: 1 });
            }
            ExprKind::Paren(inner) => self.expr_to_reg(inner, dst)?,
            ExprKind::Name(n) => match self.resolve(n) {
                Some(VarRef::Local(r, _, _)) => {
                    self.move_to(dst, r);
                }
                Some(VarRef::Const(v)) => {
                    let k = self.value_const(v, e.pos)?;
                    self.load_const(k, dst);
                }
                Some(VarRef::Upval(u)) => {
                    self.emit(Instr::GetUpval { a: dst, u });
                }
                None => {
                    let k = self.str_const(n.as_bytes(), e.pos)?;
                    match self.env(e.pos)? {
                        VarRef::Upval(u) => self.load_place(&Place::Global(u, k), dst)?,
                        VarRef::Local(r, _, _) => self.load_place(&Place::Index(r, KeyRef::K(k)), dst)?,
                        VarRef::Const(_) => return Err(self.err(e.pos, "_ENV is a constant")),
                    }
                }
            },
            ExprKind::Index(obj, key) => {
                let t = self.expr_any_reg(obj)?;
                let k = self.key_ref(key)?;
                self.at(e.end); // C Lua reads a field where its key ends
                self.load_place(&Place::Index(t, k), dst)?;
            }
            ExprKind::Call(..) | ExprKind::Method(..) => {
                let base = self.call(e, Want::Fixed(1))?;
                self.move_to(dst, base);
            }
            ExprKind::Func(f) => {
                let p = self.function(f)?;
                self.at(e.pos);
                self.emit(Instr::Closure { a: dst, p });
            }
            ExprKind::Table(fields) => self.table(fields, dst, e.pos)?,
            ExprKind::Concat(parts) => {
                // In `dst` itself only when it is a temporary (a local may be read by the
                // parts).
                if usize::from(dst) + 1 == usize::from(self.freereg())
                    && usize::from(dst) >= self.fsr().nvarstack()
                {
                    self.set_freereg(usize::from(dst));
                }
                let base = self.freereg();
                for p in parts {
                    self.expr_next(p)?;
                }
                self.at(e.pos);
                self.concat(base, parts.len());
                self.move_to(dst, base);
            }
            ExprKind::Interp(parts) => self.interp(parts, dst, e.pos)?,
            ExprKind::Un(op, inner) => {
                let r = self.expr_any_reg(inner)?;
                self.at(e.pos);
                self.emit(match op {
                    UnOp::Neg => Instr::Unm { a: dst, b: r },
                    UnOp::Not => Instr::Not { a: dst, b: r },
                    UnOp::Len => Instr::Len { a: dst, b: r },
                    UnOp::BNot => Instr::BNot { a: dst, b: r },
                });
            }
            ExprKind::Bin(op, l, r) => self.binary(*op, l, r, dst, e.pos)?,
        }
        self.set_freereg(usize::from(save));
        Ok(())
    }

    fn interp(&mut self, parts: &[InterpExpr], dst: Reg, pos: Pos) -> CResult<()> {
        // Built in `dst` itself when it is a temporary, as a concatenation is (LR-6).
        if usize::from(dst) + 1 == usize::from(self.freereg()) && usize::from(dst) >= self.fsr().nvarstack() {
            self.set_freereg(usize::from(dst));
        }
        let base = self.freereg();
        for p in parts {
            match p {
                InterpExpr::Lit(b) => {
                    let r = self.alloc_reg(pos)?;
                    let k = self.str_const(b, pos)?;
                    self.load_const(k, r);
                }
                InterpExpr::Expr(e) => {
                    let r = self.expr_next(e)?;
                    self.at(e.pos);
                    self.emit(Instr::ToStr { a: r, b: r });
                }
            }
        }
        self.at(pos);
        if parts.len() == 1 {
            self.move_to(dst, base);
        } else {
            self.at(Pos { line: crate::ast::interp_join_line(parts, pos), col: 1 });
            self.emit(Instr::Concat { a: base, n: parts.len() as Reg });
            self.move_to(dst, base);
        }
        Ok(())
    }

    fn table(&mut self, fields: &[Field], dst: Reg, pos: Pos) -> CResult<()> {
        // Built in `dst` itself when it is the newest register (items go right above it).
        // In `dst` itself only when it is the newest temporary: a local may be read by the
        // fields (`x = {x}`).
        let in_place =
            usize::from(dst) + 1 == usize::from(self.freereg()) && usize::from(dst) >= self.fsr().nvarstack();
        let t = if in_place { dst } else { self.alloc_reg(pos)? };
        let narr = fields.iter().filter(|f| matches!(f, Field::Pos(_))).count();
        let nhash = fields.iter().filter(|f| !matches!(f, Field::Pos(_))).count();
        // A final call or `...` is not counted in Lua's array size (`lastlistfield`): the
        // table is made with room for it, sized as C Lua sizes it (`NEWTABLE_MULTRET`).
        let multret = matches!(fields.last(), Some(Field::Pos(e)) if e.kind.is_multi());
        let hash = nhash.min(crate::bytecode::NEWTABLE_HASH_MAX as usize) as u16
            | if multret { crate::bytecode::NEWTABLE_MULTRET } else { 0 };
        self.emit(Instr::NewTable { a: t, arr: narr.min(u16::MAX as usize) as u16, hash });
        let mut pending = 0usize;
        let mut stored = 0usize;
        let n = fields.len();
        for (i, f) in fields.iter().enumerate() {
            match f {
                Field::Pos(e) => {
                    let last = i + 1 == n;
                    if last && e.kind.is_multi() {
                        self.multi(e, MULTI)?;
                        self.at(pos);
                        self.emit(Instr::SetList { a: t, n: SETLIST_MULTI, off: stored as u32 });
                        pending = 0;
                        self.set_freereg(usize::from(t) + 1);
                        continue;
                    }
                    self.expr_next(e)?;
                    pending += 1;
                    if pending == LIST_FLUSH {
                        self.at(pos);
                        self.emit(Instr::SetList { a: t, n: pending as u8, off: stored as u32 });
                        stored += pending;
                        pending = 0;
                        self.set_freereg(usize::from(t) + 1);
                    }
                }
                Field::Named(name, v, fpos) => {
                    let save = self.freereg();
                    let k = self.str_const(name.as_bytes(), *fpos)?;
                    let val = self.expr_rk(v)?;
                    self.at(*fpos);
                    self.at(crate::compiler::field_store_pos(v));
                    self.store(&Place::Index(t, KeyRef::K(k)), val)?;
                    self.set_freereg(usize::from(save));
                }
                Field::Keyed(k, v) => {
                    let save = self.freereg();
                    let kr = self.key_ref(k)?;
                    let val = self.expr_rk(v)?;
                    self.at(k.pos);
                    self.at(crate::compiler::field_store_pos(v));
                    self.store(&Place::Index(t, kr), val)?;
                    self.set_freereg(usize::from(save));
                }
            }
        }
        if pending > 0 {
            self.at(pos);
            self.emit(Instr::SetList { a: t, n: pending as u8, off: stored as u32 });
        }
        self.move_to(dst, t);
        Ok(())
    }

    fn binary(&mut self, op: BinOp, l: &Expr, r: &Expr, dst: Reg, pos: Pos) -> CResult<()> {
        match op {
            BinOp::And | BinOp::Or if decided_by_constant(op, fold_logic(l)) => {
                // `k and x` with k a true constant, `nil or x`: just x (as Lua's code
                // generator folds them).
                self.expr_to_reg(r, dst)?;
            }
            BinOp::And | BinOp::Or => {
                let l = fold_logic(l);
                // Into a temporary when `dst` is a local the right side may read.
                let target =
                    if usize::from(dst) < self.fsr().nvarstack() { self.alloc_reg(pos)? } else { dst };
                let local = match &l.kind {
                    ExprKind::Paren(inner) if matches!(inner.kind, ExprKind::Name(_)) => &**inner,
                    _ => l,
                };
                let local = match &local.kind {
                    ExprKind::Name(n) => match self.resolve(n) {
                        Some(VarRef::Local(r, _, _)) => Some(r),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(lr) = local {
                    // A local on the left: tested and copied in one step, as C Lua.
                    self.at(pos);
                    self.emit(Instr::TestSet { a: target, b: lr, k: op == BinOp::Or });
                } else {
                    self.expr_to_reg(l, target)?;
                    self.at(pos);
                    // and: jump to the end when false; or: when true.
                    self.emit(Instr::Test { a: target, k: op == BinOp::Or });
                }
                let j = self.jump();
                self.expr_to_reg(r, target)?;
                self.patch_here(&[j]);
                self.move_to(dst, target);
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                let jumps = self.cond_jump(
                    &Expr {
                        kind: ExprKind::Bin(op, Box::new(l.clone()), Box::new(r.clone())),
                        pos,
                        end: r.end,
                    },
                    true,
                )?;
                self.emit(Instr::LoadFalseSkip { a: dst });
                self.patch_here(&jumps);
                self.emit(Instr::LoadBool { a: dst, v: true });
            }
            BinOp::Concat => {
                let base = self.freereg();
                self.expr_next(l)?;
                self.expr_next(r)?;
                self.at(pos);
                self.concat(base, 2);
                self.move_to(dst, base);
            }
            _ => {
                // A left-deep chain (`a + b + c ...`) compiles in a loop, not by recursion.
                let mut spine: Vec<(BinOp, &Expr, Pos)> = vec![(op, r, pos)];
                let mut left = l;
                while let ExprKind::Bin(op2, l2, r2) = &left.kind {
                    if arith_op(*op2).is_none() {
                        break;
                    }
                    spine.push((*op2, r2, left.pos));
                    left = l2;
                }
                if spine.len() == 1 {
                    let b = self.expr_rk(l)?;
                    let c = self.expr_rk(r)?;
                    self.at(pos);
                    let aop = arith_op(op).ok_or_else(|| self.err(pos, "internal: operator"))?;
                    self.emit(Instr::Arith { op: aop, a: dst, b, c });
                    return Ok(());
                }
                // The running value lives in a temporary (a local `dst` may be read later).
                let temp = usize::from(dst) >= self.fsr().nvarstack()
                    && usize::from(dst) + 1 == usize::from(self.freereg());
                let acc = if temp { dst } else { self.alloc_reg(pos)? };
                let mut b = self.expr_rk(left)?;
                while let Some((op, r, pos)) = spine.pop() {
                    let save = self.freereg();
                    let c = self.expr_rk(r)?;
                    self.at(pos);
                    let aop = arith_op(op).ok_or_else(|| self.err(pos, "internal: operator"))?;
                    self.emit(Instr::Arith { op: aop, a: acc, b, c });
                    self.set_freereg(usize::from(save).max(usize::from(acc) + 1));
                    b = Rk::reg(acc);
                }
                self.move_to(dst, acc);
            }
        }
        Ok(())
    }

    /// Code that jumps (returned jumps, to patch) when `e`'s truth is `when`, and falls
    /// through otherwise.
    fn cond_jump(&mut self, e: &Expr, when: bool) -> CResult<Vec<usize>> {
        self.deep(e.pos)?;
        self.at(e.pos);
        match &e.kind {
            ExprKind::Nil | ExprKind::False => {
                return Ok(if when { Vec::new() } else { vec![self.jump()] });
            }
            ExprKind::True | ExprKind::Int(_) | ExprKind::Num(_) | ExprKind::Str(_) => {
                return Ok(if when { vec![self.jump()] } else { Vec::new() });
            }
            ExprKind::Paren(inner) if !inner.kind.is_multi() => return self.cond_jump(inner, when),
            ExprKind::Un(UnOp::Not, inner) => return self.cond_jump(inner, !when),
            ExprKind::Bin(BinOp::And, l, r) => {
                if when {
                    let skip = self.cond_jump(l, false)?;
                    let j = self.cond_jump(r, true)?;
                    self.patch_here(&skip);
                    return Ok(j);
                }
                let mut j = self.cond_jump(l, false)?;
                j.extend(self.cond_jump(r, false)?);
                return Ok(j);
            }
            ExprKind::Bin(BinOp::Or, l, r) => {
                if when {
                    let mut j = self.cond_jump(l, true)?;
                    j.extend(self.cond_jump(r, true)?);
                    return Ok(j);
                }
                let skip = self.cond_jump(l, true)?;
                let j = self.cond_jump(r, false)?;
                self.patch_here(&skip);
                return Ok(j);
            }
            ExprKind::Bin(
                op @ (BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge),
                l,
                r,
            ) => {
                let save = self.freereg();
                let (a, b) = (self.expr_rk(l)?, self.expr_rk(r)?);
                self.at(r.end); // C Lua numbers a comparison where its right side ends
                let i = match op {
                    BinOp::Eq => Instr::Eq { b: a, c: b, k: when },
                    BinOp::Ne => Instr::Eq { b: a, c: b, k: !when },
                    BinOp::Lt => Instr::Lt { b: a, c: b, k: when },
                    BinOp::Le => Instr::Le { b: a, c: b, k: when },
                    BinOp::Gt => Instr::Lt { b, c: a, k: when },
                    _ => Instr::Le { b, c: a, k: when },
                };
                self.emit(i);
                let j = self.jump();
                self.set_freereg(usize::from(save));
                return Ok(vec![j]);
            }
            _ => {}
        }
        let save = self.freereg();
        let r = self.expr_any_reg(e)?;
        self.at(e.pos);
        self.emit(Instr::Test { a: r, k: when });
        let j = self.jump();
        self.set_freereg(usize::from(save));
        Ok(vec![j])
    }
}

#[derive(Clone, Copy)]
enum VarRef {
    Local(Reg, usize, Attrib),
    Upval(Reg),
    Const(Value),
}

#[derive(Clone, Copy)]
enum KeyRef {
    K(u32),
    Reg(Reg),
}

#[derive(Clone, Copy)]
enum Place {
    Local(Reg),
    Upval(Reg),
    Global(Reg, u32),
    Index(Reg, KeyRef),
}

/// `a and b` / `a or b` after folding constant left sides away.
fn fold_logic(e: &Expr) -> &Expr {
    match &e.kind {
        ExprKind::Bin(op @ (BinOp::And | BinOp::Or), l, r) if decided_by_constant(*op, fold_logic(l)) => {
            fold_logic(r)
        }
        _ => e,
    }
}
