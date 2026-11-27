//! The iterative front end against the recursive one it replaced (feature `front-oracle`):
//! for every program here, the same syntax tree or the same error (message, line and
//! column), and the same bytecode for every prototype or the same compile error. Programs
//! the old front end refused for its nesting limit are skipped (the new one has none).
//!
//! Programs: every file under `tests/lua` (and each `-- case:` in it), the benchmarks,
//! generated programs (plain and `.rlua`), mutations of all of these (mostly invalid), deep
//! constructs around the old limit, and a list of hand-written valid and invalid ones.
//! `cargo test -p remedy-lua --test all --features front-oracle front_equiv`
#![cfg(feature = "front-oracle")]

#[path = "sweep1_fuzz/grammar.rs"]
#[allow(dead_code)]
mod grammar;
#[path = "sweep1_fuzz/mutate.rs"]
#[allow(dead_code)]
mod mutate;
#[path = "sweep1_fuzz/rng.rs"]
#[allow(dead_code)]
mod rng;

use std::path::Path;

use remedy_lua::__front::{compile_new, compile_new_with, compile_old, parse_new, parse_old, run_new};
use rng::Rng;

/// The old front end's own limits (gone from the new one).
fn old_limit(e: &str) -> bool {
    e.contains("too many syntax levels") || e.contains("expression too complex")
}

#[derive(Default, Debug)]
struct Tally {
    programs: usize,
    parsed: usize,
    parse_errors: usize,
    compiled: usize,
    compile_errors: usize,
    old_limit: usize,
    /// Programs whose bytecode register reuse changes (run both ways: same results).
    reused: usize,
}

impl Tally {
    fn check(&mut self, src: &str, name: &str) {
        self.programs += 1;
        let src = src.as_bytes();
        let old = parse_old(src, name);
        if old.as_ref().is_err_and(|e| old_limit(e)) {
            self.old_limit += 1;
            // Still nothing may go wrong in the new one.
            let _ = compile_new(src, name);
            return;
        }
        let new = parse_new(src, name);
        assert!(
            old == new,
            "parse differs for {name}:\n{}\nold: {old:?}\nnew: {new:?}",
            String::from_utf8_lossy(src)
        );
        if old.is_err() {
            self.parse_errors += 1;
            return;
        }
        self.parsed += 1;
        let old = compile_old(src, name);
        if old.as_ref().is_err_and(|e| old_limit(e)) {
            self.old_limit += 1;
            let _ = compile_new(src, name);
            return;
        }
        let new = compile_new(src, name);
        assert!(
            old == new,
            "compile differs for {name}:\n{}\nold: {old:#?}\nnew: {new:#?}",
            String::from_utf8_lossy(src)
        );
        if old.is_ok() {
            self.compiled += 1;
            // The one change from the old compiler: chains of unary operators and fields
            // reuse their target register (as C Lua; one register, not one a level). The
            // bytecode above is compared with it off; with it on, the program must give the
            // same results.
            if compile_new_with(src, name, true) != new {
                self.reused += 1;
                let (a, b) = (run_new(src, name, false), run_new(src, name, true));
                let budget = |s: &str| s.contains("fuel") || s.contains("memory");
                assert!(
                    a == b || budget(&a) || budget(&b),
                    "register reuse changed results for {name}:\n{}\nwithout: {a}\nwith:    {b}",
                    String::from_utf8_lossy(src)
                );
            }
        } else {
            self.compile_errors += 1;
        }
    }
}

/// Every Lua file under `dir` (recursively): the whole file, and each `-- case:` in it.
fn files(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            files(&p, out);
            continue;
        }
        let Some(ext) = p.extension().and_then(|x| x.to_str()) else { continue };
        if ext != "lua" && ext != "rlua" {
            continue;
        }
        let name = format!("{}.{ext}", p.file_stem().unwrap().to_string_lossy());
        let src = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
        if src.contains("\n-- case:") {
            for chunk in format!("\n{src}").split("\n-- case:").skip(1) {
                let body = chunk.split_once('\n').map_or("", |x| x.1);
                out.push((body.to_owned(), name.clone()));
            }
        }
        out.push((src, name));
    }
}

