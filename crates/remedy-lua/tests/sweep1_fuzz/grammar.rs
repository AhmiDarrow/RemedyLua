//! A typed, grammar-based generator of terminating Lua 5.4 programs.
//!
//! Every program is produced twice at once: `r` (RemedyLua dialect when `rl` is on:
//! annotations, compound assignment, `continue`, interpolation) and `l`, the equivalent
//! plain Lua. With `rl` off the two are identical. Line structure is the same in both so
//! error positions agree.
//!
//! Termination by construction: numeric loops have small literal bounds (or a guard
//! counter), `while`/`repeat` carry a guard counter, every function body starts with
//! `if __enter() then return end` (a global call budget), backward `goto` has a counter.

use super::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    Int,
    Flt,
    Str,
    Bool,
    Nil,
    Tab,
    Fun,
    Any,
}

const CONCRETE: [Ty; 7] = [Ty::Int, Ty::Flt, Ty::Str, Ty::Bool, Ty::Nil, Ty::Tab, Ty::Fun];

#[derive(Default, Clone, Debug)]
pub struct Out {
    pub r: String,
    pub l: String,
}

pub trait Piece {
    fn put(&self, o: &mut Out);
}
impl Piece for &str {
    fn put(&self, o: &mut Out) {
        o.r.push_str(self);
        o.l.push_str(self);
    }
}
impl Piece for String {
    fn put(&self, o: &mut Out) {
        o.r.push_str(self);
        o.l.push_str(self);
    }
}
impl Piece for &Out {
    fn put(&self, o: &mut Out) {
        (*self).put(o);
    }
}
impl Piece for Out {
    fn put(&self, o: &mut Out) {
        o.r.push_str(&self.r);
        o.l.push_str(&self.l);
    }
}

pub fn cat(ps: &[&dyn Piece]) -> Out {
    let mut o = Out::default();
    for p in ps {
        p.put(&mut o);
    }
    o
}

macro_rules! c {
    ($($p:expr),* $(,)?) => { cat(&[$(&$p),*]) };
}

/// `[k]`, with a space when `k` starts with `[`: `t[[[x]]]` lexes as `t` called with the
/// long string `[[[x]]` (long-run finding).
fn index(k: impl Piece) -> Out {
    let k = c!(k);
    if k.l.starts_with('[') || k.r.starts_with('[') { c!("[ ", k, "]") } else { c!("[", k, "]") }
}

fn split(r: impl Into<String>, l: impl Into<String>) -> Out {
    Out { r: r.into(), l: l.into() }
}

fn join(xs: &[Out], sep: &str) -> Out {
    let mut o = Out::default();
    for (i, x) in xs.iter().enumerate() {
        if i > 0 {
            sep.put(&mut o);
        }
        x.put(&mut o);
    }
    o
}

#[derive(Clone, Debug)]
struct Var {
    name: String,
    ty: Ty,
    konst: bool,
    /// Methods defined on this table with `function v:m()`.
    methods: Vec<String>,
}

/// The loop a `continue` belongs to, for `continue_body`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoopKind {
    While,
    /// A numeric or generic `for`.
    For,
    /// `repeat ... until` whose condition does not use the body's locals (the generator's).
    Repeat,
    /// `repeat ... until` whose condition uses the body's locals: every local declared at
    /// the body's own level must come before every `continue` (RemedyLua rejects the
    /// program otherwise, with C Lua's own `jumps into the scope of local`), and the body
    /// must not end in `return`.
    RepeatScoped,
}

/// The plain-Lua translation of a `.rlua` loop body that uses `continue` (`body.l` already
/// has each `continue` as `goto <label>`): where `::label::` goes so that every such
/// `goto` sees it under Lua 5.4's rules, with the meaning RemedyLua gives `continue`
/// (which compiles it as a jump to a label of its own at the same place).
///
/// - `while`, numeric and generic `for`: `::label::` is the last statement of the body
///   block, at the body's own level (never inside a nested block: a `goto` sees only
///   labels in enclosing blocks). A label followed by nothing but `end` is outside the
///   scope of the body's locals, so a `goto` may jump to it past `local` (`<const>`,
///   `<close>` too) declarations.
/// - `repeat`: before `until` the label is not at the end of its block (the condition is
///   inside the body's scope), so a jump to it past a local is an error. The body goes in
///   a `do ... end` on both sides and the label follows that block: the `continue` jumps
///   out of the inner block (closing its `<close>` locals) and then evaluates `until`.
///   Both sides get the block, so the condition sees the same names in both (none of the
///   body's locals: the generator never lets it use them).
/// - `repeat` whose condition uses the body's locals (`RepeatScoped`): no block; the label
///   goes straight before `until`, as RemedyLua places its own. Valid exactly when no
///   body-level local is declared between a `continue` and the label, which is also
///   RemedyLua's rule for `continue` there.
/// - A body that ends in `return` cannot be followed by a label: it goes in a `do ... end`.
///
/// `both` puts the label on both sides (the `goto continue` idiom, written in both); else
/// the RemedyLua side gets an empty line, so line numbers stay aligned.
pub fn continue_body(kind: LoopKind, body: Out, label: &str, both: bool, ends_return: bool) -> Out {
    let wrap = match kind {
        LoopKind::While | LoopKind::For => ends_return,
        LoopKind::Repeat => true,
        LoopKind::RepeatScoped => {
            assert!(!ends_return, "a scoped repeat body ending in `return` has no translation here");
            false
        }
    };
    let body = if wrap { c!("do\n", body, "end\n") } else { body };
    if both { c!(body, format!("::{label}::\n")) } else { c!(body, split("\n", format!("::{label}::\n"))) }
}

#[derive(Clone)]
struct LoopCtx {
    /// Label a plain-Lua `continue` jumps to (also used by the `goto` continue pattern).
    label: String,
    /// RemedyLua `continue` used (label needed on the plain side).
    used: bool,
    /// `goto <label>` used on both sides (label needed on both).
    plain: bool,
}

/// What the generated program touched (for the coverage notes).
#[derive(Default, Clone)]
pub struct Coverage {
    pub forms: std::collections::BTreeMap<&'static str, u64>,
}

pub struct Gen<'a> {
    rng: &'a mut Rng,
    pub rl: bool,
    scopes: Vec<Vec<Var>>,
    uniq: u32,
    budget: i32,
    depth: usize,
    vararg: bool,
    loops: Vec<LoopCtx>,
    in_fn: bool,
    pub cov: Coverage,
    /// Deep mode: pushes one construct to extreme nesting.
    max_expr_depth: usize,
    /// Whether the block `block` made last ends in a `return` (which nothing may follow).
    block_returned: bool,
}

