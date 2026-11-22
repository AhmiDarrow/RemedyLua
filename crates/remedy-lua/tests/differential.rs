//! Differential testing against the reference C Lua: random programs (expressions over
//! every value type and operator, string and math library calls, tables, metatables,
//! closures, coroutines, error handling) run in both, and their serialized results must
//! be equal.
//!
//! The default run is short. For a long run:
//! `REMEDYLUA_FUZZ_ITERS=100000 REMEDYLUA_FUZZ_SEED=7 cargo test --test differential -- --nocapture`.
//! A failing program is printed in full, with its seed, so it can be replayed with
//! `REMEDYLUA_FUZZ_ONLY=<seed>`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

/// A small deterministic generator (xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

const LITERALS: &[&str] = &[
    "0", "1", "-1", "2", "3", "7", "10", "-7", "255", "1000000", "math.maxinteger", "math.mininteger", "0.0", "-0.0",
    "0.5", "1.5", "-2.25", "3.0", "1e10", "1e300", "2^53", "math.huge", "-math.huge", "0.1", "1/3", "\"10\"",
    "\"0x1F\"", "\"1e2\"", "\"abc\"", "\"\"", "\" 5 \"", "\"3.5\"", "true", "false", "nil", "a", "b", "c", "s", "t",
    "t[1]", "#t", "t.x",
];

const BINOPS: &[&str] =
    &["+", "-", "*", "/", "//", "%", "^", "&", "|", "~", "<<", ">>", "..", "==", "~=", "<", "<=", ">", ">=", "and", "or"];

const CALLS: &[&str] = &[
    "(type(%) == 'table' and 'table' or tostring(%))", "tonumber(%)", "type(%)", "math.type(%)", "math.floor(%)", "math.ceil(%)", "math.abs(%)",
    "math.tointeger(%)", "math.max(%, 2)", "math.min(%, 2)", "math.fmod(%, 3)", "string.len(%)", "string.upper(%)",
    "string.rep(%, 2)", "string.format('%d', %)", "string.format('%5.2f', %)", "string.format('%g', %)",
    "string.format('%q', %)", "string.format('%s', %)", "string.format('%x', %)", "select('#', %)", "(%)",
    "string.sub('hello world', %)", "string.byte('hello', %)", "string.char(%)", "('x'):rep((%) % 64)",
    "string.find('hello world', %)", "string.match('key=123', %)", "string.gsub('hello', 'l', %)",
    "utf8.char(%)", "math.ult(%, 5)", "-(%)", "not (%)", "~(%)", "#(%)", "rawlen(%)", "rawequal(%, 1)",
    "string.reverse(%)", "table.concat({1, 2, %}, ',')", "select(2, pcall(error, %))", "string.pack('i4', %)",
];

fn expr(r: &mut Rng, depth: usize) -> String {
    if depth == 0 || r.chance(30) {
        return r.pick(LITERALS).to_owned();
    }
    match r.below(10) {
        0..=4 => format!("({} {} {})", expr(r, depth - 1), r.pick(BINOPS), expr(r, depth - 1)),
        5..=7 => r.pick(CALLS).replace('%', &expr(r, depth - 1)),
        8 => format!("({} and {} or {})", expr(r, depth - 1), expr(r, depth - 1), expr(r, depth - 1)),
        _ => format!("{{{}, {}}}", expr(r, depth - 1), expr(r, depth - 1)),
    }
}

/// A program of protected expression evaluations.
fn expr_program(r: &mut Rng) -> String {
    let mut p = String::from(
        "local a, b, c, s = 3, 2.5, -4, 'str'\nlocal t = {10, 20, 30, x = 'y'}\nlocal out = {}\n\
         local function try(f) local r = table.pack(pcall(f)) r.n = nil return r end\n",
    );
    for _ in 0..(5 + r.below(10)) {
        p.push_str(&format!("out[#out + 1] = try(function() return {} end)\n", expr(r, 3)));
    }
    p.push_str("return out");
    p
}