const HAND: &[&str] = &[
    // Invalid.
    "x = ",
    "local",
    "local function",
    "function",
    "function a.b:c.d() end",
    "return return",
    "if x then",
    "if x then else",
    "if x then elseif y then else",
    "if x then\n\nelse\n",
    "while true",
    "while true do",
    "for i = 1 do end",
    "for i, j = 1, 2 do end",
    "for i in do end",
    "for i = 1, 2, 3, 4 do end",
    "for 1 = 1, 2 do end",
    "repeat until",
    "repeat x = 1",
    "x = (1",
    "x = (1\n\n",
    "x = {1, 2",
    "x = {1, 2\n\n",
    "x = a[1",
    "x = a.",
    "x = a:b",
    "x = a:b c",
    "f(",
    "f(1,",
    "f(1\n,\n2",
    "local x <foo> = 1",
    "local x <close>, y <close> = 1, 2",
    "local x <const = 1",
    "goto",
    "::a",
    "::a:: ::a::",
    "goto nowhere",
    "do goto l end ::l::",
    "goto l local x ::l:: print(x)",
    "break",
    "x = ...",
    "function f() x = ... end",
    "function f(...) return function() return ... end end",
    "a, b",
    "a + 1 = 2",
    "(a) = 1",
    "a.b:c = 1",
    "f() = 1",
    "a, f() = 1, 2",
    "x = 1 + ",
    "x = not",
    "x = #",
    "return 1,",
    "return 1 2",
    "return 1; x = 1",
    "x = 'abc",
    "x = [[abc",
    "x = 0x",
    "x = 1e",
    "--[[ unclosed",
    "x = '\\q'",
    "local t = {[1] 2}",
    "local t = {[1] = }",
    "local t = {a = }",
    "local t = {,}",
    "x = function(a, 1) end",
    "x = function(..., a) end",
    "x = function(a,) end",
    "do end end",
    "end",
    "until x",
    "elseif",
    "x = a..",
    "x = 1 ... 2",
    "@",
    "x = 1 = 2",
    "local function f() return end end",
    "while x do break break end",
    "local a <const> = 1 a = 2",
    "local a <const> = 1 function f() a = 2 end",
    "local a <close> = nil a = 1",
    "x = y z",
    "x = {f = function() end,, }",
    "x = a.b.c[",
    "return x[",
    "a.b.c:d(",
    "for i = 1, 2 do local x = i end end",
    "if a then return 1 else return 2 end return 3 4",
    // Valid, with corners of code generation.
    "x = -0.0 return -0.0, 0.0, -(0.0)",
    "return 2^-3^2, -2^2, (-2)^2, 2^3^2",
    "local a, b, c, d = 1, 2, 3, 4 return a .. b .. c, a .. b + c .. d, (a .. b) .. c, a .. (b .. c)",
    "return not not nil, not 1, - - 1, ~ ~ 5, # 'abc'",
    "return 1 // 0.0, 1 % 0.0, 1 // 1, 7 % -3, 3 & 5 | 2 ~ 1, 1 << 63, 1 >> 1",
    "local a <const> = 'x' local b <const> = a return a .. b",
    "local x <close> = nil return 1",
    "local function f(...) return f(...) end return (f(1)), ...",
    "a.b.c.d = 1 a.b, c, d[1] = 1, 2",
    "function a.b.c.d:e() return self end function a.b.c.d() end function a() end local function b() end function b() end",
    "return a:b():c().d:e{1}'x'[2]",
    "local x = 1 and 2 or 3 local y = nil or false and 1 local z = x and y or z return x, y, z",
    "if a == b and c ~= d or not e then f() elseif a < b or b <= c and c > d and d >= e then g() else h() end",
    "while not (a and b) do a = a or 1 if a then break end end",
    "repeat local x = f() until x or not x and y",
    "for i = 10, 1, -1 do if i % 2 == 0 then goto continue end print(i) ::continue:: end",
    "for k, v in pairs(t) do local a, b = k, v end for k in next, t do end",
    "local t = {f(), ...} local u = {f()} local v = {..., f()} local w = {[1] = 1, x = 2, 3, [f()] = g()}",
    "local x, y = f() local a, b, c = ..., 1 local p, q = (f())",
    "x, y = y, x local a, b = 1 a, b = b, a local t = {} t[a], a = 1, 2 a, t[a] = 3, 4",
    "local t = {} t.x, t.y = t.y, t.x t[1], t[2], t[3] = f()",
    "return f(g(h(1, 2), ...), {x = y, [z] = w}, 'a' .. 'b')",
    "local s = 0 for i = 1, 100 do s = s + i * 2 - 1 / 3 // 4 % 5 ^ 6 end return s",
    "local a = 1 local function g() a = a + 1 return a end local function h() local b = a return function() return a + b end end return g, h",
    "do local a = 1 do local b = 2 goto x end ::x:: end",
    "local x = 1 + 2 * 3 - 4 / 5 + 6 // 7 + 8 % 9 + 2^3 + (1 + 2) * (3 + 4)",
    "return 'a' .. 1 .. 2.5 .. x .. y.z .. f() .. (g())",
    "local x = 1 x = x + 1 x = x .. 'a' x = -x x = #x x = not x",
    "local t = setmetatable({}, {__index = function(t, k) return k end}) return t.x, t['y'], t[1]",
    "local x = 0x7fffffffffffffff + 1 local y = 9007199254740993 local z = 1e308 * 10 local w = 0x10p-2",
    "goto a ::a:: goto b do ::b:: end ::b::",
    "return function(a, b, ...) local c <const> = 3 return a, b, c, ... end",
];

