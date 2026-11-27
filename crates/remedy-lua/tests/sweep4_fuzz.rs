//! Sweep 4, area "fuzz": long differential fuzzing where the sweep 1 grammar is thin.
//!
//! `focus` programs are built from units aimed at metamethod-heavy code (every event,
//! chains, errors, non-function handlers, yields inside them), coroutines mixed with
//! closures, `goto`, `<close>`, string library chains, integer/float boundaries and the
//! Lua 5.3 compatibility paths (`math.pow`, `__le` through `__lt`, ...). Each program runs
//! through the sweep 1 pipeline (`sweep1_fuzz::handle_plain`): fast loop, full path and C Lua
//! (built with `LUA_COMPAT_5_3`) must agree on results and errors, the two RemedyLua runs on
//! fuel and memory too, and fuel and memory cut points must stop cleanly and identically.
//!
//! - `focus_quick` (default run): a handful of programs, so the generator stays valid.
//! - `fuzz_focus` / `fuzz_focus_mutated` (ignored, long): `SWEEP4_ITERS`, `SWEEP4_SEED`,
//!   `SWEEP4_SECS`; findings go to `SWEEP1_OUT` like the sweep 1 drivers.
//! - `focus_front_equiv` (feature `front-oracle`): old and new front ends on generated
//!   and mutated focus programs (`SWEEP4_FRONT_ITERS`).
//! - `coverage` (ignored): which opcodes (compiled) and library functions (named) the
//!   sweep 1 grammar and the focus generator reach.
//! - `regressions`: one program per finding fixed in sweep 4.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs, dead_code)]

use std::fmt::Write as _;

use super::sweep1_fuzz::harness::{self, Class};
use super::sweep1_fuzz::rng::Rng;
use super::sweep1_fuzz::{Recorder, handle_plain, mutate};

fn env_u64(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

// ---------------------------------------------------------------- the generator

/// Boundary numbers, as Lua source.
const NUMS: &[&str] = &[
    "0",
    "1",
    "-1",
    "2",
    "3",
    "7",
    "-7",
    "63",
    "64",
    "-64",
    "255",
    "math.maxinteger",
    "math.mininteger",
    "math.maxinteger - 1",
    "math.mininteger + 1",
    "0.0",
    "-0.0",
    "0.5",
    "-2.5",
    "1.5",
    "3.0",
    "1e308",
    "-1e308",
    "math.huge",
    "-math.huge",
    "2^53",
    "2^53 + 1",
    "2^63",
    "-2^63",
    "2^31",
    "1e15",
    "1e16",
    "0.1",
    "5e-324",
    "9007199254740993",
    "0x7fffffffffffffff",
    "0xffffffffffffffff",
    "1 // 1",
    "-0x8000000000000000",
    "2^-1074",
    "(2^53 - 1) // 1",
    "math.pi",
];
/// Strings that coerce (or almost) and plain ones.
const STRS: &[&str] = &[
    "\"10\"",
    "\"0x10\"",
    "\" 5 \"",
    "\"1e2\"",
    "\"abc\"",
    "\"\"",
    "\"-0\"",
    "\"1.5\"",
    "\"9223372036854775808\"",
    "\"-9223372036854775808\"",
    "\"0x7fffffffffffffff\"",
    "\"1e\"",
    "\"inf\"",
    "\"nan\"",
    "\" 0x1p4 \"",
    "\"\\0\"",
    "string.rep(\"ab\", 25)",
    "\"hello world\"",
    "\"3\"",
];
const BINOPS: &[&str] =
    &["+", "-", "*", "/", "//", "%", "^", "&", "|", "~", "<<", ">>", "..", "==", "~=", "<", "<=", ">", ">="];
const ARITH_EV: &[&str] = &[
    "__add", "__sub", "__mul", "__div", "__mod", "__pow", "__idiv", "__band", "__bor", "__bxor", "__shl",
    "__shr", "__concat",
];
const UN_EV: &[&str] = &["__unm", "__bnot", "__len"];
const CMP_EV: &[&str] = &["__eq", "__lt", "__le"];

/// Writes the source of one program out of units.
pub(crate) struct Focus<'a> {
    rng: &'a mut Rng,
    n: usize,
    /// Objects with metatables declared so far (`O1`, `O2`, ...).
    objs: usize,
    pub(crate) hits: std::collections::BTreeMap<&'static str, u64>,
}

