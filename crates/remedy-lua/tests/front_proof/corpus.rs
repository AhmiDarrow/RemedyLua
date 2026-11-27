//! The programs the front-end proof runs on, as `(name, source)` (a name ending `.rlua` is
//! in the RemedyLua dialect): every `.lua`/`.rlua` file of the conformance, spec, sweep1 and
//! fuzz sets and the benchmarks (whole, and each `-- case:` in it), prefixes of them cut at
//! several points, seeded generated programs, and hand-written invalid programs that reach
//! every error the lexer, parser and compiler raise.
// The whole corpus feeds the equivalence checks, which need feature `front-oracle`.
#![cfg_attr(not(feature = "front-oracle"), allow(dead_code))]

use std::path::Path;

use super::grammar::Gen;
use super::rng::Rng;

/// The whole corpus.
pub fn corpus() -> Vec<(String, String)> {
    let mut out = files();
    out.extend(truncated(&out));
    out.extend(generated(300));
    out.extend(invalid());
    out
}

/// Every source file of the test and benchmark sets.
pub fn files() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    walk(&root.join("tests/lua"), &mut out);
    walk(&root.join("benches/lua"), &mut out);
    assert!(out.len() > 30, "corpus files not found under {}", root.display());
    out
}

fn walk(dir: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            walk(&p, out);
            continue;
        }
        let Some(ext) = p.extension().and_then(|x| x.to_str()) else { continue };
        if ext != "lua" && ext != "rlua" {
            continue;
        }
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
        if src.contains("\n-- case:") {
            for (i, chunk) in format!("\n{src}").split("\n-- case:").skip(1).enumerate() {
                let body = chunk.split_once('\n').map_or("", |x| x.1);
                out.push((
                    format!("{}#{i}.{ext}", name.trim_end_matches(&format!(".{ext}"))),
                    body.to_owned(),
                ));
            }
        }
        out.push((name, src));
    }
}

/// Each file cut at 1/7 .. 6/7 of its length (on a char boundary): truncated files.
pub fn truncated(files: &[(String, String)]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (name, src) in files {
        for k in 1..7 {
            let mut at = src.len() * k / 7;
            while !src.is_char_boundary(at) {
                at -= 1;
            }
            out.push((format!("cut{k}-{name}"), src[..at].to_owned()));
        }
    }
    out
}

/// `n` seeded programs from the differential fuzzer's grammar, plain and RemedyLua, and
/// its deep programs.
pub fn generated(n: u64) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for i in 0..n {
        let mut rng = Rng::new(0xf00d ^ i.wrapping_mul(0x9e37_79b9_7f4a_7c15));
        let rl = i % 2 == 1;
        let p = Gen::new(&mut rng, rl).program(4 + (i % 20) as i32);
        if rl {
            out.push((format!("gen{i}.rlua"), p.r));
        }
        out.push((format!("gen{i}.lua"), p.l));
        if i % 10 == 0 {
            let mut rng = Rng::new(0xdeeb ^ i);
            let d = Gen::new(&mut rng, rl).deep();
            out.push((format!("deep{i}.rlua"), d.r));
            out.push((format!("deep{i}.lua"), d.l));
        }
    }
    out
}

/// Message stems the invalid programs must reach between them: every error the lexer,
/// parser and compiler raise (except "chunk has too many tokens", which needs gigabytes,
/// and the memory error, which `memcap` covers).
pub const MESSAGES: &[&str] = &[
    // Lexer.
    "invalid long string delimiter",
    "unfinished long string",
    "unfinished long comment",
    "unfinished string",
    "hexadecimal digit expected",
    "missing '{'",
    "missing '}'",
    "UTF-8 value too large",
    "decimal escape too large",
    "invalid escape sequence",
    "malformed number",
    "unfinished interpolated string",
    "unfinished '{' in interpolated string",
    "empty expression in interpolated string",
    // Parser.
    "unexpected symbol",
    "<eof> expected",
    "'then' expected",
    "'do' expected",
    "'end' expected",
    "'until' expected",
    "'=' expected",
    "'in' expected",
    "'(' expected",
    "')' expected",
    "']' expected",
    "'}' expected",
    "'::' expected",
    "'>' expected",
    "(to close",
    "unknown attribute",
    "multiple to-be-closed variables",
    "function arguments expected",
    "cannot use '...' outside a vararg function",
    "<name> or '...' expected",
    "<name> expected",
    "'}' expected in interpolated string",
    "type expected",
    "'->' expected",
    "syntax error (cannot assign to this expression)",
    "syntax error near",
    // Compiler.
    "break outside loop",
    "continue outside loop",
    "no visible label",
    "jumps into the scope of local",
    "already defined",
    "attempt to assign to const variable",
];