/// RemedyLua dialect programs (named `.rlua`).
const HAND_RLUA: &[&str] = &[
    "local x: number = 1 x += 2 x -= 1 x *= 3 x /= 2 x //= 1 x %= 7 x ^= 2 x ..= 'a' return x",
    "local t = {a = 1} t.a += 1 t['a'] ..= 'x' t[1 + 1] = 2 t[2] += t.a",
    "for i = 1, 10 do if i % 2 == 0 then continue end print(i) end",
    "local continue = 1 continue = continue + 1 return continue",
    "while true do local x = 1 if x then continue end break end",
    "repeat local y = 1 continue until true",
    "type Point = {x: number, y: number} type F = (number, string) -> boolean type U = number | string | nil type O = number? type A = {number} type M = {[string]: number}",
    "type N = (a: number, ...) -> (number, string) type E = () -> () type L = \"left\" | 'right' type Q = a.b.c",
    "local function f(a: number, b: string?, ...: any): (number, string) return a, b end",
    "local function g(): number return 1 end local h = function(x: {number}): () -> number return function() return 1 end end",
    "local s = `x = {x} and {y + 1} and {`inner {z}`}` return s",
    "return `{1}{2}` .. `plain` .. `{f(`{g()}`)}`",
    "local x: number = ",
    "local function f(a: ) end",
    "type T = ",
    "type T = {[string]: }",
    "type T = (number, string)",
    "type T = (a: number) -> ",
    "type T = {x: number,",
    "type T = {number",
    "x += ",
    "1 += 2",
    "f() += 1",
    "local s = `abc {1 + } def`",
    "local s = `abc {} def`",
    "local s = `abc {x y} def`",
    "local s = `unclosed {x`",
    "local x: {number = 1",
    "local function f(...: ) end",
    "function f(): (number, ) end",
    "local a: number <const> = 1 local b <close>: any = nil",
    "for i: number = 1, 2 do end for k: string, v: number in pairs(t) do end",
];

#[test]
fn iterative_front_end_matches_recursive() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut t = Tally::default();

    // The corpora and the benchmarks.
    let mut corpus = Vec::new();
    files(&root.join("tests/lua"), &mut corpus);
    files(&root.join("benches/lua"), &mut corpus);
    let n_files = corpus.len();
    for (src, name) in &corpus {
        t.check(src, name);
    }
    for (i, src) in HAND.iter().enumerate() {
        t.check(src, &format!("hand{i}.lua"));
    }
    for (i, src) in HAND_RLUA.iter().enumerate() {
        t.check(src, &format!("hand{i}.rlua"));
    }
    let fixed = t.programs;

    // Generated programs, plain and in the RemedyLua dialect (the plain twin too).
    let iters: u64 = std::env::var("FRONT_EQUIV_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(1500);
    // `FRONT_EQUIV_SEED` picks fresh programs (0: the default set).
    let seed: u64 = std::env::var("FRONT_EQUIV_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut generated = Vec::new();
    for i in 0..iters {
        let i = i ^ seed.wrapping_mul(0x1000_0001);
        let mut rng = Rng::new(0x5eed ^ i.wrapping_mul(0x9e37_79b9));
        let mut g = grammar::Gen::new(&mut rng, false);
        let p = g.program(4 + (i % 24) as i32);
        t.check(&p.l, "gen.lua");
        generated.push(p.l);
        let mut rng = Rng::new(0xab1e ^ i.wrapping_mul(0x85eb_ca6b));
        let mut g = grammar::Gen::new(&mut rng, true);
        let p = g.program(4 + (i % 24) as i32);
        t.check(&p.r, "gen.rlua");
        t.check(&p.l, "gen.lua");
        generated.push(p.r);
        if i % 5 == 0 {
            let mut rng = Rng::new(0xdee9 ^ i);
            let mut g = grammar::Gen::new(&mut rng, i % 2 == 0);
            let d = g.deep();
            t.check(&d.l, "deep.lua");
            t.check(&d.r, "deep.rlua");
        }
    }

    // Mutations (mostly invalid: the errors must agree too).
    let mut pool: Vec<String> = corpus.iter().map(|(s, _)| s.clone()).collect();
    pool.extend(HAND.iter().map(|s| s.to_string()));
    pool.extend(HAND_RLUA.iter().map(|s| s.to_string()));
    pool.extend(generated);
    let mut rng = Rng::new(0x0dd5 ^ seed);
    for i in 0..iters * 2 {
        let base = pool[rng.below(pool.len())].clone();
        let m = mutate::mutate(&base, &mut rng, &pool);
        t.check(&m, if i % 3 == 0 { "mut.rlua" } else { "mut.lua" });
    }

    println!("{n_files} corpus programs, {fixed} fixed in all; {t:?}");
    assert!(t.compiled > 1000 && t.parse_errors > 500, "{t:?}");
}