impl<'a> Focus<'a> {
    pub(crate) fn new(rng: &'a mut Rng) -> Focus<'a> {
        Focus { rng, n: 0, objs: 0, hits: Default::default() }
    }

    fn hit(&mut self, k: &'static str) {
        *self.hits.entry(k).or_default() += 1;
    }

    fn id(&mut self) -> usize {
        self.n += 1;
        self.n
    }

    fn num(&mut self) -> String {
        (*self.rng.pick(NUMS)).to_owned()
    }

    fn operand(&mut self) -> String {
        match self.rng.below(10) {
            0..=3 => self.num(),
            4 | 5 => (*self.rng.pick(STRS)).to_owned(),
            6..=8 if self.objs > 0 => format!("O{}", 1 + self.rng.below(self.objs)),
            6 => "nil".into(),
            7 => "true".into(),
            _ => "{}".into(),
        }
    }

    /// A metamethod body for event `ev` (arguments `a, b`).
    fn handler(&mut self, ev: &str, yields: bool) -> String {
        let id = self.id();
        let cmp = CMP_EV.contains(&ev);
        let r = match self.rng.below(if yields { 12 } else { 10 }) {
            0 => format!("return \"{ev}{id}\""),
            1 => format!("return {id}"),
            2 => "return __tag(a), __tag(b)".into(),
            3 => format!("error(\"mm{id}\")"),
            4 => format!("error({{code = {id}}})"),
            5 => "return a".into(),
            6 => "return nil".into(),
            7 if cmp => "return 0".into(),
            7 => "return rawequal(a, b), rawlen({1, 2})".into(),
            8 => format!("error(\"lvl{id}\", 2)"),
            9 => "return (select('#', a, b))".into(),
            _ => format!("return (coroutine.yield(\"y{id}\"))"),
        };
        format!("function(a, b) {r} end")
    }

    /// `local On = setmetatable({v = ...}, {...})`: a random set of events.
    fn object(&mut self, yields: bool) -> String {
        self.hit("meta:object");
        self.objs += 1;
        let o = self.objs;
        let mut mt = String::new();
        let mut evs: Vec<&str> = ARITH_EV.iter().chain(UN_EV).chain(CMP_EV).copied().collect();
        evs.extend(["__index", "__newindex", "__call", "__close", "__tostring", "__name", "__metatable"]);
        for ev in evs {
            if !self.rng.pct(45) {
                continue;
            }
            let h = match ev {
                "__index" => match self.rng.below(5) {
                    0 if o > 1 => format!("O{}", self.rng.range(1, o as i64 - 1)),
                    1 => "string".into(),
                    2 => "\"str\"".into(),
                    3 => "setmetatable({x = 1}, {__index = function(t, k) return type(k) == \"number\" and k * 2 or nil end})".into(),
                    _ => self.handler(ev, yields),
                },
                "__newindex" => match self.rng.below(3) {
                    0 => "{}".into(),
                    1 => "function(t, k, v) rawset(t, k, type(v) == \"number\" and v + 1 or v) end".into(),
                    _ => self.handler(ev, yields),
                },
                "__call" => match self.rng.below(3) {
                    0 => "function(self, ...) return select('#', ...), ... end".into(),
                    1 => "setmetatable({}, {__call = function(...) return select('#', ...) end})".into(),
                    _ => self.handler(ev, yields),
                },
                "__close" => match self.rng.below(3) {
                    0 => "function(o, e) __emit(\"close\", __tag(o), e) end".into(),
                    1 => "function(o, e) error(\"in close\") end".into(),
                    _ => "42".into(),
                },
                "__tostring" => match self.rng.below(3) {
                    0 => format!("function(o) return \"T{o}\" end"),
                    1 => "function(o) return 7 end".into(),
                    _ => "function(o) error(\"ts\") end".into(),
                },
                "__name" => (*self.rng.pick(&["\"MyType\"", "7"])).to_owned(),
                "__metatable" => "\"locked\"".into(),
                _ if self.rng.pct(8) => (*self.rng.pick(&["5", "\"str\"", "{}"])).to_owned(),
                _ => self.handler(ev, yields),
            };
            let _ = write!(mt, "{ev} = {h}, ");
        }
        let v = self.num();
        format!("local O{o} = setmetatable({{v = {v}, 1, 2}}, {{{mt}}})\n")
    }

    fn expr(&mut self, d: usize) -> String {
        if d == 0 || self.rng.pct(35) {
            return self.operand();
        }
        match self.rng.below(6) {
            0 => format!("{}({})", self.rng.pick(&["-", "~", "#", "not "]), self.expr(d - 1)),
            1 if self.objs > 0 => {
                let o = 1 + self.rng.below(self.objs);
                let k = self.rng.pick(&["x", "v", "len", "upper", "zz", "[1]", "[3]", "[2.0]", "[\"v\"]"]);
                if k.starts_with('[') { format!("O{o}{k}") } else { format!("O{o}.{k}") }
            }
            2 if self.objs > 0 => format!("O{}({})", 1 + self.rng.below(self.objs), self.expr(d - 1)),
            _ => {
                let op = self.rng.pick(BINOPS);
                format!("({} {op} {})", self.expr(d - 1), self.expr(d - 1))
            }
        }
    }

    // -- units

    fn meta_unit(&mut self) -> String {
        self.hit("unit:meta");
        let mut s = String::new();
        if self.objs == 0 || self.rng.pct(40) {
            s += &self.object(false);
        }
        let id = self.id();
        let _ = write!(s, "__seg({id}, function()\n");
        for _ in 0..1 + self.rng.below(5) {
            let e = self.expr(3);
            match self.rng.below(8) {
                0 if self.objs > 0 => {
                    let o = 1 + self.rng.below(self.objs);
                    let _ = writeln!(
                        s,
                        "__emit(pcall(function() O{o}.nk = {e} return rawget(O{o}, \"nk\") end))"
                    );
                }
                1 if self.objs > 0 => {
                    let o = 1 + self.rng.below(self.objs);
                    let f = self.rng.pick(&[
                        "tostring",
                        "print",
                        "getmetatable",
                        "rawlen",
                        "next",
                        "ipairs",
                        "type",
                    ]);
                    let _ = writeln!(s, "__emit(pcall({f}, O{o}))");
                }
                2 if self.objs > 0 => {
                    let o = 1 + self.rng.below(self.objs);
                    let _ =
                        writeln!(s, "__emit(pcall(function() local c <close> = O{o} return \"body\" end))");
                }
                3 if self.objs > 0 => {
                    let o = 1 + self.rng.below(self.objs);
                    let f = self.rng.pick(&["\"%s\"", "\"%d\"", "\"%5.1f\"", "\"%q\""]);
                    let _ = writeln!(s, "__emit(pcall(string.format, {f}, O{o}))");
                }
                4 if self.objs > 1 => {
                    let (a, b) = (1 + self.rng.below(self.objs), 1 + self.rng.below(self.objs));
                    let _ = writeln!(
                        s,
                        "__emit(pcall(function() local x, y = O{a}, O{b} return x == y, x < y, x <= y, y > x, x .. y .. x end))"
                    );
                }
                5 if self.objs > 0 => {
                    let o = 1 + self.rng.below(self.objs);
                    let _ = writeln!(
                        s,
                        "__emit(pcall(function() local s = 0 for i, v in ipairs(O{o}) do s = s + 1 if i > 5 then break end end return s, #O{o} end))"
                    );
                }
                _ => {
                    let _ = writeln!(s, "__emit(pcall(function() return {e} end))");
                }
            }
        }
        s + "end)\n"
    }

    /// Metamethods and closures run inside a coroutine that yields from them.
    fn coro_meta_unit(&mut self) -> String {
        self.hit("unit:coro-meta");
        let mut s = self.object(true);
        let o = self.objs;
        let id = self.id();
        let _ = write!(s, "do local co = coroutine.wrap(function(...)\nlocal acc = {{...}}\n");
        for _ in 0..1 + self.rng.below(4) {
            let e = self.expr(2);
            let _ = match self.rng.below(6) {
                0 => writeln!(s, "__emit(\"in\", pcall(function() return {e} end))"),
                1 => writeln!(s, "do local c <close> = O{o} __emit(\"tbc\", O{o}.v) end"),
                2 => writeln!(s, "acc[#acc + 1] = function() return #acc, O{o}.x end __emit(acc[#acc]())"),
                3 => writeln!(
                    s,
                    "for i = 1, 2 do local k = i acc[#acc + 1] = function() k = k + 1 return k end end __emit(acc[#acc](), acc[#acc]())"
                ),
                4 => writeln!(s, "__emit(\"yv\", coroutine.yield({e}))"),
                _ => writeln!(s, "__emit(O{o} .. \"s\", -O{o}, O{o} < O{o}, O{o} <= O{o}, O{o}(1, 2))"),
            };
        }
        let _ = write!(
            s,
            "return \"done\"\nend)\nlocal r = table.pack(pcall(co, {id}))\nlocal n = 0\nwhile r[1] and r[2] ~= \"done\" and n < 40 do n = n + 1 __emit(\"yielded\", r[2]) r = table.pack(pcall(co, n % 3 == 0 and false or n)) end\n__emit(\"co-end\", r[1], r[2], n)\nend\n"
        );
        s
    }

    fn coro_closure_unit(&mut self) -> String {
        self.hit("unit:coro-closure");
        let id = self.id();
        let n = self.rng.range(1, 4);
        let body = match self.rng.below(7) {
            6 => format!("local p = setmetatable({{}}, {{__pairs = function(t) coroutine.yield(\"pairs\") return ipairs({{1, 2, {n}}}) end}}) local s = 0 for i, v in pairs(p) do s = s + v coroutine.yield(s) end return s"),
            0 => format!("local fs = {{}} for i = 1, {n} do local j = i * 10 fs[i] = function(d) j = j + (d or 1) return j end coroutine.yield(fs[i]) end return fs"),
            1 => "local x = 0 local function inc() x = x + 1 return x end while true do local v = coroutine.yield(inc) if v == nil then break end x = x + v end return x".into(),
            2 => format!("local t = setmetatable({{}}, {{__index = function(t, k) return coroutine.yield(k) end}}) local s = 0 for i = 1, {n} do s = s + (t[i] or 0) end return s"),
            3 => format!("local inner = coroutine.wrap(function() for i = 1, {n} do coroutine.yield(i) end return \"inner-done\" end) for i = 1, {n} + 1 do coroutine.yield(inner()) end return pcall(inner)"),
            4 => "local ok, e = pcall(function() local a <close> = setmetatable({}, {__close = function() coroutine.yield(\"closing\") end}) coroutine.yield(\"body\") end) return ok, e".into(),
            _ => format!("local function rec(k) if k == 0 then return coroutine.yield(\"bottom\") end return rec(k - 1) end return rec({n})"),
        };
        format!(
            "do local co = coroutine.create(function(a) {body} end)\nlocal got = {{}}\nfor i = 1, 8 do local r = table.pack(coroutine.resume(co, i <= 6 and i or nil)) local v = r[2] if type(v) == \"function\" then v = v(i) end __emit(\"r{id}\", r[1], v, r.n, coroutine.status(co)) if coroutine.status(co) == \"dead\" then break end end\n__emit(coroutine.close(co), coroutine.status(co))\nend\n"
        )
    }

    fn goto_unit(&mut self) -> String {
        self.hit("unit:goto");
        let id = self.id();
        let n = self.rng.range(1, 5);
        let s = match self.rng.below(5) {
            0 => format!(
                "do local fs, i = {{}}, 1\n::top:: local j = i fs[#fs + 1] = function() j = j + 1 return j, i end i = i + 1 if i <= {n} then goto top end\nfor _, f in ipairs(fs) do __emit(\"g{id}\", f()) end end\n"
            ),
            1 => format!(
                "for i = 1, {n} do local c <close> = setmetatable({{}}, {{__close = function() __emit(\"cl{id}\", i) end}}) if i % 2 == 0 then goto cont end __emit(\"body\", i) ::cont:: end\n"
            ),
            2 => format!(
                "do local k = 0 while true do k = k + 1 for j = 1, 3 do if j == 2 then goto next end __emit(\"w{id}\", k, j) end ::next:: if k >= {n} then goto out end end ::out:: __emit(\"out\", k) end\n"
            ),
            3 => {
                // Invalid gotos: the messages must agree (via load).
                let src = self.rng.pick(&[
                    "goto x local a ::x:: print(a)",
                    "do goto x end ::x:: ::x::",
                    "goto nowhere",
                    "do ::l:: end goto l",
                    "local x <const> = 1 x = 2",
                    "local x <close> = nil local y <close> = false return 1",
                    "local x <foo> = 1",
                    "for i = 1, 2 do ::c:: end goto c",
                    "repeat goto cont local z ::cont:: until z",
                    "repeat local z goto cont ::cont:: until z",
                    "::a:: ::b:: goto a",
                    "break",
                    "local a <close>, b <close> = 1, 2",
                    "do local t <close> = {} end",
                ]);
                format!("__emit(\"load{id}\", load({:?}))\n__emit(pcall(load({src:?}) or print))\n", src)
            }
            _ => format!(
                "do local r = {{}} for i = 1, {n} do do local x <close> = setmetatable({{}}, {{__close = function(_, e) r[#r + 1] = \"c\" .. i end}}) if i == 2 then goto skip end r[#r + 1] = i end ::skip:: end __emit(table.concat(r, \",\")) end\n"
            ),
        };
        s
    }

    fn close_unit(&mut self) -> String {
        self.hit("unit:close");
        let id = self.id();
        let h = |rng: &mut Rng, tag: &str| -> String {
            match rng.below(5) {
                0 => format!("setmetatable({{}}, {{__close = function(_, e) __emit(\"{tag}\", e) end}})"),
                1 => format!("setmetatable({{}}, {{__close = function(_, e) error(\"{tag}-err\") end}})"),
                2 => "nil".into(),
                3 => "false".into(),
                _ => format!(
                    "setmetatable({{}}, {{__close = function(_, e) __emit(\"{tag}\", e) error({{tag = \"{tag}\"}}) end}})"
                ),
            }
        };
        let (a, b, c) = (h(self.rng, "a"), h(self.rng, "b"), h(self.rng, "c"));
        let exit = self.rng.pick(&[
            "return 1, 2",
            "error(\"body\")",
            "error(nil)",
            "error(setmetatable({}, {__tostring = function() return \"E\" end}))",
            "",
            "break",
        ]);
        let wrapper = match self.rng.below(4) {
            0 => format!(
                "__emit(\"x{id}\", pcall(function() local a <close> = {a} local b <close> = {b} do local c <close> = {c} end {exit} end))"
            ),
            1 => format!(
                "__emit(\"x{id}\", pcall(function() for i = 1, 2 do local a <close> = {a} local b <close> = {b} if i == 2 then {exit} end end end))"
            ),
            2 => format!(
                "do local co = coroutine.create(function() local a <close> = {a} local b <close> = {b} coroutine.yield(1) {exit} end) __emit(coroutine.resume(co)) __emit(coroutine.close(co)) __emit(coroutine.status(co), coroutine.resume(co)) end"
            ),
            _ => format!(
                "__emit(\"x{id}\", pcall(function() local n = 0 for k, v, _, c in next, {{1, 2}}, nil, {a} do n = n + 1 if n == 1 then {exit} end end return n end))"
            ),
        };
        // `break` outside a loop does not compile: load it.
        if exit == &"break" && !wrapper.contains("for i") {
            return format!("__emit(load({wrapper:?}))\n");
        }
        wrapper + "\n"
    }

    fn string_unit(&mut self) -> String {
        self.hit("unit:string");
        let subj = *self.rng.pick(&[
            "\"hello world from lua\"",
            "string.rep(\"abcdefghij\", 5)",
            "\"\"",
            "\"a\\0b\\0c\"",
            "\"  x = 10, y = 20  \"",
            "\"(a(b)c)[d]\"",
            "\"THE END\"",
            "\"1,2,,3\"",
            "string.rep(\"\\255\", 41)",
            "\"x\" .. string.rep(\"y\", 39)",
        ]);
        let pat = *self.rng.pick(&[
            "\"%a+\"",
            "\"(%w)(%w*)\"",
            "\"()\"",
            "\"%f[%w]%w+\"",
            "\"%b()\"",
            "\"[^%s]+\"",
            "\"^(.-)$\"",
            "\"(a*(.)%w(%s*))\"",
            "\"%d?\"",
            "\".-\"",
            "\"[%a-z]\"",
            "\"[]]\"",
            "\"%z\"",
            "\"x*\"",
            "\"$\"",
            "\"(()a)\"",
            "\"%s*(%w+)%s*=%s*(%w+)\"",
            "\"([^,]*)\"",
            "\"%\"",
            "\"(\"",
            "\"[a\"",
            "\"%g+\"",
            "\"%1\"",
            "\"%f[%z]\"",
        ]);
        let n = self.rng.range(-3, 6);
        let call = match self.rng.below(14) {
            0 => format!("s:find({pat}, {n})"),
            1 => format!("s:find({pat}, {n}, true)"),
            2 => format!("s:match({pat}, {n})"),
            3 => format!(
                "s:gsub({pat}, {}, {n})",
                self.rng.pick(&[
                    "\"<%0>\"",
                    "\"%1-%2\"",
                    "\"%%\"",
                    "{a = 1, hello = false}",
                    "string.upper",
                    "function(a, b) return b end",
                    "\"%9\""
                ])
            ),
            4 => format!(
                "(function() local r = {{}} for a, b in s:gmatch({pat}) do r[#r + 1] = tostring(a) .. \":\" .. tostring(b) if #r > 30 then break end end return table.concat(r, \" \") end)()"
            ),
            5 => format!(
                "s:sub({n}, {}):upper():rep({}, \",\"):len()",
                self.rng.range(-8, 60),
                self.rng.range(0, 3)
            ),
            6 => format!("s:byte({n}, {})", self.rng.range(-3, 8)),
            7 => format!(
                "string.format({}, s, {})",
                self.rng.pick(&["\"%s|%10.3s|%-5s|%q\"", "\"%q %d\"", "\"%.20s %x\"", "\"%5s%%%5.2f\""]),
                self.num()
            ),
            8 => format!("#(s .. {} .. s), s < {}, s == s:sub(1)", self.num(), self.rng.pick(STRS)),
            9 => format!(
                "string.pack({}, {})",
                self.rng.pick(&[
                    "\"<i4\"",
                    "\">I2\"",
                    "\"z\"",
                    "\"s1\"",
                    "\"j\"",
                    "\"<d\"",
                    "\"!4 i1 i8\"",
                    "\"i3\""
                ]),
                self.operand()
            ),
            10 => format!("select('#', string.gmatch(s, {pat})())"),
            11 => format!("s:reverse():lower(), s:len(), tostring(s):rep(0)"),
            12 => format!("string.char(s:byte(1, -1))"),
            _ => format!(
                "utf8.len(s), pcall(utf8.codepoint, s, 1, -1), utf8.char({}, 0x10FFFF)",
                self.rng.range(0, 300)
            ),
        };
        format!("do local s = {subj} __emit(\"s{}\", pcall(function() return {call} end)) end\n", self.id())
    }

    fn number_unit(&mut self) -> String {
        self.hit("unit:number");
        let id = self.id();
        let (a, b) = (self.num(), self.num());
        let s = match self.rng.below(9) {
            0 => {
                let op = self.rng.pick(BINOPS);
                format!(
                    "local a, b = {a}, {b} local x for _ = 1, 2 do x = a {op} b end return x, math.type(x)"
                )
            }
            1 => {
                let op = self.rng.pick(BINOPS);
                format!("return {a} {op} {b}")
            }
            2 => {
                let step = self.rng.pick(&[
                    "1",
                    "-1",
                    "2",
                    "0.5",
                    "-0.25",
                    "math.maxinteger",
                    "math.mininteger",
                    "0.0",
                    "\"2\"",
                    "1e308",
                ]);
                format!(
                    "local n, f, l = 0 for i = {a}, {b}, {step} do n = n + 1 f = f or i l = i if n > 4 then break end end return n, f, l, math.type(l)"
                )
            }
            3 => {
                let f = self.rng.pick(&[
                    "math.tointeger",
                    "math.floor",
                    "math.ceil",
                    "math.abs",
                    "math.fmod",
                    "math.modf",
                    "math.ult",
                    "math.max",
                    "math.min",
                    "math.sqrt",
                    "math.exp",
                    "math.log",
                    "tostring",
                    "tonumber",
                    "string.format, \"%d\"",
                    "string.format, \"%.17g\"",
                    "string.format, \"%x\"",
                    "string.format, \"%5.2f\"",
                    "string.format, \"%i\"",
                    "string.format, \"%c\"",
                ]);
                format!("return pcall({f}, {a}, {b})")
            }
            4 => {
                // The Lua 5.3 compatibility functions (LUA_COMPAT_MATHLIB).
                let f = self.rng.pick(&[
                    "math.pow",
                    "math.ldexp",
                    "math.log10",
                    "math.cosh",
                    "math.sinh",
                    "math.tanh",
                    "math.frexp",
                ]);
                format!(
                    "local x = {a} if math.abs(x) == math.huge and {f:?} == \"math.frexp\" then x = 1 end return pcall({f}, x, {b})"
                )
            }
            5 => format!(
                "return tonumber({}, {})",
                self.rng.pick(STRS),
                self.rng.pick(&["2", "8", "10", "16", "36", "1", "37", "nil"])
            ),
            6 => format!(
                "local x = {a} return x == {b}, x < {b}, x <= {b}, {b} < x, x // 1 == x, math.type(x)"
            ),
            7 => format!(
                "local t = {{}} t[{a}] = 1 t[{b}] = 2 local c = 0 for k in __spairs(t) do c = c + 1 end return c, t[{a}], t[{b}]"
            ),
            _ => format!("return {a} .. \"\", {b} .. \"\", string.format(\"%.14g\", {a})"),
        };
        format!("__emit(\"n{id}\", pcall(function() {s} end))\n")
    }

    /// The Lua 5.3 compatibility `__le` (through `not __lt(b, a)`) with other events present
    /// or not.
    fn compat_le_unit(&mut self) -> String {
        self.hit("unit:compat-le");
        let id = self.id();
        let lt = self.rng.pick(&[
            "function(a, b) return __tag(a) < __tag(b) end",
            "function(a, b) return nil end",
            "function(a, b) error(\"lt\") end",
            "function(a, b) return 1 end",
        ]);
        let le = self.rng.pick(&["", "__le = function(a, b) return false end,", "__le = 5,"]);
        let (x, y) = (self.rng.range(1, 3), self.rng.range(1, 3));
        let other = self.rng.pick(&["1", "\"s\"", "{}", "B"]);
        format!(
            "do local mt = {{__lt = {lt}, {le}}} local A, B = setmetatable({{v = {x}}}, mt), setmetatable({{v = {y}}}, mt)\n__emit(\"le{id}\", pcall(function() return A <= B, B >= A, A > B, not (A <= {other}) end))\nend\n"
        )
    }

    /// Library functions neither generator named (the `coverage` report), over proxies
    /// and edge arguments.
    fn lib_unit(&mut self) -> String {
        self.hit("unit:lib");
        let id = self.id();
        let (a, b) = (self.num(), self.num());
        let o = if self.objs > 0 { format!("O{}", 1 + self.rng.below(self.objs)) } else { "{}".into() };
        let call = match self.rng.below(14) {
            0 => format!("{}({a})", self.rng.pick(&["math.sin", "math.cos", "math.tan", "math.asin", "math.acos", "math.atan", "math.deg", "math.rad"])),
            1 => format!("math.atan({a}, {b}), math.atan2 and math.atan2({a}, {b})"),
            2 => format!("math.max({a}, {b}, {}), math.min({a}, {b})", self.num()),
            3 => format!("assert({}, {})", self.operand(), self.operand()),
            4 => format!("xpcall(function() return {} end, {}, {a})", self.expr(2), self.rng.pick(&["function(m) return \"h:\" .. tostring(m) end", "error", "print", "function(m) error(m) end", "function(m) return m, 2 end"])),
            5 => "coroutine.running(), coroutine.isyieldable(), coroutine.wrap(function() return coroutine.isyieldable(), select(2, coroutine.running()) end)()".into(),
            6 => {
                let f = self.rng.pick(&["\"<i4\"", "\">I2 z\"", "\"s1 b\"", "\"j n\"", "\"<d f\"", "\"!4 i1 i8\"", "\"i3 x\"", "\"c3\"", "\"<i16\""]);
                format!("string.packsize({f}), string.unpack({f}, string.rep(\"\\x81\", 24), {})", self.rng.range(-3, 5))
            }
            7 => format!("table.unpack({o}, 1, 3), table.concat({{1, 2, 3}}, \",\", {}, {})", self.rng.range(-1, 3), self.rng.range(0, 4)),
            // (Plain tables: a proxy whose `__len` is huge keeps C Lua in `table.remove`
            // with no hook to stop it.)
            8 => format!("(function() local t = {{1, 2, 3}} return table.insert(t, {a}), rawlen(t), table.remove(t), table.remove(t, 1), rawlen({o}) end)()"),
            9 => format!("table.move({o}, 1, 3, {}, {{}})[2], table.move({{1, 2, 3}}, 1, 3, 2, {o})[3]", self.rng.range(-1, 4)),
            10 => format!("(function() local t = {{{a}, {b}, 3, 1}} table.sort(t, {}) return t[1], t[4] end)()", self.rng.pick(&["nil", "function(x, y) return x > y end", "function(x, y) return true end", "function(x, y) return O1 < x end"])),
            11 => "(function() local r = {} for p, c in utf8.codes(\"h\\u{e9}llo\\u{10000}\") do r[#r + 1] = p .. \"=\" .. c end return table.concat(r, \" \") end)(), utf8.offset(\"h\\u{e9}llo\", 3), utf8.offset(\"ab\", -1)".into(),
            12 => format!("os.difftime({}, {}), type(os.clock()), collectgarbage(\"isrunning\"), type(collectgarbage(\"count\"))", self.rng.range(0, 9), self.rng.range(0, 9)),
            _ => format!("select({}, \"a\", \"b\", \"c\")", self.rng.pick(&["1", "-1", "-3", "-4", "0", "\"#\"", "2.0", "4"])),
        };
        format!("__emit(\"l{id}\", pcall(function() return {call} end))\n")
    }

    /// One focus program: a few objects, then units.
    pub(crate) fn program(&mut self, units: usize) -> String {
        let mut s = String::new();
        for _ in 0..self.rng.range(0, 2) {
            s += &self.object(false);
        }
        for _ in 0..units {
            let u = match self.rng.weighted(&[5, 3, 3, 2, 3, 4, 4, 1, 3]) {
                8 => self.lib_unit(),
                0 => self.meta_unit(),
                1 => self.coro_meta_unit(),
                2 => self.coro_closure_unit(),
                3 => self.goto_unit(),
                4 => self.close_unit(),
                5 => self.string_unit(),
                6 => self.number_unit(),
                _ => self.compat_le_unit(),
            };
            s += &u;
        }
        s + "return \"end\"\n"
    }
}

// ---------------------------------------------------------------- drivers

#[test]
fn focus_quick() {
    // Valid Lua (C Lua compiles it), no panic or hang, and the same output three ways.
    let mut rec = Recorder::new("s4quick");
    for i in 0..env_u64("SWEEP4_QUICK", 6) {
        let mut rng = Rng::new(0x5_4000 + i);
        let p = Focus::new(&mut rng).program(12);
        let theirs = harness::oracle(p.as_bytes());
        assert_ne!(theirs.class, Class::Syntax, "generator emitted invalid Lua:\n{p}\n{}", theirs.text);
        let mut r2 = Rng::new(i);
        handle_plain(&p, &mut rec, &mut r2, i % 2 == 0);
    }
    assert!(rec.sigs.is_empty(), "differences: {:?} (programs in {})", rec.sigs, rec.dir.display());
}

#[test]
#[ignore = "sweep4: fuzz driver (long); SWEEP4_ITERS/SWEEP4_SEED/SWEEP4_SECS"]
fn fuzz_focus() {
    let (iters, secs, seed) =
        (env_u64("SWEEP4_ITERS", 300), env_u64("SWEEP4_SECS", 600), env_u64("SWEEP4_SEED", 1));
    let started = std::time::Instant::now();
    let mut rec = Recorder::new("s4focus");
    let mut hits = std::collections::BTreeMap::new();
    for i in 0..iters {
        if started.elapsed().as_secs() > secs {
            break;
        }
        let mut rng = Rng::new(seed.wrapping_mul(0x2545_F491).wrapping_add(i));
        let mut f = Focus::new(&mut rng);
        let p = f.program(6 + (i % 20) as usize);
        for (k, v) in &f.hits {
            *hits.entry(*k).or_insert(0u64) += v;
        }
        let mut r2 = Rng::new(seed ^ i ^ 0x44);
        handle_plain(&p, &mut rec, &mut r2, true);
    }
    rec.report(started);
    println!("seed {seed}; units: {hits:?}");
}

#[test]
#[ignore = "sweep4: fuzz driver (long); SWEEP4_ITERS/SWEEP4_SEED/SWEEP4_SECS"]
fn fuzz_focus_mutated() {
    let (iters, secs, seed) =
        (env_u64("SWEEP4_ITERS", 300), env_u64("SWEEP4_SECS", 600), env_u64("SWEEP4_SEED", 1));
    let started = std::time::Instant::now();
    let mut pool = super::sweep1_fuzz::conformance_corpus();
    for i in 0..300 {
        let mut rng = Rng::new(seed.wrapping_add(0x77_0000 + i));
        pool.push(Focus::new(&mut rng).program(6));
    }
    let mut rng = Rng::new(seed ^ 0x5eed_4444);
    let mut rec = Recorder::new("s4mut");
    for _ in 0..iters {
        if started.elapsed().as_secs() > secs {
            break;
        }
        // Mostly mutate focus programs (the last 300).
        let k = if rng.pct(75) { pool.len() - 1 - rng.below(300) } else { rng.below(pool.len()) };
        let m = mutate::mutate(&pool[k], &mut rng, &pool);
        let mut r2 = Rng::new(rng.next());
        handle_plain(&m, &mut rec, &mut r2, rng.pct(25));
    }
    rec.report(started);
    println!("seed {seed}");
}

#[cfg(feature = "front-oracle")]
#[test]
fn focus_front_equiv() {
    use remedy_lua::__front::{compile_new, compile_old, parse_new, parse_old};
    let iters = env_u64("SWEEP4_FRONT_ITERS", 60);
    let seed = env_u64("SWEEP4_SEED", 1);
    let limit = |e: &String| e.contains("too many syntax levels") || e.contains("expression too complex");
    let mut pool = vec![];
    for i in 0..iters {
        let mut rng = Rng::new(seed.wrapping_mul(31).wrapping_add(0xf_0000 + i));
        pool.push(Focus::new(&mut rng).program(10));
    }
    let mut rng = Rng::new(seed ^ 0xfe);
    let mut progs = pool.clone();
    for _ in 0..iters * 3 {
        let base = pool[rng.below(pool.len())].clone();
        progs.push(mutate::mutate(&base, &mut rng, &pool));
    }
    let (mut ok, mut errs) = (0, 0);
    for p in &progs {
        let b = p.as_bytes();
        let old = parse_old(b, "f.lua");
        if old.as_ref().is_err_and(limit) {
            let _ = compile_new(b, "f.lua");
            continue;
        }
        assert_eq!(old, parse_new(b, "f.lua"), "parse differs:\n{p}");
        if old.is_err() {
            errs += 1;
            continue;
        }
        let old = compile_old(b, "f.lua");
        if old.as_ref().is_err_and(limit) {
            continue;
        }
        assert_eq!(old, compile_new(b, "f.lua"), "compile differs:\n{p}");
        ok += 1;
    }
    println!("{ok} compiled the same, {errs} failed the same");
    assert!(ok > 0);
}

/// Opcodes compiled (front-oracle's bytecode dump) and library functions named, for the
/// sweep 1 grammar and the focus generator.
#[cfg(feature = "front-oracle")]
#[test]
#[ignore = "sweep4: coverage report"]
fn coverage() {
    use super::sweep1_fuzz::grammar;
    use remedy_lua::__front::compile_new;
    const OPS: &[&str] = &[
        "Move",
        "LoadK",
        "LoadInt",
        "LoadBool",
        "LoadFalseSkip",
        "LoadNil",
        "GetUpval",
        "SetUpval",
        "GetTabUp",
        "SetTabUp",
        "GetTable",
        "GetTableK",
        "SetTable",
        "SetTableK",
        "NewTable",
        "SelfK",
        "Arith",
        "Add",
        "AddK",
        "Sub",
        "SubK",
        "Mul",
        "MulK",
        "Div",
        "DivK",
        "Mod",
        "ModK",
        "IDiv",
        "IDivK",
        "Unm",
        "BNot",
        "Not",
        "Len",
        "Concat",
        "ToStr",
        "Jmp",
        "Eq",
        "Lt",
        "Le",
        "Test",
        "TestSet",
        "Call",
        "TailCall",
        "Return",
        "ForPrep",
        "ForLoop",
        "TForPrep",
        "TForCall",
        "TForLoop",
        "SetList",
        "Closure",
        "VarArg",
        "Close",
        "Tbc",
    ];
    // Library functions, from a fresh state.
    let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
    let names = lua
        .exec("local o = {} for _, l in ipairs{'string','table','math','coroutine','utf8','os'} do for k, v in pairs(_G[l]) do if type(v) == 'function' then o[#o+1] = l .. '.' .. k end end end for k, v in pairs(_G) do if type(v) == 'function' then o[#o+1] = k end end table.sort(o) return table.concat(o, ' ')", "=n")
        .unwrap();
    let names: Vec<String> = lua.to_string_lossy(names[0]).unwrap().split(' ').map(str::to_owned).collect();
    let n = env_u64("SWEEP4_COV_ITERS", 300);
    for which in ["grammar", "focus"] {
        let mut ops: std::collections::BTreeMap<&str, u64> = OPS.iter().map(|o| (*o, 0)).collect();
        let mut libs: std::collections::BTreeMap<&str, u64> = names.iter().map(|s| (s.as_str(), 0)).collect();
        for i in 0..n {
            let mut rng = Rng::new(0xc0 + i);
            let p = if which == "grammar" {
                grammar::Gen::new(&mut rng, false).program(10 + (i % 40) as i32).l
            } else {
                Focus::new(&mut rng).program(12)
            };
            if let Ok(bc) = compile_new(p.as_bytes(), "c.lua") {
                for tok in bc.split(|c: char| !c.is_ascii_alphanumeric()) {
                    if let Some(c) = ops.get_mut(tok) {
                        *c += 1;
                    }
                }
            }
            for (name, c) in libs.iter_mut() {
                let short = name.rsplit('.').next().unwrap();
                // `lib.f(`, `:f(` (string methods), or a bare global `f(`.
                if p.contains(&format!("{name}("))
                    || p.contains(&format!("{name},"))
                    || (name.starts_with("string.") && p.contains(&format!(":{short}(")))
                {
                    *c += 1;
                }
            }
        }
        let zero_ops: Vec<_> = ops.iter().filter(|(_, c)| **c == 0).map(|(k, _)| *k).collect();
        let zero_libs: Vec<_> = libs.iter().filter(|(_, c)| **c == 0).map(|(k, _)| *k).collect();
        println!(
            "== {which}: {n} programs\nops: {ops:?}\nopcodes never compiled: {zero_ops:?}\nlibrary functions never named: {zero_libs:?}\n"
        );
    }
}

// ---------------------------------------------------------------- regressions

/// Runs `src` in RemedyLua (fast loop on and off, which must agree) and C Lua, same wrapper.
fn same_as_c(src: &str) {
    let ours = harness::remedy(src.as_bytes(), "=t");
    let theirs = harness::oracle(src.as_bytes());
    assert_eq!(ours.text, theirs.text, "RemedyLua vs C Lua on:\n{src}");
}

/// S4FUZZ-1: the `gmatch` iterator returned one `nil` when nothing more matched; C Lua
/// returns no values.
#[test]
fn s4fuzz_1_gmatch_end_returns_nothing() {
    same_as_c(
        "local it = string.gmatch('abc', 'x') __emit(select('#', it()))\nlocal it2 = ('ab'):gmatch('.') it2() it2() __emit(select('#', it2()), select('#', it2()))\nreturn select('#', string.gmatch(string.rep('a', 50), 'b')())",
    );
}

/// S4FUZZ-3: `table.move` into another table compares the two with `__eq` (C Lua's
/// `lua_compare`) when the ranges overlap; a raw compare skipped the metamethod.
#[test]
fn s4fuzz_3_move_compares_with_eq() {
    same_as_c(
        "local mt = {__eq = function() error('eq called') end}\nlocal A, B = setmetatable({1, 2, 3}, mt), setmetatable({}, mt)\n__emit(pcall(table.move, A, 1, 3, 2, B))\n__emit(pcall(table.move, A, 1, 3, 5, B))\n__emit(pcall(table.move, A, 1, 3, 2))\nlocal C = setmetatable({}, {__eq = {}}) __emit(pcall(table.move, {1, 2, 3}, 1, 3, 2, C))\nlocal mt2 = {__eq = function() return true end} local X, Y = setmetatable({1, 2, 3}, mt2), setmetatable({}, mt2)\ntable.move(X, 1, 3, 2, Y) __emit(Y[2], Y[3], Y[4])",
    );
}

/// S4FUZZ-4: `table.move` checks its integers before its tables, as C Lua.
#[test]
fn s4fuzz_4_move_argument_order() {
    same_as_c(
        "__emit(pcall(table.move, 3, -1, {}))\n__emit(pcall(table.move, 1, 2, 3, 4))\n__emit(pcall(table.move, {}, 1, 2, 3, 4))\n__emit(pcall(table.move, nil, 1, 'x', 3))",
    );
}

/// S4FUZZ-5: names in error messages end at a zero byte (C Lua prints them with `%s`).
#[test]
fn s4fuzz_5_names_end_at_zero_byte() {
    same_as_c(
        "__emit(pcall(function() return '\\0' >> {} end))\n__emit(pcall(function() return 'a\\0b' >> {} end))\n__emit(pcall(function() local t = {} return t['x\\0y']() end))\n__emit(pcall(function() return _G['g\\0h']() end))",
    );
}

/// S4FUZZ-6: `__pairs` may yield (C Lua calls it with `lua_callk`); its results are cut to
/// three, and its error levels count `pairs` as a C function. (Still open: a yield from
/// `__pairs` under `pcall(pairs, t)`.)
#[test]
fn s4fuzz_6_pairs_metamethod_yields() {
    same_as_c(
        "local Y = setmetatable({}, {__pairs = function(t) local r = coroutine.yield('p') return next, {10, 20}, nil, 'extra' end})
local co = coroutine.wrap(function() for k, v in pairs(Y) do __emit('it', k, v) end return 'done' end) __emit(co()) __emit(co())
local co2 = coroutine.wrap(function() local a, b, c, d = pairs(Y) return select('#', a, b, c, d), type(a), d end) __emit(co2()) __emit(co2())
local co3 = coroutine.wrap(function() return pairs(Y) end) __emit(co3()) __emit(select('#', co3()))
local M = setmetatable({}, {__pairs = function(t) return 1, 2, 3, 4, 5 end})
__emit(pairs(M)) __emit(select('#', pairs(M))) local a, b, c, d, e = pairs(M) __emit(a, b, c, d, e)
__emit(pcall(pairs, setmetatable({}, {__pairs = 5})))
__emit(pcall(function() local a, b, c = pairs(setmetatable({}, {__pairs = 5})) end))
__emit(pcall(function() for k in pairs(setmetatable({}, {__pairs = function() error('lvl', 2) end})) do end end))
__emit(pcall(function() for k in pairs(setmetatable({}, {__pairs = function() error('lvl1') end})) do end end))
__emit(pcall(function() local x = pairs(setmetatable({}, {__pairs = function() error('lvl', 3) end})) end))
__emit(pcall(function() return pairs(setmetatable({}, {__pairs = setmetatable({}, {__call = function(s, t) return 7 end})})) end))
__emit(pcall(function() for k in pairs(setmetatable({}, {__pairs = function() return nil end})) do end end))
local T = setmetatable({}, {__pairs = function(t) return ipairs({7, 8}) end}) for i, v in pairs(T) do __emit('tail', i, v) end
local a, b, c, d = pairs(T) __emit(type(a), b[2], c, d)
local U = setmetatable({}, {__pairs = function(t) return table.unpack({next, {4}, nil, 'x', 'y'}, 1, 5) end})
__emit(pcall(function() for k, v in pairs(U) do __emit('u', k, v) end end)) __emit(select('#', pairs(U)))
local W = setmetatable({}, {__pairs = function(t) return coroutine.yield(next, {5}, nil, 'x') end})
local co4 = coroutine.wrap(function() for k, v in pairs(W) do __emit('yk', k, v) end return 'ok' end) local f, s, i, x = co4() __emit(x) __emit(co4(f, s, i, x))",
    );
}

/// S4FUZZ-7 (open, major): a `__close` run while an error unwinds to a `pcall` inside a
/// coroutine may yield in C Lua (`finishpcallk` closes with yields allowed); RemedyLua
/// closes there from Rust (`close_with_error`), so the yield fails.
#[test]
fn s4fuzz_7_close_yields_while_unwinding() {
    same_as_c(
        "local co = coroutine.wrap(function() return pcall(function() local a <close> = setmetatable({}, {__close = function() coroutine.yield('closing') end}) error('boom') end) end)\n__emit(co()) __emit(co())",
    );
}

/// The other yield case of S4FUZZ-7's family: a Lua `__pairs` called as `pcall(pairs, t)`
/// runs as a protected frame of its own, so it may yield (and its results are still cut to
/// three; its errors are caught by the `pcall`).
#[test]
fn s4fuzz_7_pairs_yields_under_pcall() {
    same_as_c(
        "local t = setmetatable({}, {__pairs = function(t) coroutine.yield('y') return next, t, 7 end})
local co = coroutine.wrap(function() local ok, f, s, c = pcall(pairs, t) return ok, f == next, s == t, c end)
__emit(co()) __emit(co())
__emit(pcall(pairs, setmetatable({}, {__pairs = function() error('bad') end})))
__emit(select('#', pcall(pairs, setmetatable({}, {__pairs = function() return 1, 2, 3, 4, 5 end}))))",
    );
}

/// S4FUZZ-8: string arithmetic that does not convert falls to the other operand's
/// metamethod, called by the string library (C `trymt`, `lua_call`): error levels count
/// the library function, a value that is not callable has no position or event name, and
/// a yield there fails.
#[test]
fn s4fuzz_8_string_arith_fallback_called_from_library() {
    same_as_c(
        "local O1 = setmetatable({}, {__mul = function() error('lvl5', 2) end, __add = 'str', __sub = function(a, b) return coroutine.yield('y') end, __div = function(a, b) return a, b end})
__emit(pcall(function() return 'abc' * O1 end)) __emit(pcall(function() return 'inf' + O1 end)) __emit(pcall(function() return O1 + 'inf' end))
__emit(pcall(function() return 1 + O1 end)) __emit(pcall(function() return 'x' / O1 end))
__emit(pcall(coroutine.wrap(function() return 'abc' - O1 end))) __emit(pcall(coroutine.wrap(function() return O1 - 'abc' end)))
__emit(pcall(function() return getmetatable('').__mul('abc', O1) end))",
    );
}

/// S4FUZZ-11: "variable 'c' got a non-closable value" is reported at the line of the
/// declaration's last token (C Lua's), not the line of the name.
#[test]
fn s4fuzz_11_tbc_error_line() {
    same_as_c(
        "local O = {}\n__emit(pcall(function() local c <close>\n= O return 1 end))\n__emit(pcall(function() local a, c <close> = 1,\n\nO end))\n__emit(pcall(function() local c <close> = 5 end))",
    );
}

/// S4FUZZ-12: a `table.sort` that fails in a comparison leaves a plain table as far as
/// the sort got (C Lua sorts in place); it was left untouched.
#[test]
fn s4fuzz_12_failed_sort_keeps_progress() {
    same_as_c(
        "for _, t in ipairs{{5, nil, 12, -8}, {3, 1, 'x', 2, 9, 7, 4}, {1, 2, 3, 'a', 5, 6, 7, 8, 9, 10, 11, 12}} do __emit(pcall(table.sort, t)) __emit(t) end\nlocal t = {5, 3, 1, 4, 2} __emit(pcall(table.sort, t, function(a, b) if a == 1 and b == 4 then error('cmp') end return a < b end)) __emit(t)\nlocal t2 = {5, 3, 1, 4, 2} __emit(pcall(table.sort, t2, function(a, b) return true end)) __emit(t2)",
    );
}

/// S4FUZZ-13: after a memory (or other) limit, `fuel_left` reported 0, so "fuel used"
/// was the whole budget, not the steps that ran. Exact now: with one unit less the same
/// program stops on fuel. (The harness checks the fast loop and the full path agree.)
#[test]
fn s4fuzz_13_fuel_used_is_exact_at_a_memory_error() {
    let src = "local r = {} for i = 1, 2^60 do r[#r + 1] = math.type(i) end";
    let mem = 1 << 20;
    let (o, used) = harness::remedy_raw(src.as_bytes(), "=t", harness::remedy_limits().memory(mem));
    assert_eq!(o.class, Class::Memory, "{}", o.text);
    assert!(used < harness::remedy_limits().fuel / 2, "fuel used {used}: the whole budget, not what ran");
    for d in [1, 2, 10, 1000] {
        let (o2, u2) =
            harness::remedy_raw(src.as_bytes(), "=t", harness::remedy_limits().memory(mem).fuel(used - d));
        assert_eq!((o2.class, u2), (Class::Fuel, used - d), "fuel {} of {used}: {}", used - d, o2.text);
    }
}

/// S4FUZZ-9: `a .. (b .. c)` is one concatenation, as C Lua's `codeconcat` merges it, so
/// an error after a `__concat` names the variable; a jump into the end keeps them apart.
/// Also `and`/`or` with a local on the left (`TestSet`, as C Lua).
#[test]
fn s4fuzz_9_right_nested_concat_is_one_instruction() {
    same_as_c(
        "local O1 = setmetatable({}, {__concat = function() end})
__emit(pcall(function() return 'x' .. (O1 .. 1) end))
__emit(pcall(function() local u = O1 return 'a' .. (u .. 2) end))
__emit(pcall(function() return 'a' .. (O1 .. 1) .. 'z' end))
__emit(pcall(function() return ('a' .. 'b') .. ('c' .. ('d' .. 'e')) .. 'f' end))
__emit(pcall(function() local x = false return 'a' .. (x and ('b' .. 'c')) end))
__emit(pcall(function() local x = true return 'a' .. (x and ('b' .. 'c')) end))
__emit(pcall(function() local x = nil return 'a' .. (x or ('b' .. 'c')) end))
__emit(pcall(function() local s = 'q' for i = 1, 3 do s = s .. (i .. '-' .. (i * 2)) end return s end))
__emit(pcall(function() local a, b = nil, 2 local x = a or b local y = a and b local z = (b) or a return x, y, z end))
__emit(pcall(function() local a, b = 1, 2 a = a or b b = a and nil return a, b end))
__emit(pcall(function() local a = nil return (a or a)() end))
__emit(pcall(function() local a = nil local t = {} return (a or t.x)() end))",
    );
}

/// S4FUZZ-14: `table.sort` of a table with a metatable sorts it in place, as C Lua: every
/// `__index`/`__newindex` in C Lua's order, a comparator sees the table as it is, and a
/// failed sort leaves the order it reached.
#[test]
fn s4fuzz_14_proxy_sort_in_place() {
    same_as_c(
        "local function proxy(src)
  local log = {}
  local p = setmetatable({}, {__index = function(_, k) log[#log + 1] = 'g' .. k return src[k] end,
    __newindex = function(_, k, v) log[#log + 1] = 's' .. k .. '=' .. tostring(v) src[k] = v end, __len = function() return #src end})
  return p, log, src
end
for _, data in ipairs{{5, 3, 1, 4, 2}, {5, 3, 'x', 4, 2, 9, 7}, {9, 8, 7, 6, 5, 4, 3, 2, 1}, {3, 1, 2}, {2, 1}} do
  local p, log, src = proxy(data) __emit(pcall(table.sort, p)) __emit(table.concat(log, ' ')) __emit(src)
end
local p, log, src = proxy({5, 3, 1, 4, 2, 8, 7})
__emit(pcall(table.sort, p, function(a, b) if a == 1 and b == 4 then error('cmp') end return a < b end)) __emit(table.concat(log, ' ')) __emit(src)
local p2, log2, src2 = proxy({5, 3, 1, 4, 2}) __emit(pcall(table.sort, p2, function(a, b) return true end)) __emit(table.concat(log2, ' '))
local t = setmetatable({4, 3, 2, 1}, {}) local seen = {}
__emit(pcall(table.sort, t, function(a, b) seen[#seen + 1] = table.concat(t, '') return a < b end)) __emit(table.concat(seen, ' '))",
    );
}

/// S4FUZZ-15: `table.sort` of a plain table is in place too: a comparator that reads the
/// table sees its current order, as C Lua's; values the sort holds stay alive when the
/// comparator removes them from the table (checked with a collection at every allocation).
#[test]
fn s4fuzz_15_plain_sort_in_place() {
    same_as_c(
        "local t = {4, 3, 2, 1, 7, 6, 5} local seen = {}
__emit(pcall(table.sort, t, function(a, b) seen[#seen + 1] = table.concat(t, '') return a < b end)) __emit(table.concat(seen, ' ')) __emit(t)
local t5 = {} for i = 1, 50 do t5[i] = (i * 37) % 51 end
__emit(pcall(table.sort, t5, function(a, b) if a == 7 then t5[3] = nil end return a < b end)) __emit(t5[1], t5[2], #t5)
local t6 = {3, 2, 1} __emit(pcall(table.sort, t6, function(a, b) setmetatable(t6, {__index = function() return 0 end}) return a < b end)) __emit(t6)",
    );
    let src = "local t = {} for i = 1, 200 do t[i] = {v = (i * 37) % 201} end
table.sort(t, function(a, b) for i = 1, #t do if t[i] == a then t[i] = {v = a.v} break end end local x = {} return a.v < b.v end)
local ok = 0 for i = 1, #t do if type(t[i].v) == 'number' then ok = ok + 1 end end return ok";
    for fast in [true, false] {
        let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
        lua.set_fast_loop(fast);
        lua.set_gc_stress(true);
        let r = lua.exec(src, "=t");
        assert!(matches!(r.as_deref(), Ok([remedy_lua::Value::Int(200)])), "{r:?}");
    }
}

/// S4FUZZ-2: a `__close` that is not callable (a number, a table without `__call`), on a
/// normal exit: C Lua's message has the position and names the event.
#[test]
fn s4fuzz_2_uncallable_close_message() {
    same_as_c(
        "local O = setmetatable({}, {__close = 42})\n__emit(pcall(function() do local c <close> = O end end))\n__emit(pcall(function() local c <close> = O return 1 end))\n__emit(pcall(function() local c <close> = O error('x') end))\n__emit(pcall(coroutine.wrap(function() do local c <close> = O end end)))\nlocal mt = {__close = function() end} local P = setmetatable({}, mt)\n__emit(pcall(function() local c <close> = P mt.__close = {} end))\n__emit(pcall(function() for k in next, {1}, nil, O do break end end))",
    );
}