/// Programs every front end must refuse.
pub fn invalid() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, s) in INVALID.iter().enumerate() {
        out.push((format!("bad{i}.lua"), (*s).to_owned()));
    }
    for (i, s) in INVALID_RLUA.iter().enumerate() {
        out.push((format!("bad{i}.rlua"), (*s).to_owned()));
    }
    out
}

const INVALID: &[&str] = &[
    // Truncated.
    "x = ",
    "local",
    "local function",
    "function",
    "function f(",
    "function f(a,",
    "function f() return",
    "if x",
    "if x then",
    "if x then else",
    "if x then elseif",
    "while",
    "while x",
    "while x do",
    "for",
    "for i",
    "for i =",
    "for i = 1",
    "for i = 1,",
    "for i = 1, 2",
    "for k, v in",
    "repeat",
    "repeat x = 1",
    "repeat until",
    "do",
    "x = (1",
    "x = (1\n\n",
    "x = {1, 2",
    "x = {[1]",
    "x = {[1] =",
    "x = a[1",
    "x = a.",
    "x = a:",
    "x = a:b",
    "f(",
    "f(1,",
    "f(1\n,\n2",
    "return f(g(h(",
    "local t = {{{",
    "x = 1 +",
    "x = not",
    "x = -",
    "return 1,",
    "goto",
    "::",
    "::a",
    "local x <",
    "local x <const",
    // Stray tokens.
    "x = 1 )",
    "x = 1 ]",
    "}",
    "end",
    "until x",
    "elseif",
    "else",
    "then",
    "do end end",
    "@",
    "$",
    "x = !y",
    "x = 1 = 2",
    "x ==== 1",
    "local 1 = 2",
    "local function 1() end",
    "local function f.a() end",
    "function 1() end",
    "function a.b:c.d() end",
    "function f(a b) end",
    "function f(1) end",
    "x = function(a, 1) end",
    "x = function(..., a) end",
    "x = function(a,) end",
    "return return",
    "return 1 2",
    "return 1; x = 1",
    "x = y z",
    "x = a.1",
    "x = a:1()",
    "x = a:b c",
    "x = f 1",
    "a, b",
    "a.b:c = 1",
    "a + 1 = 2",
    "(a) = 1",
    "f() = 1",
    "a, f() = 1, 2",
    "'s' = 1",
    "1 = 1",
    "x = a..",
    "x = 1 ... 2",
    "function f() return ... end",
    "function f(...) return function() return ... end end",
    "for i = 1 do end",
    "for i, j = 1, 2 do end",
    "for i in do end",
    "for 1 = 1, 2 do end",
    "for a.b = 1, 2 do end",
    "for i = 1, 2, 3, 4 do end",
    "if x then return 1 else return 2 end return 3 4",
    "local t = {[1] 2}",
    "local t = {[1] = }",
    "local t = {a = }",
    "local t = {,}",
    "x = {f = function() end,, }",
    "while x do end end",
    "local function f() return end end",
    "x = #",
    "x = 1 + * 2",
    "\x01",
    "x = '\u{0}' y",
    // Bad escapes and strings.
    "x = '\\q'",
    "x = '\\xZZ'",
    "x = '\\x5'",
    "x = '\\u123'",
    "x = '\\u{12'",
    "x = '\\u{zz}'",
    "x = '\\u{80000000}'",
    "x = '\\300'",
    "x = '\\256'",
    "x = '\\",
    "x = 'abc",
    "x = \"abc",
    "x = 'abc\ny'",
    "x = 'abc\\z",
    // Numbers.
    "x = 0x",
    "x = 1e",
    "x = 1e+",
    "x = 3x",
    "x = 1..2",
    "x = 0x1p",
    "x = 08.5.5",
    // Long strings and comments.
    "x = [[abc",
    "x = [==[abc]=]",
    "x = [=abc",
    "x = [= =[",
    "--[[ unclosed",
    "--[==[ unclosed ]]",
    "x = 1 --[=[ unclosed\n\n",
    "x = [[abc\n\ndef",
    "x = [==[\n\n\n",
    "--[[\nline 2\nline 3",
    "x = 1\n--[==[ a\nb ]=]\n",
    // goto and labels.
    "goto nowhere",
    "goto continue",
    "goto 1",
    ":: a",
    "::a:: ::a::",
    "::a:: do ::a:: end",
    "do goto l end",
    "goto l local x ::l:: print(x)",
    "repeat goto l local x ::l:: until x",
    "local function f() goto l end ::l::",
    "for i = 1, 2 do goto out end",
    // <const> and <close>.
    "local x <foo> = 1",
    "local x <> = 1",
    "local x <close>, y <close> = 1, 2",
    "local a <const> = 1 a = 2",
    "local a <const> a = 1",
    "local a <const> = 1 function f() a = 2 end",
    "local a <const> = 1 local function f() return function() a = 3 end end",
    "local a <close> = nil a = 1",
    "local a <const>, b = 1 b = 2 a = 3",
    "local a <const> = 1 a, b = 1, 2",
    "local x <const = 1",
    "local x <const> <close> = 1",
    // break.
    "break",
    "do break end",
    "if x then break end",
    "function f() break end",
    "while x do local function g() break end end",
    "for i = 1, 2 do end break",
    "repeat local f = function() break end until true",
];