/// Tables, metatables, closures and coroutines from templates with random parts.
fn structure_program(r: &mut Rng) -> String {
    let n = 1 + r.below(40);
    let k = 1 + r.below(7);
    let op = r.pick(&["+", "-", "*", "//", "%", "..", "<", "<=", "=="]);
    let mm = r.pick(&["__add", "__sub", "__mul", "__idiv", "__mod", "__concat", "__lt", "__le", "__eq"]);
    match r.below(8) {
        0 => format!(
            "local t = {{}}\nfor i = 1, {n} do t[i] = (i * {k}) % 11 end\n\
             for i = 1, {n}, {k} do t[i] = nil end\nlocal c = 0\nfor _ in pairs(t) do c = c + 1 end\n\
             local keys = {{}} for i, v in ipairs(t) do keys[#keys + 1] = v end\n\
             table.sort(keys)\nreturn c, keys, select('#', table.unpack(keys))"
        ),
        1 => format!(
            "local mt = {{{mm} = function(x, y) local a = type(x) == 'table' and x.v or x local b = type(y) == 'table' and y.v or y return a {op} b end}}\n\
             local p, q = setmetatable({{v = {n}}}, mt), setmetatable({{v = {k}}}, mt)\n\
             local r = {{}}\nr[1] = {{pcall(function() return p {op} q end)}}\nr[2] = {{pcall(function() return p {op} {k} end)}}\n\
             r[3] = {{pcall(function() return {n} {op} q end)}}\nreturn r"
        ),
        2 => format!(
            "local fs = {{}}\nfor i = 1, {n} do local j = i * {k} fs[i] = function(d) j = j + d return j end end\n\
             local s = 0\nfor i = 1, #fs do s = s + fs[i](i) + fs[i](1) end\nreturn s, #fs"
        ),
        3 => format!(
            "local function gen(n) return coroutine.wrap(function() for i = 1, n do coroutine.yield(i * {k}) end return 'done' end) end\n\
             local g = gen({n})\nlocal out = {{}}\nfor i = 1, {n} + 1 do out[i] = g() end\n\
             local co = coroutine.create(function(a) local b = coroutine.yield(a + 1) error({{code = b}}) end)\n\
             local r1 = {{coroutine.resume(co, {k})}}\nlocal r2 = {{coroutine.resume(co, {n})}}\n\
             return out, r1, r2[1], r2[2].code, coroutine.status(co), {{coroutine.resume(co)}}"
        ),
        4 => format!(
            "local words = {{}}\nfor w in ('the quick brown fox jumps over the lazy dog'):gmatch('%a+') do words[#words + 1] = w end\n\
             table.sort(words, function(x, y) if #x ~= #y then return #x < #y end return x < y end)\n\
             local s = table.concat(words, ' ', 1, math.min(#words, {k}))\n\
             return s, (s:gsub('(%w+)', '<%1>')), s:rep({k} % 3, ','), s:find('o', {n} % 10), string.format('%-{n}s|', s)"
        ),
        5 => format!(
            "local log = {{}}\nlocal function c(n) return setmetatable({{}}, {{__close = function(_, e) log[#log + 1] = n .. tostring(e) end}}) end\n\
             local ok, err = pcall(function()\n  local x <close> = c('x')\n  for i = 1, {k} do local y <close> = c('y' .. i) if i == {k} then error('E' .. i, 0) end end\nend)\n\
             return ok, err, log"
        ),
        6 => format!(
            "local proxy = setmetatable({{}}, {{__index = function(t, k) return k * {k} end, __newindex = function(t, k, v) rawset(t, k, v * 2) end, __len = function() return {n} end, __call = function(self, a) return a + {n} end}})\n\
             proxy[1] = 5\nlocal r = {{}}\nfor i = 1, 4 do r[i] = proxy[i] end\nreturn r, #proxy, proxy(1), rawget(proxy, 2)"
        ),
        _ => format!(
            "local acc = {{}}\nlocal i = 0\nwhile true do\n  i = i + 1\n  if i > {n} then break end\n  if i % {k} == 0 then goto continue end\n  acc[#acc + 1] = i\n  ::continue::\nend\n\
             local r = 0\nrepeat local z = r r = r + 1 until z >= {k}\nreturn acc, r, i"
        ),
    }
}

/// Drops ` (constant '...')` notes: C Lua names a constant operand or not depending on
/// how its code generator happened to load it.
fn normalize(r: Result<String, String>) -> Result<String, String> {
    fn strip(s: String) -> String {
        let mut out = String::new();
        let mut rest = s.as_str();
        // Every "what it was" note goes: C Lua names more operands in long and/or chains,
        // where its jump lists know more than RemedyLua's simpler code (the conformance
        // suite checks the notes where code is plain).
        while let Some(i) = [" (constant '", " (upvalue '", " (local '", " (global '", " (field '", " (method '"]
            .iter()
            .filter_map(|p| rest.find(p))
            .min()
        {
            out.push_str(&rest[..i]);
            let after = &rest[i + 2..];
            match after.find("')") {
                Some(j) => rest = &after[j + 2..],
                None => {
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }
    // MSVC's C library prints NaN as "-nan(ind)"; RemedyLua prints it as glibc does.
    let nan = |s: String| s.replace("-nan(ind)", "-nan").replace("nan(ind)", "nan");
    r.map(strip).map(nan).map_err(strip)
}

fn check(seed: u64, program: &str) -> Option<String> {
    let theirs = normalize(common::oracle(program, 5_000_000));
    let ours = normalize(common::remedy(program, 50_000_000));
    // The oracle's own limits and build: a step limit, and this C build's "%a" without
    // C99 printf support (RemedyLua formats "%a" with modifiers as glibc does).
    let skip = |s: &str| s.contains("step limit") || s.contains("not implemented");
    if theirs.as_ref().is_err_and(|e| skip(e)) || theirs.as_ref().is_ok_and(|e| skip(e)) {
        return None;
    }
    (ours != theirs && !common::c_limit_only_outcome(&ours, &theirs)).then(|| format!("seed {seed}:\n{program}\n  remedy: {ours:?}\n  c lua:  {theirs:?}"))
}

#[test]
fn random_programs_match_c_lua() {
    let iters: u64 = std::env::var("REMEDYLUA_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    let base: u64 = std::env::var("REMEDYLUA_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let only: Option<u64> = std::env::var("REMEDYLUA_FUZZ_ONLY").ok().and_then(|s| s.parse().ok());
    let mut failures = Vec::new();
    let seeds: Vec<u64> = match only {
        Some(s) => vec![s],
        None => (0..iters).map(|i| base.wrapping_mul(1_000_003).wrapping_add(i)).collect(),
    };
    for seed in &seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let program = if r.chance(60) { expr_program(&mut r) } else { structure_program(&mut r) };
        if let Some(f) = check(*seed, &program) {
            failures.push(f);
            if failures.len() >= 5 {
                break;
            }
        }
    }
    println!("{} random programs", seeds.len());
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n\n"));
}