impl<'a> Gen<'a> {
    pub fn new(rng: &'a mut Rng, rl: bool) -> Gen<'a> {
        Gen {
            rng,
            rl,
            scopes: vec![],
            uniq: 0,
            budget: 0,
            depth: 0,
            vararg: true,
            loops: vec![],
            in_fn: false,
            cov: Coverage::default(),
            max_expr_depth: 5,
            block_returned: false,
        }
    }

    /// A string from a capability-gated call (`SWEEP1_ALL_LIBS=1`): `io` on the wrapper's
    /// one file `IOF`, `os.setlocale`, `os.getenv` of what both engines have.
    fn gated_call(&mut self, d: usize) -> Out {
        let a = self.expr(Ty::Str, d);
        let f = |r: &mut Rng| *r.pick(&["'n'", "'l'", "'L'", "'a'", "3", "1"]);
        match self.rng.below(6) {
            0 => {
                self.hit("lib:io.write/read");
                c!(
                    "(function() local f = io.open(IOF, 'w') f:write(",
                    a,
                    ", 1, 2.5) f:close() local g = io.open(IOF) local s = g:read('a') g:close() return s end)()"
                )
            }
            1 => {
                self.hit("lib:io.read formats");
                let (f1, f2) = (f(self.rng), f(self.rng));
                c!(
                    "(function() local f = io.open(IOF, 'w+') f:write(",
                    a,
                    ") f:seek('set') local x, y = f:read(",
                    f1,
                    ", ",
                    f2,
                    ") f:close() return tostring(x) .. '|' .. tostring(y) end)()"
                )
            }
            2 => {
                self.hit("lib:io.lines");
                let f1 = f(self.rng);
                c!(
                    "(function() local f = io.open(IOF, 'w') f:write(",
                    a,
                    ") f:close() local t = {} for l in io.lines(IOF, ",
                    f1,
                    ") do t[#t + 1] = tostring(l) end return table.concat(t, ',') end)()"
                )
            }
            3 => {
                self.hit("lib:io.input/output");
                c!(
                    "(function() io.output(IOF) io.write(",
                    a,
                    ") io.close() io.output(io.stdout) io.input(IOF) local s = io.read('L') io.close(io.input()) io.input(io.stdin) return tostring(s) end)()"
                )
            }
            4 => {
                self.hit("lib:os.setlocale");
                let l = *self.rng.pick(&["nil", "'C'", "'POSIX'", "'bogus_XX'"]);
                let k = *self.rng.pick(&["", ", 'all'", ", 'numeric'", ", 'time'", ", 'bogus'"]);
                c!("tostring(os.setlocale(", l, k, "))")
            }
            _ => {
                self.hit("lib:os.getenv");
                c!(
                    "tostring(os.getenv(",
                    self.rng.pick(&["'HOME'", "'PATH'", "'REMEDYLUA_NONE'"]).to_string(),
                    "))"
                )
            }
        }
    }

    fn hit(&mut self, form: &'static str) {
        *self.cov.forms.entry(form).or_default() += 1;
    }

    fn fresh(&mut self, p: &str) -> String {
        self.uniq += 1;
        format!("{p}{}", self.uniq)
    }

    fn declare(&mut self, name: &str, ty: Ty, konst: bool) {
        if let Some(s) = self.scopes.last_mut() {
            s.push(Var { name: name.to_owned(), ty, konst, methods: vec![] });
        }
    }

    fn visible(&self) -> Vec<&Var> {
        let mut seen = std::collections::HashSet::new();
        let mut out = vec![];
        for s in self.scopes.iter().rev() {
            for v in s.iter().rev() {
                if seen.insert(v.name.clone()) {
                    out.push(v);
                }
            }
        }
        out
    }

    fn var_of(&mut self, ty: Ty) -> Option<String> {
        let vs: Vec<String> = self
            .visible()
            .into_iter()
            .filter(|v| ty == Ty::Any || v.ty == ty)
            .map(|v| v.name.clone())
            .collect();
        if vs.is_empty() { None } else { Some(self.rng.pick(&vs).clone()) }
    }

    fn assignable(&mut self, ty: Option<Ty>) -> Option<(String, Ty)> {
        let vs: Vec<(String, Ty)> = self
            .visible()
            .into_iter()
            .filter(|v| !v.konst && ty.is_none_or(|t| t == v.ty))
            .map(|v| (v.name.clone(), v.ty))
            .collect();
        if vs.is_empty() { None } else { Some(self.rng.pick(&vs).clone()) }
    }

    fn set_methods(&mut self, name: &str, m: String) {
        for s in self.scopes.iter_mut().rev() {
            if let Some(v) = s.iter_mut().rev().find(|v| v.name == name) {
                v.methods.push(m);
                return;
            }
        }
    }

    fn method_targets(&self) -> Vec<(String, String)> {
        let mut out = vec![];
        for v in self.visible() {
            for m in &v.methods {
                out.push((v.name.clone(), m.clone()));
            }
        }
        out
    }

    // ---------------------------------------------------------------- literals

    fn int_lit(&mut self) -> String {
        self.hit("numeral:int");
        match self.rng.below(16) {
            0..=6 => self.rng.range(0, 12).to_string(),
            7 => self.rng.range(13, 100000).to_string(),
            8 => format!("0x{:x}", self.rng.range(0, 4096)),
            9 => format!("0X{:X}", self.rng.range(0, 255)),
            10 => self
                .rng
                .pick(&["9223372036854775807", "0x7fffffffffffffff", "0x7FFFFFFFFFFFFFFF"])
                .to_string(),
            11 => self
                .rng
                .pick(&[
                    "0xffffffffffffffff",
                    "0x8000000000000000",
                    "0x10000000000000000",
                    "0xfffffffffffffffff",
                ])
                .to_string(),
            12 => self
                .rng
                .pick(&["math.maxinteger", "math.mininteger", "(-9223372036854775807 - 1)"])
                .to_string(),
            13 => self.rng.pick(&["00", "007", "0x0", "0xa", "0x1F", "1000000000000"]).to_string(),
            14 => self
                .rng
                .pick(&["255", "256", "65535", "2147483647", "4294967296", "4611686018427387904"])
                .to_string(),
            _ => self.rng.range(-3, 3).to_string().replace('-', "- "),
        }
    }

    fn flt_lit(&mut self) -> String {
        self.hit("numeral:float");
        let pool = [
            "1.5",
            ".5",
            "5.",
            "0.0",
            "3.0",
            "1e3",
            "1E+3",
            "2.5e-3",
            "1e-300",
            "1e308",
            "1e309",
            "4.9e-324",
            "0.1",
            "0x1p4",
            "0x.8",
            "0xA.8p1",
            "0x1P-2",
            "0x.1p-1",
            "0X1.8P+1",
            "0xA.",
            "123456789012345678901",
            "9223372036854775808",
            "-0.0",
            "2^53",
            "2^63",
            "1/0",
            "-1/0",
            "0/0",
            "math.pi",
            "math.huge",
            "-math.huge",
            "1e15",
            "1e16",
            "1e100",
            "3.14159",
            "100.25",
            "-2.5",
            "6.02e23",
            "1.7976931348623157e308",
            "2.2250738585072014e-308",
            "0.30000000000000004",
        ];
        if self.rng.pct(40) {
            let a = self.rng.range(0, 99);
            let b = self.rng.range(0, 99);
            return format!("{a}.{b}");
        }
        self.rng.pick(&pool).to_string()
    }

    /// A short string literal with escapes, as Lua source.
    fn str_lit(&mut self) -> String {
        self.hit("string:short");
        let q = if self.rng.pct(50) { '"' } else { '\'' };
        let mut s = String::new();
        s.push(q);
        let n = self.rng.below(8);
        for _ in 0..n {
            match self.rng.below(24) {
                0 => s.push_str("\\n"),
                1 => s.push_str("\\t"),
                2 => s.push_str("\\\\"),
                3 => s.push_str(if q == '"' { "\\\"" } else { "\\'" }),
                4 => s.push_str(&format!("\\x{:02x}", self.rng.range(0, 255))),
                5 => s.push_str(&format!("\\{:03}", self.rng.range(0, 255))),
                6 => s.push_str(&format!(
                    "\\u{{{:X}}}",
                    self.rng.pick(&[0x41i64, 0xE9, 0x20AC, 0x1F600, 0x7FF, 0x10FFFF, 0x7FFFFFFF, 0])
                )),
                7 => s.push_str("\\z  \n   "),
                8 => s.push_str(self.rng.pick(&[
                    "\\a",
                    "\\b",
                    "\\f",
                    "\\v",
                    "\\r",
                    "\\0",
                    "\\x00",
                    "\\065\\066",
                    "\\\n",
                ])),
                9 => s.push_str(
                    self.rng.pick(&["%", "%%", "%d", "%s", "[", "]", "(", ")", ".", "-", "^", "$"]),
                ),
                10 => s.push(if q == '"' { '\'' } else { '"' }),
                11 => s.push_str(self.rng.pick(&["é", "€", "日本", "\u{1F600}"])),
                12 => s.push_str(
                    self.rng.pick(&["0x10", " 12 ", "1e2", "  -3", "5.", "inf", "nan", "0x", "1e"]),
                ),
                _ => {
                    let w =
                        self.rng.pick(&["a", "b", "x", "hello", "Lua", "k", "abc", "A B", " ", "_", "z9"]);
                    s.push_str(w);
                }
            }
        }
        s.push(q);
        s
    }

    fn long_str(&mut self) -> String {
        self.hit("string:long");
        let lvl = self.rng.below(3);
        let eq = "=".repeat(lvl);
        let mut body = String::new();
        if self.rng.pct(40) {
            body.push('\n');
        }
        for _ in 0..self.rng.below(5) {
            body.push_str(
                self.rng.pick(&["abc", "\n", "]", "[[", "\\n", "x]=]y", "%d", "  ", "\t", "\"'", "--", "]]"]),
            );
        }
        // Never close early: drop a closing bracket of our level.
        let close = format!("]{eq}]");
        while body.contains(&close) {
            body = body.replace(&close, "]_]");
        }
        if body.ends_with(']') && lvl == 0 {
            body.push(' ');
        }
        format!("[{eq}[{body}]{eq}]")
    }

    fn pattern(&mut self) -> String {
        self.hit("lib:pattern");
        let mut p = String::new();
        if self.rng.pct(20) {
            p.push('^');
        }
        let n = 1 + self.rng.below(4);
        let mut caps = 0;
        for _ in 0..n {
            let item = match self.rng.below(14) {
                0 => self
                    .rng
                    .pick(&[
                        "%a", "%d", "%s", "%w", "%p", "%l", "%u", "%c", "%x", "%g", "%A", "%D", "%S", "%W",
                    ])
                    .to_string(),
                1 => self
                    .rng
                    .pick(&["[abc]", "[^a-z]", "[%a_]", "[%d%.]", "[]]", "[^]]", "[a-]", "[-a]", "[%]]"])
                    .to_string(),
                2 => ".".into(),
                3 => self.rng.pick(&["a", "b", "l", "x", " ", "1"]).to_string(),
                4 => "%b()".into(),
                5 => self.rng.pick(&["%f[%w]", "%f[%W]", "%f[%a_]"]).to_string(),
                6 => {
                    caps += 1;
                    self.rng.pick(&["(%a+)", "(.)", "(%d*)", "()", "(a-)"]).to_string()
                }
                7 if caps > 0 => "%1".into(),
                8 => self.rng.pick(&["%.", "%%", "%-", "%[", "%("]).to_string(),
                9 => self.rng.pick(&["(", ")", "[a", "%", "%z", "%9", "[%"]).to_string(), // malformed sometimes
                _ => self.rng.pick(&["a", "%a", ".", "%d", "[%w_]"]).to_string(),
            };
            p.push_str(&item);
            if self.rng.pct(40) {
                p.push(*self.rng.pick(&['*', '+', '-', '?']));
            }
        }
        if self.rng.pct(15) {
            p.push('$');
        }
        format!("{p:?}")
    }

    fn fmt_spec(&mut self) -> (String, Vec<Ty>) {
        self.hit("lib:format");
        let mut f = String::new();
        let mut tys = vec![];
        for _ in 0..1 + self.rng.below(3) {
            if self.rng.pct(40) {
                f.push_str(self.rng.pick(&["x", "<", " ", "=", "%%"]));
            }
            f.push('%');
            for _ in 0..self.rng.below(3) {
                f.push(*self.rng.pick(&['-', '+', ' ', '#', '0']));
            }
            if self.rng.pct(40) {
                f.push_str(&self.rng.range(0, 30).to_string());
            }
            if self.rng.pct(30) {
                f.push('.');
                f.push_str(&self.rng.range(0, 20).to_string());
            }
            let (c, t) = *self.rng.pick(&[
                ('d', Ty::Int),
                ('i', Ty::Int),
                ('x', Ty::Int),
                ('X', Ty::Int),
                ('o', Ty::Int),
                ('u', Ty::Int),
                ('c', Ty::Int),
                ('e', Ty::Flt),
                ('E', Ty::Flt),
                ('f', Ty::Flt),
                ('F', Ty::Flt),
                ('g', Ty::Flt),
                ('G', Ty::Flt),
                ('s', Ty::Str),
                ('s', Ty::Int),
                ('d', Ty::Flt),
                ('s', Ty::Any),
            ]);
            f.push(c);
            tys.push(t);
        }
        if self.rng.pct(5) {
            f.push_str(self.rng.pick(&["%", "%y", "%10", "%123d", "%.123f"]));
        }
        (format!("{f:?}"), tys)
    }

    // ---------------------------------------------------------------- expressions

    pub fn expr(&mut self, ty: Ty, d: usize) -> Out {
        let leaf = d >= self.max_expr_depth || self.rng.pct(30);
        match ty {
            Ty::Int => self.int_expr(d, leaf),
            Ty::Flt => self.flt_expr(d, leaf),
            Ty::Str => self.str_expr(d, leaf),
            Ty::Bool => self.bool_expr(d, leaf),
            Ty::Nil => {
                if let (false, Some(v)) = (leaf, self.var_of(Ty::Nil)) {
                    c!(v)
                } else {
                    c!("nil")
                }
            }
            Ty::Tab => self.tab_expr(d, leaf),
            Ty::Fun => self.fun_expr(d, leaf),
            Ty::Any => self.any_expr(d, leaf),
        }
    }

    fn num_expr(&mut self, d: usize) -> Out {
        if self.rng.pct(60) { self.expr(Ty::Int, d) } else { self.expr(Ty::Flt, d) }
    }

    fn paren(&mut self, e: Out) -> Out {
        c!("(", e, ")")
    }

    /// `e` as an operand: parenthesized unless we're testing precedence.
    fn opnd(&mut self, e: Out) -> Out {
        if self.rng.pct(25) { e } else { self.paren(e) }
    }

    fn unop(&mut self, op: &str, e: Out) -> Out {
        // `- -x` must not become a comment.
        let sp = if op == "-" && (e.l.starts_with('-')) { " " } else { "" };
        let e = if self.rng.pct(70) { self.paren(e) } else { e };
        c!(op, sp, e)
    }

    fn binop(&mut self, a: Out, op: &str, b: Out) -> Out {
        self.hit(match op {
            "+" => "op:+",
            "-" => "op:-",
            "*" => "op:*",
            "/" => "op:/",
            "//" => "op://",
            "%" => "op:%",
            "^" => "op:^",
            ".." => "op:..",
            "&" => "op:&",
            "|" => "op:|",
            "~" => "op:~",
            "<<" => "op:<<",
            ">>" => "op:>>",
            "==" => "op:==",
            "~=" => "op:~=",
            "<" => "op:<",
            "<=" => "op:<=",
            ">" => "op:>",
            ">=" => "op:>=",
            "and" => "op:and",
            "or" => "op:or",
            _ => "op:other",
        });
        let a = self.opnd(a);
        let b = self.opnd(b);
        c!(a, " ", op, " ", b)
    }

    fn int_expr(&mut self, d: usize, leaf: bool) -> Out {
        if leaf {
            if self.rng.pct(50)
                && let Some(v) = self.var_of(Ty::Int)
            {
                return c!(v);
            }
            return c!(self.int_lit());
        }
        let d = d + 1;
        match self.rng.below(20) {
            0..=3 => {
                let op = *self.rng.pick(&["+", "-", "*", "&", "|", "~", "<<", ">>"]);
                let a = self.expr(Ty::Int, d);
                let b = self.expr(Ty::Int, d);
                self.binop(a, op, b)
            }
            4 => {
                let op = *self.rng.pick(&["//", "%"]);
                let a = self.expr(Ty::Int, d);
                let b = if self.rng.pct(90) {
                    c!(self.rng.pick(&["3", "7", "-2", "1", "-1", "16"]).to_string())
                } else {
                    self.expr(Ty::Int, d)
                };
                self.binop(a, op, b)
            }
            5 => {
                let op = *self.rng.pick(&["-", "~"]);
                let e = self.expr(Ty::Int, d);
                self.hit(if op == "-" { "op:unary-" } else { "op:unary~" });
                self.unop(op, e)
            }
            6 => {
                self.hit("op:#");
                let s = self.expr(Ty::Str, d);
                self.unop("#", s)
            }
            7 => {
                self.hit("op:#table");
                let t = self.seq_table(d);
                c!("#", t)
            }
            8 if self.vararg => {
                self.hit("vararg:select#");
                c!("select('#', ...)")
            }
            9 => {
                self.hit("lib:string.len");
                let s = self.expr(Ty::Str, d);
                let s = self.paren(s);
                if self.rng.pct(50) { c!(s, ":len()") } else { c!("string.len(", s, ")") }
            }
            10 => {
                let f = *self.rng.pick(&["math.max", "math.min"]);
                self.hit("lib:math.max/min");
                let a = self.expr(Ty::Int, d);
                let b = self.expr(Ty::Int, d);
                c!(f, "(", a, ", ", b, ")")
            }
            11 => {
                self.hit("lib:math.abs/tointeger/floor");
                let f = *self.rng.pick(&["math.abs", "math.floor", "math.ceil"]);
                let a = self.expr(Ty::Int, d);
                c!(f, "(", a, ")")
            }
            12 => {
                self.hit("lib:string.byte");
                let s = self.expr(Ty::Str, d);
                let s = self.paren(s);
                let i = self.rng.range(-3, 4);
                c!("(", s, ":byte(", i.to_string(), ") or 0)")
            }
            13 => {
                self.hit("lib:tonumber");
                let s = *self.rng.pick(&[
                    "\"10\"",
                    "\"0x1F\"",
                    "\" 42 \"",
                    "\"7\", 8",
                    "\"zz\", 36",
                    "\"-0x10\"",
                    "\"1e2\"",
                    "\"10\", 2",
                ]);
                c!("(math.tointeger(tonumber(", s, ")) or -1)")
            }
            14 => {
                self.hit("lib:string.find");
                let s = self.expr(Ty::Str, d);
                let p = self.pattern();
                c!("(string.find(", s, ", ", p, ") or 0)")
            }
            15 => {
                self.hit("lib:utf8.len");
                let s = self.expr(Ty::Str, d);
                c!("(utf8.len(", s, ") or -1)")
            }
            16 => {
                self.hit("lib:math.tointeger");
                let f = self.expr(Ty::Flt, d);
                c!("(math.tointeger(", f, ") or 0)")
            }
            17 => {
                self.hit("op:coercion-int");
                let s = *self.rng.pick(&["\"10\"", "\"0x10\"", "\" 3 \""]);
                let e = self.expr(Ty::Int, d);
                c!("(", s, " | 0) + ", self.paren(e))
            }
            _ => {
                if let Some(v) = self.var_of(Ty::Int) {
                    c!(v)
                } else {
                    c!(self.int_lit())
                }
            }
        }
    }

    fn flt_expr(&mut self, d: usize, leaf: bool) -> Out {
        if leaf {
            if self.rng.pct(50)
                && let Some(v) = self.var_of(Ty::Flt)
            {
                return c!(v);
            }
            return c!(self.flt_lit());
        }
        let d = d + 1;
        match self.rng.below(12) {
            0..=2 => {
                let op = *self.rng.pick(&["+", "-", "*", "/", "^", "//", "%"]);
                let a = self.num_expr(d);
                let b = self.expr(Ty::Flt, d);
                self.binop(a, op, b)
            }
            3 => {
                let a = self.expr(Ty::Int, d);
                let b = self.expr(Ty::Int, d);
                self.binop(a, "/", b)
            }
            4 => {
                self.hit("op:unary-");
                let e = self.expr(Ty::Flt, d);
                self.unop("-", e)
            }
            5 => {
                let f = *self.rng.pick(&[
                    "math.sqrt",
                    "math.abs",
                    "math.exp",
                    "math.log",
                    "math.sin",
                    "math.cos",
                    "math.atan",
                    "math.floor",
                    "math.ceil",
                ]);
                self.hit("lib:math.fn");
                let a = self.num_expr(d);
                c!(f, "(", a, ") + 0.0")
            }
            6 => {
                self.hit("lib:math.fmod/modf");
                let a = self.num_expr(d);
                let b = self.num_expr(d);
                if self.rng.pct(50) {
                    c!("math.fmod(", a, ", ", b, ") + 0.0")
                } else {
                    c!("(math.modf(", a, "))")
                }
            }
            7 => {
                self.hit("lib:math.log2");
                let a = self.num_expr(d);
                let b = c!(self.rng.pick(&["2", "10", "0.5", "1"]).to_string());
                c!("math.log(", a, ", ", b, ")")
            }
            8 => {
                self.hit("op:coercion-float");
                let s = *self.rng.pick(&["\"1.5\"", "\"1e2\"", "\"0x.8\"", "\" .5 \"", "\"0x1p-2\""]);
                let e = self.num_expr(d);
                c!(s, " * ", self.paren(e))
            }
            9 => {
                self.hit("lib:tonumber");
                let s = *self.rng.pick(&[
                    "\"1.25\"",
                    "\"1e-2\"",
                    "\"0x1.8p1\"",
                    "\"inf\"",
                    "\"  2.  \"",
                    "\".\"",
                    "\"1e\"",
                ]);
                c!("(tonumber(", s, ") or 0.5) + 0.0")
            }
            10 => {
                self.hit("lib:math.ult/maxmin");
                let a = self.expr(Ty::Flt, d);
                let b = self.num_expr(d);
                c!("math.max(", a, ", ", b, ") + 0.0")
            }
            _ => {
                let a = self.expr(Ty::Int, d);
                c!("(", a, ") + 0.0")
            }
        }
    }

    fn str_expr(&mut self, d: usize, leaf: bool) -> Out {
        if leaf {
            if self.rng.pct(40)
                && let Some(v) = self.var_of(Ty::Str)
            {
                return c!(v);
            }
            return if self.rng.pct(15) { c!(self.long_str()) } else { c!(self.str_lit()) };
        }
        let d = d + 1;
        if std::env::var_os("SWEEP1_ALL_LIBS").is_some() && self.rng.pct(6) {
            return self.gated_call(d);
        }
        match self.rng.below(22) {
            0..=2 => {
                let a = if self.rng.pct(70) { self.expr(Ty::Str, d) } else { self.num_expr(d) };
                let b = if self.rng.pct(70) { self.expr(Ty::Str, d) } else { self.num_expr(d) };
                self.binop(a, "..", b)
            }
            3 => {
                self.hit("lib:string.rep");
                let s = self.expr(Ty::Str, d);
                let n = self.rng.range(-1, 4);
                if self.rng.pct(50) {
                    c!("string.rep(", s, ", ", n.to_string(), ")")
                } else {
                    c!("string.rep(", s, ", ", n.to_string(), ", \",\")")
                }
            }
            4 => {
                let m = *self.rng.pick(&["upper", "lower", "reverse"]);
                self.hit("lib:string.upper/lower/reverse");
                let s = self.expr(Ty::Str, d);
                let s = self.paren(s);
                if self.rng.pct(50) { c!(s, ":", m, "()") } else { c!("string.", m, "(", s, ")") }
            }
            5 => {
                self.hit("lib:string.sub");
                let s = self.expr(Ty::Str, d);
                let s = self.paren(s);
                let i = self.rng.range(-5, 5);
                if self.rng.pct(50) {
                    let j = self.rng.range(-5, 8);
                    c!(s, ":sub(", i.to_string(), ", ", j.to_string(), ")")
                } else {
                    let j = self.int_lit();
                    c!("string.sub(", s, ", ", i.to_string(), ", ", j, ")")
                }
            }
            6 => {
                let (f, tys) = self.fmt_spec();
                let mut args = vec![c!(f)];
                for t in tys {
                    args.push(if t == Ty::Any { self.prim_expr(d) } else { self.expr(t, d) });
                }
                c!("string.format(", join(&args, ", "), ")")
            }
            7 => {
                self.hit("lib:tostring");
                let e = self.prim_expr(d);
                c!("tostring(", e, ")")
            }
            8 => {
                self.hit("lib:string.char");
                let n = 1 + self.rng.below(3);
                let cs: Vec<Out> = (0..n).map(|_| c!(self.rng.range(0, 255).to_string())).collect();
                c!("string.char(", join(&cs, ", "), ")")
            }
            9 => {
                self.hit("lib:table.concat");
                let t = self.seq_table(d);
                let sep = self.str_lit();
                c!("table.concat(", t, ", ", sep, ")")
            }
            10 => {
                self.hit("lib:string.gsub");
                let s = self.expr(Ty::Str, d);
                let p = self.pattern();
                let repl = match self.rng.below(4) {
                    0 => {
                        c!(self.rng.pick(&["\"%0%0\"", "\"<%1>\"", "\"x\"", "\"%%\"", "\"%2\""]).to_string())
                    }
                    1 => c!("{a = \"A\", [\"1\"] = 1, b = false}"),
                    2 => c!("function(c) return c .. \"!\" end"),
                    _ => c!("string.upper"),
                };
                let n = if self.rng.pct(30) { format!(", {}", self.rng.range(0, 3)) } else { String::new() };
                c!("(string.gsub(", s, ", ", p, ", ", repl, n, "))")
            }
            11 => {
                self.hit("lib:string.match");
                let s = self.expr(Ty::Str, d);
                let p = self.pattern();
                c!("tostring((string.match(", s, ", ", p, ")))")
            }
            12 => {
                self.hit("lib:utf8.char");
                let cs = *self.rng.pick(&["72, 228, 8364", "0x10FFFF", "0x7FFFFFFF", "65", "0"]);
                c!("utf8.char(", cs, ")")
            }
            13 => {
                self.hit("lib:string.pack");
                let (f, v) = *self.rng.pick(&[
                    ("\"<i4\"", "100"),
                    ("\">I2\"", "513"),
                    ("\"z\"", "\"ab\""),
                    ("\"s1\"", "\"hey\""),
                    ("\"<d\"", "1.5"),
                    ("\"b\"", "-1"),
                    ("\"<j\"", "math.mininteger"),
                    ("\">!4 i3\"", "-5"),
                    ("\"<f\"", "0.1"),
                    ("\"=I8\"", "-1"),
                ]);
                c!("string.pack(", f, ", ", v, ")")
            }
            14 if self.rl => {
                self.hit("rlua:interpolation");
                self.interp(d)
            }
            15 => {
                self.hit("lib:os.date");
                let f = *self.rng.pick(&[
                    "\"!%Y-%m-%d %H:%M:%S\"",
                    "\"!%j %a %b %p\"",
                    "\"!%c\"",
                    "\"!%x %X %y\"",
                ]);
                let t = self.rng.range(0, 4_000_000_000);
                c!("os.date(", f, ", ", t.to_string(), ")")
            }
            16 => {
                self.hit("lib:string.format%q");
                let s = self.str_lit();
                let i = self.int_lit();
                c!("string.format(\"%q %q\", ", s, ", ", i, ")")
            }
            17 => {
                self.hit("call:string-literal-arg");
                c!("string.upper", self.str_lit())
            }
            18 => {
                self.hit("op:concat-chain");
                let n = 3 + self.rng.below(4);
                let xs: Vec<Out> = (0..n)
                    .map(|_| if self.rng.pct(70) { self.expr(Ty::Str, d) } else { self.expr(Ty::Int, d) })
                    .map(|e| c!("(", e, ")"))
                    .collect();
                join(&xs, " .. ")
            }
            19 => {
                self.hit("lib:string.format%s-table");
                let s = self.expr(Ty::Str, d);
                c!("string.format(\"%5.2s|%-6s|%s\", ", s, ", \"ab\", true)")
            }
            _ => {
                if let Some(v) = self.var_of(Ty::Str) {
                    c!(v)
                } else {
                    c!(self.str_lit())
                }
            }
        }
    }

    /// A RemedyLua interpolated string and its plain-Lua equivalent.
    fn interp(&mut self, d: usize) -> Out {
        let mut r = String::from("`");
        let mut parts: Vec<String> = vec![];
        for _ in 0..1 + self.rng.below(4) {
            if self.rng.pct(50) {
                let (rr, ll) = *self.rng.pick(&[
                    ("a", "a"),
                    ("x y", "x y"),
                    ("\\n", "\\n"),
                    ("\\{", "{"),
                    ("\\}", "}"),
                    ("\"", "\\\""),
                    ("'", "'"),
                    ("\\`", "`"),
                    ("\\\\", "\\\\"),
                    ("%d", "%d"),
                    ("é", "é"),
                    ("\\x41", "\\x41"),
                ]);
                r.push_str(rr);
                parts.push(format!("\"{ll}\""));
            } else {
                let e = self.prim_expr(d + 1);
                // The text between braces is lexed separately; it is the plain expression.
                r.push('{');
                r.push_str(&e.r);
                r.push('}');
                parts.push(format!("tostring({})", e.l));
            }
        }
        r.push('`');
        let l = if parts.is_empty() { "\"\"".to_owned() } else { format!("({})", parts.join(" .. ")) };
        split(r, l)
    }

    fn bool_expr(&mut self, d: usize, leaf: bool) -> Out {
        if leaf {
            if self.rng.pct(40)
                && let Some(v) = self.var_of(Ty::Bool)
            {
                return c!(v);
            }
            return c!(self.rng.pick(&["true", "false"]).to_string());
        }
        let d = d + 1;
        match self.rng.below(10) {
            0..=2 => {
                let op = *self.rng.pick(&["<", "<=", ">", ">=", "==", "~="]);
                let (a, b) = match self.rng.below(4) {
                    0 => (self.expr(Ty::Str, d), self.expr(Ty::Str, d)),
                    1 => (self.expr(Ty::Int, d), self.expr(Ty::Flt, d)),
                    _ => (self.num_expr(d), self.num_expr(d)),
                };
                self.binop(a, op, b)
            }
            3 => {
                let op = *self.rng.pick(&["==", "~="]);
                let a = self.any_expr(d, true);
                let b = self.any_expr(d, true);
                self.binop(a, op, b)
            }
            4 => {
                self.hit("op:not");
                let e = self.any_expr(d, false);
                self.unop("not ", e)
            }
            5 => {
                let op = *self.rng.pick(&["and", "or"]);
                let a = self.expr(Ty::Bool, d);
                let b = self.expr(Ty::Bool, d);
                self.binop(a, op, b)
            }
            6 => {
                self.hit("lib:rawequal");
                let a = self.any_expr(d, true);
                let b = self.any_expr(d, true);
                c!("rawequal(", a, ", ", b, ")")
            }
            7 => {
                self.hit("lib:math.ult");
                let a = self.expr(Ty::Int, d);
                let b = self.expr(Ty::Int, d);
                c!("math.ult(", a, ", ", b, ")")
            }
            8 => {
                self.hit("lib:string.find-plain");
                let s = self.expr(Ty::Str, d);
                let p = self.str_lit();
                c!("(string.find(", s, ", ", p, ", 1, true) ~= nil)")
            }
            _ => {
                self.hit("lib:coroutine.isyieldable");
                c!("coroutine.isyieldable()")
            }
        }
    }

    /// A table literal that is a proper sequence (so `#` is well defined).
    fn seq_table(&mut self, d: usize) -> Out {
        let n = self.rng.below(5);
        let ty = if self.rng.pct(50) { Ty::Int } else { Ty::Str };
        let xs: Vec<Out> = (0..n).map(|_| self.expr(ty, d + 1)).collect();
        c!("{", join(&xs, ", "), "}")
    }

    fn table_ctor(&mut self, d: usize) -> Out {
        self.hit("expr:table-constructor");
        let n = self.rng.below(6);
        let mut xs = vec![];
        for i in 0..n {
            let f = match self.rng.below(7) {
                0..=2 => self.expr(Ty::Any, d + 1),
                3 => {
                    self.hit("table:name=");
                    let k = self.rng.pick(&["x", "y", "v", "name", "n", "end_"]).to_string();
                    c!(k, " = ", self.expr(Ty::Any, d + 1))
                }
                4 => {
                    self.hit("table:[k]=");
                    let k = match self.rng.below(5) {
                        0 => c!(self.rng.range(-2, 6).to_string()),
                        1 => c!(self.rng.pick(&["1.0", "2.0", "2^53", "0.5", "-0.0"]).to_string()),
                        2 => c!(self.str_lit()),
                        3 => c!(self.rng.pick(&["true", "false"]).to_string()),
                        _ => self.expr(Ty::Int, d + 1),
                    };
                    c!(index(k), " = ", self.expr(Ty::Any, d + 1))
                }
                5 if i + 1 == n => {
                    self.hit("table:multret-last");
                    if self.vararg && self.rng.pct(50) {
                        c!("...")
                    } else {
                        c!("string.byte(\"abc\", 1, -1)")
                    }
                }
                _ => self.expr(Ty::Int, d + 1),
            };
            xs.push(f);
        }
        let mut o = c!("{");
        for (i, x) in xs.iter().enumerate() {
            if i > 0 {
                o = c!(o, self.rng.pick(&[", ", "; ", ","]).to_string());
            }
            o = c!(o, x);
        }
        if !xs.is_empty() && self.rng.pct(20) {
            self.hit("table:trailing-sep");
            o = c!(o, self.rng.pick(&[",", ";"]).to_string());
        }
        c!(o, "}")
    }

    fn tab_expr(&mut self, d: usize, leaf: bool) -> Out {
        if leaf {
            if self.rng.pct(60)
                && let Some(v) = self.var_of(Ty::Tab)
            {
                return c!(v);
            }
            return c!("{}");
        }
        match self.rng.below(7) {
            0..=2 => self.table_ctor(d),
            3 => self.seq_table(d),
            4 => {
                self.hit("lib:table.pack");
                let args = self.args(d + 1);
                c!("table.pack(", args, ")")
            }
            5 if self.vararg => {
                self.hit("vararg:{...}");
                c!("{...}")
            }
            _ => {
                self.hit("lib:setmetatable");
                let mt = self.metatable(d + 1);
                let v = self.int_lit();
                c!("setmetatable({v = ", v, "}, ", mt, ")")
            }
        }
    }

    fn metatable(&mut self, d: usize) -> Out {
        let mut fields: Vec<Out> = vec![];
        let names = [
            "__index",
            "__newindex",
            "__call",
            "__add",
            "__sub",
            "__mul",
            "__div",
            "__mod",
            "__pow",
            "__unm",
            "__idiv",
            "__band",
            "__bor",
            "__bxor",
            "__shl",
            "__shr",
            "__bnot",
            "__concat",
            "__len",
            "__eq",
            "__lt",
            "__le",
            "__tostring",
            "__name",
            "__close",
            "__metatable",
        ];
        for _ in 0..1 + self.rng.below(5) {
            let n = *self.rng.pick(&names);
            self.hit("metamethod");
            let v: Out = match n {
                "__index" => match self.rng.below(3) {
                    0 => c!("{x = 1, y = \"why\", v = 0}"),
                    1 => c!("function(t, k) if __enter() then return end return k end"),
                    _ => c!("setmetatable({}, {__index = function(t, k) return \"deep\" end})"),
                },
                "__newindex" => {
                    if self.rng.pct(50) {
                        c!(
                            "function(t, k, v) if __enter() then return end rawset(t, k, v) __emit(\"newindex\", k) end"
                        )
                    } else {
                        c!("{}")
                    }
                }
                "__call" => {
                    c!("function(self, a, b) if __enter() then return end return __tag(self), a, b end")
                }
                "__tostring" => c!(self
                    .rng
                    .pick(&[
                        "function(t) return \"T\" .. tostring(rawget(t, \"v\")) end",
                        "function() return 42 end"
                    ])
                    .to_string()),
                "__name" => c!("\"MyType\""),
                "__metatable" => c!("\"locked\""),
                "__len" => c!(self
                    .rng
                    .pick(&[
                        "function() return 7 end",
                        "function() return \"len\" end",
                        "function() return 2.5 end"
                    ])
                    .to_string()),
                "__eq" | "__lt" | "__le" => c!(self
                    .rng
                    .pick(&[
                        "function(a, b) return __tag(a) == __tag(b) end",
                        "function() return 1 end",
                        "function() return nil end"
                    ])
                    .to_string()),
                "__close" => c!("function(o, e) __emit(\"close-mt\", __tag(o), e) end"),
                "__unm" | "__bnot" => c!("function(a) if __enter() then return end return __tag(a) end"),
                _ => {
                    let body = self.rng.pick(&["__tag(a)", "__tag(b)", "\"r\"", "7", "nil"]).to_string();
                    c!("function(a, b) if __enter() then return end return ", body, " end")
                }
            };
            let _ = d;
            fields.push(c!(n, " = ", v));
        }
        c!("{", join(&fields, ", "), "}")
    }

    fn fun_expr(&mut self, d: usize, leaf: bool) -> Out {
        if leaf || self.budget <= 0 {
            if self.rng.pct(60)
                && let Some(v) = self.var_of(Ty::Fun)
            {
                return c!(v);
            }
            self.hit("expr:library-function-value");
            return c!(self
                .rng
                .pick(&[
                    "string.upper",
                    "math.max",
                    "tostring",
                    "select",
                    "type",
                    "string.len",
                    "print",
                    "rawlen",
                    "next"
                ])
                .to_string());
        }
        self.func_literal(d)
    }

    /// `function(params) body end`.
    fn func_literal(&mut self, _d: usize) -> Out {
        self.hit("expr:function");
        let (params, vararg) = self.params();
        let body = {
            let nn = 2 + self.rng.below(3);
            self.func_body(&params, vararg, nn)
        };
        let (pr, pl) = self.param_text(&params, vararg);
        let (ret_r, ret_l) = self.ret_annot();
        c!(split(format!("function({pr}){ret_r}"), format!("function({pl}){ret_l}")), "\n", body, "end")
    }

    fn params(&mut self) -> (Vec<(String, Ty)>, bool) {
        let n = self.rng.below(4);
        let ps = (0..n)
            .map(|_| (self.fresh("p"), *self.rng.pick(&[Ty::Any, Ty::Int, Ty::Str, Ty::Any])))
            .collect();
        (ps, self.rng.pct(40))
    }

    fn annot(&mut self, ty: Ty) -> String {
        if !self.rl || self.rng.pct(50) {
            return String::new();
        }
        self.hit("rlua:annotation");
        let t = match ty {
            Ty::Int => *self.rng.pick(&["number", "integer", "number?", "any"]),
            Ty::Flt => "number",
            Ty::Str => *self.rng.pick(&["string", "string | nil", "\"lit\" | string"]),
            Ty::Bool => "boolean",
            Ty::Nil => "nil",
            Ty::Tab => {
                *self.rng.pick(&["{number}", "{x: number, y: string?}", "{[string]: number}", "table"])
            }
            Ty::Fun => *self.rng.pick(&["(number, string) -> boolean", "() -> ()", "function"]),
            Ty::Any => *self.rng.pick(&["any", "unknown", "number | string", "Foo.Bar"]),
        };
        format!(": {t}")
    }

    fn param_text(&mut self, ps: &[(String, Ty)], vararg: bool) -> (String, String) {
        let mut r = vec![];
        let mut l = vec![];
        for (p, t) in ps {
            r.push(format!("{p}{}", self.annot(*t)));
            l.push(p.clone());
        }
        if vararg {
            let a = if self.rl && self.rng.pct(30) { ": number" } else { "" };
            r.push(format!("...{a}"));
            l.push("...".into());
        }
        (r.join(", "), l.join(", "))
    }

    fn ret_annot(&mut self) -> (String, String) {
        if self.rl && self.rng.pct(30) {
            self.hit("rlua:return-annotation");
            let t = *self.rng.pick(&[": number", ": (number, string)", ": ()", ": string?"]);
            (t.to_owned(), String::new())
        } else {
            (String::new(), String::new())
        }
    }

    /// A function body: the call-budget guard, statements, maybe a return. Ends with "\n".
    fn func_body(&mut self, params: &[(String, Ty)], vararg: bool, n: usize) -> Out {
        let saved = (std::mem::take(&mut self.loops), self.vararg, self.in_fn);
        self.vararg = vararg;
        self.in_fn = true;
        self.scopes.push(vec![]);
        for (p, t) in params {
            self.declare(p, *t, false);
        }
        let mut o = c!("if __enter() then return end\n");
        for _ in 0..n {
            let s = self.stat();
            o = c!(o, s);
        }
        if self.rng.pct(70) {
            let r = self.return_stat();
            o = c!(o, r);
        }
        self.scopes.pop();
        (self.loops, self.vararg, self.in_fn) = saved;
        o
    }

    fn args(&mut self, d: usize) -> Out {
        let n = self.rng.below(4);
        let mut xs: Vec<Out> = (0..n).map(|_| self.expr(Ty::Any, d + 1)).collect();
        if self.rng.pct(20) {
            self.hit("call:multret-last-arg");
            xs.push(if self.vararg && self.rng.pct(50) {
                c!("...")
            } else {
                c!("string.byte(\"xyz\", 1, 3)")
            });
        }
        join(&xs, ", ")
    }

    /// A value of a printable primitive type (never a table or function: their text has
    /// addresses).
    fn prim_expr(&mut self, d: usize) -> Out {
        let t = *self.rng.pick(&[Ty::Int, Ty::Flt, Ty::Str, Ty::Bool, Ty::Nil, Ty::Str]);
        self.expr(t, d)
    }

    fn any_expr(&mut self, d: usize, leaf: bool) -> Out {
        if leaf {
            if self.rng.pct(50)
                && let Some(v) = self.var_of(Ty::Any)
            {
                return c!(v);
            }
            let t = *self.rng.pick(&CONCRETE);
            return self.expr(t, self.max_expr_depth);
        }
        let d = d + 1;
        match self.rng.below(16) {
            0..=4 => {
                let t = *self.rng.pick(&CONCRETE);
                self.expr(t, d)
            }
            5 | 6 => {
                // Call a function value.
                self.hit("call:function-value");
                let f = match self.var_of(Ty::Fun) {
                    Some(f) => c!(f),
                    None => {
                        let f = self.fun_expr(d, true);
                        self.paren(f)
                    }
                };
                let a = self.args(d);
                let call = match self.rng.below(8) {
                    0 => {
                        self.hit("call:string-arg");
                        c!(f, " ", self.str_lit())
                    }
                    1 => {
                        self.hit("call:table-arg");
                        c!(f, self.seq_table(d))
                    }
                    _ => c!(f, "(", a, ")"),
                };
                if self.rng.pct(50) { self.paren(call) } else { call }
            }
            7 => {
                self.hit("expr:index");
                let t = self.expr(Ty::Tab, d);
                let t = self.paren(t);
                match self.rng.below(4) {
                    0 => c!(t, ".", self.rng.pick(&["x", "y", "v", "n", "name"]).to_string()),
                    1 => c!(t, "[", self.rng.range(-1, 4).to_string(), "]"),
                    2 => c!(t, index(self.str_lit())),
                    _ => c!(t, index(self.expr(Ty::Any, d))),
                }
            }
            8 => {
                let ms = self.method_targets();
                if ms.is_empty() {
                    return self.expr(Ty::Int, d);
                }
                self.hit("call:method");
                let (o, m) = self.rng.pick(&ms).clone();
                let a = self.args(d);
                c!(o, ":", m, "(", a, ")")
            }
            9 if self.vararg => {
                self.hit("vararg:expr");
                c!(self
                    .rng
                    .pick(&[
                        "(...)",
                        "select(2, ...)",
                        "(select(-1, ...))",
                        "select('#', ...)",
                        "...",
                        "(select(1, ...))"
                    ])
                    .to_string())
            }
            10 => {
                self.hit("op:and-or");
                let a = self.expr(Ty::Bool, d);
                let b = self.prim_expr(d);
                let c2 = self.prim_expr(d);
                c!(a, " and ", self.paren(b), " or ", self.paren(c2))
            }
            11 => {
                self.hit("lib:select");
                let i = *self.rng.pick(&["1", "2", "-1", "-2", "3", "'#'"]);
                let a = self.args(d);
                c!("select(", i, if a.l.is_empty() { "" } else { ", " }, a, ")")
            }
            12 => {
                self.hit("lib:type/math.type");
                let e = self.expr(Ty::Any, d);
                if self.rng.pct(50) { c!("type(", e, ")") } else { c!("math.type(", e, ")") }
            }
            13 => {
                self.hit("lib:load");
                let code = *self.rng.pick(&[
                    "\"return 1 + 1\"",
                    "\"return ...\"",
                    "\"x = \"",
                    "\"return 'a' .. \"",
                    "\"local a <const> = 1; a = 2\"",
                    "\"goto nowhere\"",
                    "\"for i = 1, 2 do break break end return 'b'\"",
                    "\"return 0x\"",
                    "\"return 1e\"",
                    "\"return \\\"unfinished\"",
                    "\"::a:: ::a::\"",
                    "\"return [==[ x ]=]\"",
                    "\"local x <foo> = 1\"",
                    "\"return function(...) local a, b = ... return b end\"",
                    "\"break\"",
                    "\"return 1 .. 2\"",
                    "\"return 3 // 0\"",
                    "\"return 2^63 == math.maxinteger + 1.0\"",
                ]);
                let chunk = if self.rng.pct(30) { ", \"=chunk\"" } else { "" };
                c!(
                    "select(2, pcall(function() local f, e = load(",
                    code,
                    chunk,
                    ") if not f then return e end return f(5, 6) end))"
                )
            }
            14 => {
                self.hit("expr:paren-truncate");
                let e = self.expr(Ty::Any, d);
                self.paren(e)
            }
            _ => {
                if let Some(v) = self.var_of(Ty::Any) {
                    c!(v)
                } else {
                    c!("nil")
                }
            }
        }
    }

    // ---------------------------------------------------------------- statements

    fn block(&mut self, n: usize) -> Out {
        self.scopes.push(vec![]);
        self.depth += 1;
        let mut o = Out::default();
        for _ in 0..n {
            let s = self.stat();
            o = c!(o, s);
        }
        let returned = self.rng.pct(4);
        if returned {
            let r = self.return_stat();
            o = c!(o, r);
        }
        self.block_returned = returned;
        self.depth -= 1;
        self.scopes.pop();
        o
    }

    fn return_stat(&mut self) -> Out {
        self.hit("stat:return");
        let o = match self.rng.below(6) {
            0 => c!("return"),
            1 if self.vararg => {
                self.hit("vararg:return...");
                c!("return ...")
            }
            2 => {
                self.hit("stat:tail-call");
                let f = self.fun_expr(1, true);
                let a = self.args(1);
                c!("return (", f, ")(", a, ")")
            }
            _ => {
                let n = 1 + self.rng.below(3);
                let xs: Vec<Out> = (0..n).map(|_| self.expr(Ty::Any, 1)).collect();
                c!("return ", join(&xs, ", "))
            }
        };
        c!(o, if self.rng.pct(20) { ";" } else { "" }, "\n")
    }

    pub fn stat(&mut self) -> Out {
        self.budget -= 1;
        let deep = self.depth > 5 || self.budget <= 0;
        let w: [u32; 24] = if deep {
            [10, 6, 4, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 3, 2, 0, 0, 0]
        } else {
            [10, 8, 5, 10, 4, 5, 3, 3, 4, 4, 3, 3, 2, 3, 3, 3, 3, 2, 2, 3, 3, 2, 2, 2]
        };
        let o = match self.rng.weighted(&w) {
            0 => self.local_stat(),
            1 => self.assign_stat(),
            2 => self.call_stat(),
            3 => self.emit_stat(),
            4 => {
                self.hit("stat:do");
                let b = {
                    let nn = 1 + self.rng.below(3);
                    self.block(nn)
                };
                c!("do\n", b, "end\n")
            }
            5 => self.if_stat(),
            6 => self.while_stat(),
            7 => self.repeat_stat(),
            8 => self.numfor_stat(),
            9 => self.genfor_stat(),
            10 => self.local_function(),
            11 => self.function_stat(),
            12 => self.goto_stat(),
            13 => self.pcall_stat(),
            14 => self.close_stat(),
            15 => self.lib_stat(),
            16 => self.coroutine_stat(),
            17 => self.meta_stat(),
            18 => self.method_object(),
            19 if self.rl => self.compound_stat(),
            20 if !self.loops.is_empty() => self.loop_exit_stat(),
            21 => self.error_stat(),
            22 => self.vararg_stat(),
            23 => self.multi_assign_stat(),
            _ => self.emit_stat(),
        };
        if self.rng.pct(3) {
            self.hit("stat:empty;");
            return c!(";", o);
        }
        o
    }

    fn local_stat(&mut self) -> Out {
        self.hit("stat:local");
        let n = 1 + self.rng.below(3);
        let tys: Vec<Ty> = (0..n).map(|_| *self.rng.pick(&CONCRETE)).collect();
        let konst = self.rng.pct(20);
        let mut names = vec![];
        let mut decl = vec![];
        for t in &tys {
            // Sometimes shadow an existing name.
            let name = if self.rng.pct(10) {
                self.var_of(*t).unwrap_or_else(|| self.fresh("v"))
            } else {
                self.fresh("v")
            };
            let attr = if konst {
                self.hit("attrib:const");
                " <const>"
            } else {
                ""
            };
            let an = self.annot(*t);
            decl.push(split(format!("{name}{attr}{an}"), format!("{name}{attr}")));
            names.push(name);
        }
        let ne = if self.rng.pct(85) { n } else { self.rng.below(n + 2) };
        let mut exprs: Vec<Out> = vec![];
        for i in 0..ne {
            let t = tys.get(i).copied().unwrap_or(Ty::Any);
            exprs.push(self.expr(t, 0));
        }
        if ne > 0 && ne < n && self.rng.pct(50) {
            self.hit("assign:multret-expansion");
            exprs.pop();
            exprs.push(c!("string.byte(\"lua\", 1, -1)"));
        }
        for (i, name) in names.iter().enumerate() {
            let t = if ne == n { tys[i] } else { Ty::Any };
            self.declare(name, t, konst);
        }
        if exprs.is_empty() {
            if konst {
                return c!("local ", join(&decl, ", "), " = nil\n");
            }
            return c!("local ", join(&decl, ", "), "\n");
        }
        c!("local ", join(&decl, ", "), " = ", join(&exprs, ", "), "\n")
    }

    fn target(&mut self) -> Option<(Out, Ty)> {
        if self.rng.pct(25)
            && let Some(t) = self.var_of(Ty::Tab)
        {
            self.hit("assign:field");
            let k = match self.rng.below(4) {
                0 => c!(".", self.rng.pick(&["x", "y", "v", "n"]).to_string()),
                1 => c!("[", self.rng.range(1, 5).to_string(), "]"),
                2 => index(self.str_lit()),
                _ => index(self.expr(Ty::Int, 2)),
            };
            return Some((c!(t, k), Ty::Any));
        }
        if self.rng.pct(8) {
            self.hit("assign:global");
            let g = self.rng.pick(&["G1", "G2", "gx"]).to_string();
            return Some((c!(g), Ty::Any));
        }
        self.assignable(None).map(|(n, t)| (c!(n), t))
    }

    fn assign_stat(&mut self) -> Out {
        self.hit("stat:assign");
        let Some((t, ty)) = self.target() else { return self.local_stat() };
        let e = self.expr(ty, 0);
        c!(t, " = ", e, "\n")
    }

    fn multi_assign_stat(&mut self) -> Out {
        self.hit("stat:multi-assign");
        let mut ts = vec![];
        let mut es = vec![];
        for _ in 0..2 + self.rng.below(2) {
            if let Some((t, ty)) = self.target() {
                ts.push(t);
                es.push(self.expr(ty, 1));
            }
        }
        if ts.is_empty() {
            return self.local_stat();
        }
        if self.rng.pct(30) && es.len() > 1 {
            es.pop();
            es.pop();
            es.push(c!("string.byte(\"ab\", 1, 2)"));
        }
        c!(join(&ts, ", "), " = ", join(&es, ", "), "\n")
    }

    fn compound_stat(&mut self) -> Out {
        self.hit("rlua:compound-assign");
        let ops = [
            ("+=", "+", Ty::Int),
            ("-=", "-", Ty::Flt),
            ("*=", "*", Ty::Int),
            ("/=", "/", Ty::Flt),
            ("//=", "//", Ty::Int),
            ("%=", "%", Ty::Int),
            ("..=", "..", Ty::Str),
        ];
        let (ro, lo, ty) = *self.rng.pick(&ops);
        let target = if self.rng.pct(30) {
            match self.var_of(Ty::Tab) {
                Some(t) => {
                    let k = *self.rng.pick(&[".x", ".v", "[1]", "[\"k\"]"]);
                    format!("{t}{k}")
                }
                None => return self.emit_stat(),
            }
        } else {
            let want = if ty == Ty::Flt { Ty::Int } else { ty };
            match self.assignable(Some(want)).or_else(|| self.assignable(None)) {
                Some((n, _)) => n,
                None => return self.emit_stat(),
            }
        };
        let e = self.expr(ty, 1);
        let sp = if self.rng.pct(20) { "" } else { " " };
        c!(
            split(format!("{target}{sp}{ro}{sp}"), format!("{target} = {target} {lo} (")),
            e,
            split("", ")"),
            "\n"
        )
    }

    fn call_stat(&mut self) -> Out {
        self.hit("stat:call");
        match self.rng.below(5) {
            0 => {
                let ms = self.method_targets();
                if let Some((o, m)) = ms.first().cloned() {
                    let a = self.args(1);
                    return c!(o, ":", m, "(", a, ")\n");
                }
                self.emit_stat()
            }
            1 => {
                self.hit("call:string-method-stat");
                let s = self.str_lit();
                // Leading `;`: a statement starting with `(` would continue the previous one.
                c!(";(", s, "):upper()\n")
            }
            _ => {
                let f = match self.var_of(Ty::Fun) {
                    Some(f) => c!(f),
                    None => return self.emit_stat(),
                };
                let a = self.args(1);
                if self.rng.pct(15) { c!(f, self.str_lit(), "\n") } else { c!(f, "(", a, ")\n") }
            }
        }
    }

    fn emit_stat(&mut self) -> Out {
        self.hit("stat:emit");
        let n = 1 + self.rng.below(3);
        if self.rng.pct(20) {
            self.hit("lib:print");
            let xs: Vec<Out> = (0..n).map(|_| self.prim_expr(0)).collect();
            return c!("print(", join(&xs, ", "), ")\n");
        }
        let xs: Vec<Out> = (0..n).map(|_| self.expr(Ty::Any, 0)).collect();
        c!("__emit(", join(&xs, ", "), ")\n")
    }

    fn cond(&mut self) -> Out {
        if self.rng.pct(70) { self.expr(Ty::Bool, 1) } else { self.expr(Ty::Any, 2) }
    }

    fn if_stat(&mut self) -> Out {
        self.hit("stat:if");
        let c0 = self.cond();
        let b0 = {
            let nn = 1 + self.rng.below(3);
            self.block(nn)
        };
        let mut o = c!("if ", c0, " then\n", b0);
        for _ in 0..self.rng.below(3) {
            self.hit("stat:elseif");
            let c1 = self.cond();
            let b1 = {
                let nn = 1 + self.rng.below(2);
                self.block(nn)
            };
            o = c!(o, "elseif ", c1, " then\n", b1);
        }
        if self.rng.pct(50) {
            self.hit("stat:else");
            let b = {
                let nn = 1 + self.rng.below(2);
                self.block(nn)
            };
            o = c!(o, "else\n", b);
        }
        c!(o, "end\n")
    }

    /// A loop body with an optional continue label at its end (plain-Lua side for
    /// `continue`, both sides for the `goto continue` idiom), placed by `continue_body`.
    fn loop_body(&mut self, kind: LoopKind, n: usize) -> Out {
        let label = self.fresh("cont");
        self.loops.push(LoopCtx { label: label.clone(), used: false, plain: false });
        let b = self.block(n);
        let ends_return = self.block_returned;
        let ctx = self.loops.pop().unwrap_or(LoopCtx { label: String::new(), used: false, plain: false });
        if !ctx.used && !ctx.plain {
            return b;
        }
        continue_body(kind, b, &label, ctx.plain, ends_return)
    }

    fn while_stat(&mut self) -> Out {
        self.hit("stat:while");
        let g = self.fresh("w");
        let k = self.rng.range(0, 4);
        let c0 = self.cond();
        let body = {
            let nn = 1 + self.rng.below(3);
            self.loop_body(LoopKind::While, nn)
        };
        c!(
            format!("local {g} = 0\nwhile {g} < {k} and "),
            self.paren(c0),
            " do\n",
            format!("{g} = {g} + 1\n"),
            body,
            "end\n"
        )
    }

    fn repeat_stat(&mut self) -> Out {
        self.hit("stat:repeat");
        let g = self.fresh("w");
        let k = self.rng.range(1, 4);
        let body = {
            let nn = 1 + self.rng.below(3);
            self.loop_body(LoopKind::Repeat, nn)
        };
        let c0 = self.cond();
        c!(
            format!("local {g} = 0\nrepeat\n{g} = {g} + 1\n"),
            body,
            format!("until {g} >= {k} or "),
            self.paren(c0),
            "\n"
        )
    }

    fn numfor_stat(&mut self) -> Out {
        self.hit("stat:numeric-for");
        let v = self.fresh("i");
        let (head, guarded, ty) = match self.rng.below(12) {
            0..=4 => {
                let a = self.rng.range(-2, 3);
                let b = self.rng.range(-1, 5);
                if self.rng.pct(40) {
                    let s = *self.rng.pick(&["1", "2", "-1", "3", "-2"]);
                    (format!("{a}, {b}, {s}"), false, Ty::Int)
                } else {
                    (format!("{a}, {b}"), false, Ty::Int)
                }
            }
            5 => {
                self.hit("for:float");
                let h = *self.rng.pick(&[
                    "1, 2, 0.25",
                    "0.5, 2",
                    "1.0, 3",
                    "3, 1, -0.5",
                    "0.1, 0.35, 0.1",
                    "1, 2.5",
                ]);
                (h.to_owned(), false, Ty::Flt)
            }
            6 => {
                self.hit("for:extreme");
                let h = *self.rng.pick(&[
                    "math.maxinteger - 2, math.maxinteger",
                    "math.mininteger, math.mininteger + 2",
                    "math.mininteger + 2, math.mininteger, -1",
                    "math.maxinteger - 1, math.huge",
                    "1, math.huge",
                    "-math.huge, 1",
                    "1, 0x7fffffffffffffff, 0x4000000000000000",
                    "0, -1, math.mininteger",
                    "3, 1",
                    "1, 3.7",
                    "1, -math.huge, -1",
                    "1.0, 0x7fffffffffffffff",
                ]);
                (h.to_owned(), true, Ty::Any)
            }
            7 => {
                self.hit("for:error");
                let h = *self.rng.pick(&[
                    "1, 3, 0",
                    "\"1\", 2",
                    "1, \"2\"",
                    "1, 2, \"1\"",
                    "1, nil",
                    "1.5, 2, 0",
                    "{}, 1",
                ]);
                (h.to_owned(), true, Ty::Any)
            }
            _ => {
                let a = self.expr(Ty::Int, 3);
                (format!("({}) % 3, {}", a.l, self.rng.range(0, 4)), false, Ty::Int)
            }
        };
        // The head may only use plain Lua; build it from l (heads never contain RemedyLua syntax).
        self.scopes.push(vec![]);
        self.declare(&v, ty, false);
        let an = self.annot(ty);
        let gname = self.fresh("k");
        let body = {
            let nn = 1 + self.rng.below(3);
            self.loop_body(LoopKind::For, nn)
        };
        self.scopes.pop();
        let guard = if guarded {
            format!("{gname} = {gname} + 1 if {gname} > 4 then break end\n")
        } else {
            String::new()
        };
        let pre = if guarded { format!("local {gname} = 0\n") } else { String::new() };
        c!(
            pre,
            split(format!("for {v}{an} = {head} do\n"), format!("for {v} = {head} do\n")),
            guard,
            body,
            "end\n"
        )
    }

    fn genfor_stat(&mut self) -> Out {
        self.hit("stat:generic-for");
        let k = self.fresh("k");
        let v = self.fresh("x");
        let which = self.rng.below(9);
        let (head, vars): (Out, Vec<(String, Ty)>) = match which {
            0 => {
                self.hit("for:ipairs");
                let t = self.expr(Ty::Tab, 1);
                (c!("ipairs(", t, ")"), vec![(k.clone(), Ty::Int), (v.clone(), Ty::Any)])
            }
            1 => {
                self.hit("for:sorted-pairs");
                let t = self.expr(Ty::Tab, 1);
                (c!("__spairs(", t, ")"), vec![(k.clone(), Ty::Any), (v.clone(), Ty::Any)])
            }
            2 => {
                // pairs order is unspecified: the body only counts.
                self.hit("for:pairs-count");
                let t = self.expr(Ty::Tab, 1);
                let c0 = self.fresh("cnt");
                let nx = if self.rng.pct(50) { c!("pairs(", t, ")") } else { c!("next, ", t) };
                return c!(
                    format!("do local {c0} = 0\nfor {k}, {v} in "),
                    nx,
                    format!(" do {c0} = {c0} + 1 end\n__emit(\"count\", {c0})\nend\n")
                );
            }
            3 => {
                self.hit("for:gmatch");
                let s = self.expr(Ty::Str, 1);
                let p = self.pattern();
                (c!("string.gmatch(", s, ", ", p, ")"), vec![(k.clone(), Ty::Any), (v.clone(), Ty::Any)])
            }
            4 => {
                self.hit("for:utf8.codes");
                let s = self.expr(Ty::Str, 1);
                (c!("utf8.codes(", s, ")"), vec![(k.clone(), Ty::Int), (v.clone(), Ty::Int)])
            }
            5 => {
                self.hit("for:closure-iterator");
                let n = self.rng.range(0, 4);
                (
                    c!(format!(
                        "(function() local i = 0 return function() i = i + 1 if i <= {n} then return i, i * 2 end end end)()"
                    )),
                    vec![(k.clone(), Ty::Int), (v.clone(), Ty::Int)],
                )
            }
            6 => {
                self.hit("for:stateless-iterator");
                (
                    c!("function(s, c) if c < 3 then return c + 1, s end end, \"st\", 0"),
                    vec![(k.clone(), Ty::Int), (v.clone(), Ty::Str)],
                )
            }
            7 => {
                self.hit("for:4th-closing-value");
                let id = self.fresh("fc");
                (
                    c!(format!(
                        "next, {{1, 2}}, nil, setmetatable({{}}, {{__close = function(o, e) __emit(\"for-close\", \"{id}\", e) end}})"
                    )),
                    vec![(k.clone(), Ty::Any), (v.clone(), Ty::Any)],
                )
            }
            _ => {
                self.hit("for:string.gmatch-captures");
                let s = self.expr(Ty::Str, 1);
                (
                    c!("string.gmatch(", s, ", \"(%a)(%d*)\")"),
                    vec![(k.clone(), Ty::Str), (v.clone(), Ty::Str)],
                )
            }
        };
        // `for k, v in next, {1, 2}` with a 4th value: order of next over an array part.
        self.scopes.push(vec![]);
        for (n, t) in &vars {
            self.declare(n, *t, false);
        }
        let body = {
            let nn = 1 + self.rng.below(3);
            self.loop_body(LoopKind::For, nn)
        };
        self.scopes.pop();
        let mut names_r = vec![];
        let mut names_l = vec![];
        for (n, t) in &vars {
            names_r.push(format!("{n}{}", self.annot(*t)));
            names_l.push(n.clone());
        }
        c!(
            split(format!("for {} in ", names_r.join(", ")), format!("for {} in ", names_l.join(", "))),
            head,
            " do\n",
            body,
            "end\n"
        )
    }

    fn local_function(&mut self) -> Out {
        self.hit("stat:local-function");
        let name = self.fresh("f");
        let (params, vararg) = self.params();
        let (pr, pl) = self.param_text(&params, vararg);
        let (ret_r, ret_l) = self.ret_annot();
        self.declare(&name, Ty::Fun, false);
        let body = {
            let nn = 1 + self.rng.below(4);
            self.func_body(&params, vararg, nn)
        };
        c!(
            split(
                format!("local function {name}({pr}){ret_r}\n"),
                format!("local function {name}({pl}){ret_l}\n")
            ),
            body,
            "end\n"
        )
    }

    fn function_stat(&mut self) -> Out {
        self.hit("stat:function");
        let (params, vararg) = self.params();
        let (pr, pl) = self.param_text(&params, vararg);
        let (target, is_global) = match self.rng.below(3) {
            0 => {
                if let Some(t) = self.var_of(Ty::Tab) {
                    self.hit("function:dotted-name");
                    (format!("{t}.{}", self.fresh("fn")), false)
                } else {
                    (self.fresh("Gf"), true)
                }
            }
            _ => (self.fresh("Gf"), true),
        };
        let body = {
            let nn = 1 + self.rng.below(3);
            self.func_body(&params, vararg, nn)
        };
        if is_global {
            self.hit("function:global");
            // Visible afterwards as a global function.
            self.declare(&target, Ty::Fun, false);
        }
        c!(split(format!("function {target}({pr})\n"), format!("function {target}({pl})\n")), body, "end\n")
    }

    fn method_object(&mut self) -> Out {
        self.hit("stat:method-definition");
        let o = self.fresh("obj");
        let m = self.fresh("m");
        let v = self.int_lit();
        self.declare(&o, Ty::Tab, false);
        let (params, vararg) = self.params();
        let (pr, pl) = self.param_text(&params, vararg);
        let mut ps = vec![("self".to_owned(), Ty::Tab)];
        ps.extend(params);
        let body = {
            let nn = 1 + self.rng.below(3);
            self.func_body(&ps, vararg, nn)
        };
        self.set_methods(&o, m.clone());
        let nested = if self.rng.pct(30) {
            self.hit("function:a.b:c");
            let m2 = self.fresh("m");
            format!(
                "{o}.inner = {{w = 5}}\nfunction {o}.inner:{m2}(a) if __enter() then return end return self.w, a end\n__emit({o}.inner:{m2}(3))\n"
            )
        } else {
            String::new()
        };
        c!(
            format!("local {o} = {{v = {v}}}\n"),
            split(format!("function {o}:{m}({pr})\n"), format!("function {o}:{m}({pl})\n")),
            body,
            "end\n",
            nested
        )
    }

    fn goto_stat(&mut self) -> Out {
        self.hit("stat:goto");
        match self.rng.below(3) {
            0 => {
                self.hit("goto:forward");
                let l = self.fresh("L");
                let c0 = self.cond();
                let b = {
                    let nn = 1 + self.rng.below(2);
                    self.block(nn)
                };
                c!("if ", c0, format!(" then goto {l} end\ndo\n"), b, format!("end\n::{l}::\n"))
            }
            1 => {
                self.hit("goto:backward");
                let l = self.fresh("L");
                let g = self.fresh("g");
                let n = self.rng.range(1, 3);
                let b = {
                    let nn = 1 + self.rng.below(2);
                    self.block(nn)
                };
                c!(
                    format!("do\nlocal {g} = 0\n::{l}::\n{g} = {g} + 1\ndo\n"),
                    b,
                    format!("end\nif {g} < {n} then goto {l} end\nend\n")
                )
            }
            _ => {
                if self.loops.is_empty() {
                    return self.emit_stat();
                }
                // `goto continue`: jumps to the label at the end of the loop body.
                self.hit("goto:continue-idiom");
                let c0 = self.cond();
                if let Some(lp) = self.loops.last_mut() {
                    lp.plain = true;
                    let l = lp.label.clone();
                    return c!("if ", c0, format!(" then goto {l} end\n"));
                }
                self.emit_stat()
            }
        }
    }

    fn loop_exit_stat(&mut self) -> Out {
        let c0 = self.cond();
        if self.rl && self.rng.pct(60) {
            self.hit("rlua:continue");
            if let Some(lp) = self.loops.last_mut() {
                lp.used = true;
                let l = lp.label.clone();
                return c!("if ", c0, " then ", split("continue", format!("goto {l}")), " end\n");
            }
        }
        self.hit("stat:break");
        c!("if ", c0, " then break end\n")
    }

    fn pcall_stat(&mut self) -> Out {
        self.hit("stat:pcall-block");
        let (params, vararg) = self.params();
        let (pr, pl) = self.param_text(&params, vararg);
        let body = {
            let nn = 1 + self.rng.below(4);
            self.func_body(&params, vararg, nn)
        };
        let args = self.args(1);
        let sep = if args.l.is_empty() { "" } else { ", " };
        if self.rng.pct(25) {
            self.hit("lib:xpcall");
            let h = *self.rng.pick(&[
                "function(e) return \"handled: \" .. tostring(e) end",
                "function(e) return e, 2 end",
                "function(e) error(\"in handler\") end",
                "__tag",
            ]);
            return c!(
                split(format!("__emit(xpcall(function({pr})\n"), format!("__emit(xpcall(function({pl})\n")),
                body,
                format!("end, {h}{sep}"),
                args,
                "))\n"
            );
        }
        c!(
            split(format!("__emit(pcall(function({pr})\n"), format!("__emit(pcall(function({pl})\n")),
            body,
            format!("end{sep}"),
            args,
            "))\n"
        )
    }

    fn close_stat(&mut self) -> Out {
        self.hit("attrib:close");
        let id = self.fresh("c");
        let v = match self.rng.below(5) {
            0 => "nil".to_owned(),
            1 => "false".to_owned(),
            2 => "{}".to_owned(), // error: no __close
            _ => format!(
                "setmetatable({{v = \"{id}\"}}, {{__close = function(o, e) __emit(\"close\", __tag(o), e) end}})"
            ),
        };
        let b = {
            let nn = 1 + self.rng.below(3);
            self.block(nn)
        };
        let an = if self.rl && self.rng.pct(30) { ": any" } else { "" };
        c!(
            "do\n",
            split(format!("local {id} <close>{an} = {v}\n"), format!("local {id} <close> = {v}\n")),
            b,
            "end\n"
        )
    }

    fn error_stat(&mut self) -> Out {
        self.hit("lib:error");
        let c0 = self.cond();
        let e = match self.rng.below(7) {
            0 => format!("error({})", self.str_lit()),
            1 => format!("error({}, 0)", self.str_lit()),
            2 => format!("error({}, 2)", self.str_lit()),
            3 => "error({code = 1})".into(),
            4 => "error()".into(),
            5 => format!("error({})", self.int_lit()),
            _ => "error(setmetatable({}, {__tostring = function() return \"custom\" end}))".into(),
        };
        let e2 = if self.rng.pct(30) { "assert(false, \"assert msg\")".to_owned() } else { e };
        c!("if ", c0, format!(" then {e2} end\n"))
    }

    fn vararg_stat(&mut self) -> Out {
        if !self.vararg {
            return self.emit_stat();
        }
        self.hit("vararg:stat");
        let a = self.fresh("va");
        let b = self.fresh("vb");
        self.declare(&a, Ty::Any, false);
        self.declare(&b, Ty::Any, false);
        let x = *self.rng.pick(&[
            "__emit(select('#', ...), ...)",
            "__emit(table.pack(...).n)",
            "__emit(select(-1, ...))",
            "__emit((select(2, ...)))",
            "__emit(table.unpack({...}))",
            "__emit(..., \"after\")",
            "__emit(select(0, ...))",
        ]);
        c!(format!("local {a}, {b} = ...\n{x}\n"))
    }

    fn lib_stat(&mut self) -> Out {
        self.hit("stat:library");
        match self.rng.below(14) {
            0 => {
                self.hit("lib:table.insert/remove");
                let t = self.fresh("t");
                let x = self.expr(Ty::Int, 2);
                let pos = self.rng.range(0, 5);
                c!(
                    format!("do local {t} = {{1, 2, 3}}\ntable.insert({t}, "),
                    x,
                    format!(
                        ")\n__emit(pcall(table.insert, {t}, {pos}, \"p\"))\n__emit(table.remove({t}), table.remove({t}, 1), pcall(table.remove, {t}, {pos}))\n__emit({t})\nend\n"
                    )
                )
            }
            1 => {
                self.hit("lib:table.sort");
                let t = self.fresh("t");
                let n = 1 + self.rng.below(8);
                let ty = if self.rng.pct(50) { Ty::Int } else { Ty::Str };
                let xs: Vec<Out> = (0..n).map(|_| self.expr(ty, 3)).collect();
                let cmp = *self.rng.pick(&[
                    "",
                    ", function(a, b) return a > b end",
                    ", function(a, b) return a < b end",
                ]);
                c!(
                    format!("do local {t} = {{"),
                    join(&xs, ", "),
                    format!("}}\n__emit(pcall(table.sort, {t}{cmp}))\n__emit({t})\nend\n")
                )
            }
            2 => {
                self.hit("lib:table.move");
                let a = self.rng.range(1, 3);
                let e = self.rng.range(0, 4);
                let t = self.rng.range(-1, 5);
                c!(format!(
                    "__emit(table.move({{1, 2, 3, 4}}, {a}, {e}, {t}))\n__emit(table.move({{1, 2}}, 1, 2, 2, {{9}}))\n"
                ))
            }
            3 => {
                self.hit("lib:table.unpack");
                let i = self.rng.range(-1, 3);
                let j = self.rng.range(0, 5);
                c!(format!("__emit(table.unpack({{1, 2, 3}}, {i}, {j}))\n"))
            }
            4 => {
                self.hit("lib:string.unpack");
                let (f, s) = *self.rng.pick(&[
                    ("\"<i4\"", "string.pack(\"<i4\", -2)"),
                    ("\"z\"", "\"abc\\0def\""),
                    ("\">s1\"", "\"\\3abc\""),
                    ("\"<d\"", "string.pack(\"<d\", 0.1)"),
                    ("\"i3\"", "\"\\255\\255\\127\""),
                    ("\"<I16\"", "string.rep(\"\\255\", 16)"),
                    ("\"c2\"", "\"x\""),
                ]);
                c!(format!("__emit(pcall(string.unpack, {f}, {s}))\n__emit(pcall(string.packsize, {f}))\n"))
            }
            5 => {
                self.hit("lib:utf8");
                let s = self.expr(Ty::Str, 2);
                let fs = *self.rng.pick(&[
                    "utf8.len(s)",
                    "utf8.codepoint(s, 1, -1)",
                    "utf8.offset(s, 2)",
                    "utf8.offset(s, -1)",
                    "utf8.len(s, 2, -2, true)",
                    "utf8.codepoint(s, 1, 1, true)",
                ]);
                c!(
                    "do local s = ",
                    s,
                    format!("\n__emit(pcall(function() return {fs} end))\n__emit(utf8.charpattern)\nend\n")
                )
            }
            6 => {
                self.hit("lib:math.random");
                let s = self.rng.range(0, 100);
                c!(format!(
                    "math.randomseed({s})\n__emit(math.random(1, 100), math.random(5), math.random(0) ~= nil, math.random() < 1)\n"
                ))
            }
            7 => {
                self.hit("lib:string.format-error");
                let (f, _) = self.fmt_spec();
                let a = self.prim_expr(2);
                c!("__emit(pcall(string.format, ", f, ", ", a, "))\n")
            }
            8 => {
                self.hit("lib:gsub-stat");
                let s = self.expr(Ty::Str, 2);
                let p = self.pattern();
                c!("__emit(pcall(string.gsub, ", s, ", ", p, ", \"[%0]\"))\n")
            }
            9 => {
                self.hit("lib:string.find-init");
                let s = self.expr(Ty::Str, 2);
                let p = self.pattern();
                let i = self.rng.range(-6, 8);
                c!("__emit(pcall(string.find, ", s, ", ", p, format!(", {i}))\n"))
            }
            10 => {
                self.hit("lib:rawlen/rawget/rawset");
                let t = self.expr(Ty::Tab, 2);
                c!(
                    "do local t = ",
                    t,
                    "\n__emit(pcall(rawlen, t), rawget(t, 1), rawequal(t, t), pcall(rawset, t, \"rk\", 1))\nend\n"
                )
            }
            11 => {
                self.hit("lib:getmetatable");
                let t = self.expr(Ty::Any, 2);
                c!("__emit(type(getmetatable(", t, ")), getmetatable(\"\").__index == string)\n")
            }
            12 => {
                self.hit("lib:os.time");
                let y = self.rng.range(1970, 2100);
                let m = self.rng.range(-2, 14);
                let d = self.rng.range(0, 40);
                c!(format!("__emit(pcall(os.time, {{year = {y}, month = {m}, day = {d}, hour = 12}}))\n"))
            }
            _ => {
                self.hit("lib:tostring-tonumber-edge");
                let x = *self.rng.pick(&[
                    "tonumber(\"0x\")",
                    "tonumber(\"1e\")",
                    "tonumber(\"  0x1p4  \")",
                    "tonumber(\"10\", 16)",
                    "tonumber(\"ff\", 16)",
                    "tonumber(\"8\", 8)",
                    "tonumber(\"1.5\", 10)",
                    "tonumber(\"-zz\", 36)",
                    "tonumber(\"9223372036854775808\")",
                    "tostring(2^63)",
                    "tostring(-0.0)",
                    "tostring(1e15)",
                    "tostring(1e16)",
                    "tostring(0.1)",
                    "tostring(1/3)",
                    "math.tointeger(\"8\")",
                    "math.tointeger(2^53)",
                    "3 % -2",
                    "-3 % 2",
                    "3.5 % -2",
                    "-3 // 0.0",
                    "0.0 / 0.0 ~= 0.0 / 0.0",
                    "math.mininteger // -1",
                    "math.mininteger % -1",
                    "1 << 64",
                    "1 << 63",
                    "-1 >> 1",
                    "1 >> -1",
                    "2^-1074",
                    "math.maxinteger + 0.0 == math.maxinteger",
                    "math.maxinteger < math.maxinteger + 0.0",
                    "2^53 + 1 == 2^53 + 1.0",
                    "\"10\" + 1",
                    "\"3\" * \"4\"",
                    "10 .. 20",
                    "1e100 // 1",
                    "math.fmod(-6, 4)",
                    "math.fmod(6, -4.0)",
                    "string.format(\"%.3f\", 2.5)",
                    "string.format(\"%5.1f|%-8.3e|%g\", 3.14159, 1234.5, 1e20)",
                    "string.format(\"%i\", 3.0)",
                    "#\"\\0\\0\"",
                    "string.rep(\"ab\", 3, \"\")",
                    "(\"x\"):rep(0)",
                    "math.abs(math.mininteger)",
                    "math.floor(-0.0)",
                    "math.ceil(-0.5)",
                    "select(-1, 1, 2, 3)",
                    "string.format(\"%q\", 1/0)",
                    "string.format(\"%q\", math.mininteger)",
                    "8 // 3 * 3 + 8 % 3",
                    "2^2^3",
                    "-2^2",
                    "not nil == true",
                    "1 .. 2 == \"12\"",
                    "\"a\" < \"b\"",
                    "\"Z\" < \"a\"",
                    "\"\" < \"\\0\"",
                    "math.type(1//1)",
                    "math.type(1.0//1)",
                    "7 // 2.0",
                    "-7 // 2",
                    "-7 % 2.0",
                    "0x10p-1",
                    "1e2 == 100",
                ]);
                c!(format!("__emit(pcall(function() return {x} end))\n"))
            }
        }
    }

    fn coroutine_stat(&mut self) -> Out {
        self.hit("stat:coroutine");
        let co = self.fresh("co");
        let (params, vararg) = self.params();
        let (pr, pl) = self.param_text(&params, vararg);
        let saved = self.in_fn;
        let body = self.co_body(&params, vararg);
        self.in_fn = saved;
        let n = self.rng.range(1, 4);
        if self.rng.pct(35) {
            self.hit("lib:coroutine.wrap");
            return c!(
                split(
                    format!("do local {co} = coroutine.wrap(function({pr})\n"),
                    format!("do local {co} = coroutine.wrap(function({pl})\n")
                ),
                body,
                format!("end)\nfor i = 1, {n} do __emit(\"wrap\", pcall({co}, i)) end\nend\n")
            );
        }
        let close = if self.rng.pct(30) {
            self.hit("lib:coroutine.close");
            format!("__emit(\"close\", coroutine.close({co}), coroutine.status({co}))\n")
        } else {
            String::new()
        };
        c!(
            split(
                format!("do local {co} = coroutine.create(function({pr})\n"),
                format!("do local {co} = coroutine.create(function({pl})\n")
            ),
            body,
            format!(
                "end)\nfor i = 1, {n} do __emit(\"resume\", coroutine.resume({co}, i, \"r\")) end\n__emit(coroutine.status({co}))\n{close}end\n"
            )
        )
    }

    fn co_body(&mut self, params: &[(String, Ty)], vararg: bool) -> Out {
        let saved = (std::mem::take(&mut self.loops), self.vararg, self.in_fn);
        self.vararg = vararg;
        self.in_fn = true;
        self.scopes.push(vec![]);
        for (p, t) in params {
            self.declare(p, *t, false);
        }
        let mut o = Out::default();
        for _ in 0..1 + self.rng.below(4) {
            let s = if self.rng.pct(40) {
                self.hit("lib:coroutine.yield");
                let y = self.fresh("y");
                let e = self.expr(Ty::Any, 2);
                self.declare(&y, Ty::Any, false);
                match self.rng.below(4) {
                    0 => c!(format!("local {y} = coroutine.yield("), e, ")\n"),
                    1 => c!("__emit(\"got\", coroutine.yield(", e, ", 2))\n"),
                    2 => c!("__emit(\"in-pcall\", pcall(coroutine.yield, ", e, "))\n"),
                    _ => c!(
                        "__emit(coroutine.running() ~= nil, coroutine.isyieldable(), coroutine.status((coroutine.running())))\n"
                    ),
                }
            } else {
                self.stat()
            };
            o = c!(o, s);
        }
        if self.rng.pct(50) {
            let r = self.return_stat();
            o = c!(o, r);
        }
        self.scopes.pop();
        (self.loops, self.vararg, self.in_fn) = saved;
        o
    }

    fn meta_stat(&mut self) -> Out {
        self.hit("stat:metatable-ops");
        let m = self.fresh("mo");
        let mt = self.metatable(1);
        let v = self.int_lit();
        let mut o = c!(format!("local {m} = setmetatable({{v = {v}}}, "), mt, ")\n");
        self.declare(&m, Ty::Tab, false);
        for _ in 0..1 + self.rng.below(4) {
            let op = *self.rng.pick(&[
                "__emit(pcall(function() return M + 1 end))",
                "__emit(pcall(function() return 2 * M end))",
                "__emit(pcall(function() return M .. \"s\", \"s\" .. M, 1 .. M end))",
                "__emit(pcall(function() return #M end))",
                "__emit(pcall(function() return -M, ~M end))",
                "__emit(pcall(function() return M == setmetatable({}, getmetatable(M)), M ~= {} end))",
                "__emit(pcall(function() return M < M, M <= M, M > 1 end))",
                "__emit(pcall(function() return M(1, 2) end))",
                "__emit(pcall(function() return M.x, M.zz, M[1] end))",
                "__emit(pcall(function() M.newkey = 5 return rawget(M, \"newkey\") end))",
                "__emit(pcall(tostring, M) and \"tostring-ok\" or \"tostring-err\", select(2, pcall(tostring, M)) ~= nil)",
                "__emit(pcall(function() return M // 2, M % 2, M ^ 2, M / 2 end))",
                "__emit(pcall(function() return M & 1, M | 1, M ~ 1, M << 1, M >> 1 end))",
                "__emit(pcall(setmetatable, M, {}))",
                "__emit(type(getmetatable(M)))",
                "__emit(pcall(string.rep, M, 2))",
                "__emit(pcall(ipairs, M))",
                "__emit(pcall(function() for k, v in pairs(M) do end return \"pairs-ok\" end))",
                "__emit(pcall(function() local x <close> = M return \"closed\" end))",
                "__emit(pcall(function() return M.x.y.z end))",
                "__emit(pcall(math.floor, M))",
            ]);
            o = c!(o, op.replace('M', &m), "\n");
        }
        o
    }

    // ---------------------------------------------------------------- programs

    /// A whole program: some top-level locals, then segments (raw statements or
    /// protected `__seg` blocks), then a `return`.
    pub fn program(&mut self, size: i32) -> Out {
        self.budget = size;
        self.scopes = vec![vec![]];
        let mut o = Out::default();
        for t in [Ty::Int, Ty::Flt, Ty::Str, Ty::Tab, Ty::Bool] {
            if self.rng.pct(70) {
                let n = self.fresh("g");
                let e = self.expr(t, 3);
                let an = self.annot(t);
                o = c!(o, split(format!("local {n}{an} = "), format!("local {n} = ")), e, "\n");
                self.declare(&n, t, false);
            }
        }
        let mut seg = 0;
        while self.budget > 0 {
            seg += 1;
            if self.rng.pct(25) {
                let s = self.stat();
                o = c!(o, s);
            } else {
                self.hit("program:segment");
                let (params, vararg) = self.params();
                let (pr, pl) = self.param_text(&params, vararg);
                let body = {
                    let nn = 1 + self.rng.below(5);
                    self.func_body(&params, vararg, nn)
                };
                let args = self.args(1);
                let sep = if args.l.is_empty() { "" } else { ", " };
                o = c!(
                    o,
                    split(format!("__seg({seg}, function({pr})\n"), format!("__seg({seg}, function({pl})\n")),
                    body,
                    format!("end{sep}"),
                    args,
                    ")\n"
                );
            }
        }
        let vs: Vec<String> = self.visible().into_iter().take(4).map(|v| v.name.clone()).collect();
        c!(o, format!("return {}\n", vs.join(", ")))
    }

    /// One construct nested very deeply (checks the parser's limits agree).
    pub fn deep(&mut self) -> Out {
        self.hit("program:deep");
        let n = *self.rng.pick(&[10usize, 50, 100, 150, 190, 195, 198, 199, 200, 201, 220, 250, 300, 1000]);
        let s = match self.rng.below(12) {
            0 => format!("return {}1{}", "(".repeat(n), ")".repeat(n)),
            1 => format!("return {}1{}", "{".repeat(n), "}".repeat(n)),
            2 => format!("{}{}", "do ".repeat(n), "end ".repeat(n)),
            3 => format!("return {}1", "- ".repeat(n)),
            4 => format!("return {}1", "not ".repeat(n)),
            5 => format!("local x = 1 return x{}", " .. x".repeat(n)),
            6 => {
                format!("local f = function(...) return ... end return {}1{}", "f(".repeat(n), ")".repeat(n))
            }
            7 => format!("{}return 1{}", "local function f() ".repeat(n), " end".repeat(n)),
            8 => format!("{}local z = 1{}", "if true then ".repeat(n), " end".repeat(n)),
            9 => format!("local t = {{}} t{} = 1 return 1", ".a".repeat(n)),
            10 => format!("return 1{}", " + 1".repeat(n * 10)),
            _ => format!("local x = 1 return {}x{}", "2^".repeat(n), ""),
        };
        c!(s)
    }
}