const INVALID_RLUA: &[&str] = &[
    "local x: number = ",
    "local x: = 1",
    "local function f(a: ) end",
    "local function f(...: ) end",
    "function f(): (number, ) end",
    "type T = ",
    "type T = {[string]: }",
    "type T = (number, string)",
    "type T = (a: number) -> ",
    "type T = {x: number,",
    "type T = {number",
    "x += ",
    "1 += 2",
    "f() += 1",
    "x ..= ",
    "local s = `abc {1 + } def`",
    "local s = `abc {} def`",
    "local s = `abc {x y} def`",
    "local s = `unclosed {x`",
    "local s = `unclosed",
    "local s = `{x",
    "local s = `a {f(1)\n\n",
    "local s = `{ {x}",
    "local s = `{`{`",
    "local x: {number = 1",
    "continue",
    "function f() continue end",
    "while x do local function g() continue end end",
    "local a <const> = 1 a += 1",
    "for i = 1, 2 do continue continue end break",
];

/// `src` as a Lua string literal (every byte a decimal escape).
pub fn quote(src: &str) -> String {
    let mut q = String::with_capacity(src.len() * 4 + 2);
    q.push('"');
    for b in src.bytes() {
        q.push_str(&format!("\\{b}"));
    }
    q.push('"');
    q
}

#[test]
fn invalid_programs_are_refused_and_reach_every_error() {
    let (mut seen, mut loaded) = (Vec::new(), Vec::new());
    for (name, src) in invalid() {
        let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::NONE);
        match lua.load(&src, &format!("={name}")) {
            Err(remedy_lua::Error::Syntax(d)) => seen.push(d.message),
            other => loaded.push(format!("{name} {src:?}: {other:?}")),
        }
    }
    assert!(loaded.is_empty(), "should not compile:\n{}", loaded.join("\n"));
    let missing: Vec<_> = MESSAGES.iter().filter(|m| !seen.iter().any(|s| s.contains(*m))).collect();
    assert!(missing.is_empty(), "no invalid program reaches {missing:?}");
}

/// Plain-Lua invalid programs against C Lua: both refuse, with the same message at the
/// same line.
#[test]
fn invalid_programs_are_refused_by_c_lua_too() {
    let mut diffs = Vec::new();
    for (name, src) in invalid() {
        if name.ends_with(".rlua") {
            continue;
        }
        let prog = format!("local f, e = load({}, '=t') return tostring(e)", quote(&src));
        let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
        let v = lua.exec(&prog, "=driver").unwrap();
        let ours = v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default();
        let theirs = remedy_lua_oracle::run(&prog, 1 << 20).unwrap();
        assert!(theirs != "nil" && ours != "nil", "{src:?}: ours {ours:?}, C Lua {theirs:?}");
        if ours != theirs {
            diffs.push(format!("{src:?}\n   ours: {ours}\n   C:    {theirs}"));
        }
    }
    assert!(diffs.is_empty(), "{} message differences from C Lua:\n{}", diffs.len(), diffs.join("\n"));
}
