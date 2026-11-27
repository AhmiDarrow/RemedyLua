//! Sweep 1, area "spec": correctness against the Lua 5.4 manual and C Lua 5.4.7.
//!
//! `tests/lua/sweep1/spec/*.lua` holds cases (split at `-- case:` lines) that are run in
//! RemedyLua and in the C Lua oracle; every result must be equal. Cases that expose a bug
//! are not there: each one is its own `#[ignore = "sweep1: ..."]` test below, so the
//! suite stays green and each finding can be run alone.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

/// Every case may use `L(src)`: the load error message, or "ok"; and `E(f, ...)`: the
/// error (or first result) of `pcall(f, ...)`, one value so none is cut by adjustment.
const PRELUDE: &str = "local function L(s, ...) local f, e = load(s, ...) if f then return 'ok' end return e end local function E(...) return select(2, pcall(...)) end ";

fn both(src: &str) -> (Result<String, String>, Result<String, String>) {
    let p = format!("{PRELUDE}{src}");
    let theirs = common::oracle(&p, 50_000_000);
    (common::remedy(&p, 500_000_000), theirs)
}

/// Asserts that `src` gives the same serialized results in RemedyLua and C Lua.
fn same(src: &str) {
    let (ours, theirs) = both(src);
    if ours != theirs && common::c_limit_only_outcome(&ours, &theirs) {
        return;
    }
    assert_eq!(ours, theirs, "\nprogram:\n{src}\n(left: remedy, right: c lua)");
}

#[test]
fn spec_cases_match_c_lua() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua/sweep1/spec");
    let mut files: Vec<_> =
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    let (mut count, mut failures) = (0, Vec::new());
    for f in files.iter().filter(|p| p.extension().is_some_and(|x| x == "lua")) {
        let src = format!("\n{}", std::fs::read_to_string(f).unwrap().replace("\r\n", "\n"));
        for chunk in src.split("\n-- case:").skip(1) {
            let (name, body) = chunk.split_once('\n').unwrap_or((chunk, ""));
            count += 1;
            if std::env::var_os("SWEEP_TRACE").is_some() {
                eprintln!("case {}", name.trim());
            }
            let (ours, theirs) = both(body);
            if ours != theirs && !common::c_limit_only_outcome(&ours, &theirs) {
                let file = f.file_name().unwrap().to_string_lossy();
                failures.push(format!("{file}: {}\n  remedy: {ours:?}\n  c lua:  {theirs:?}", name.trim()));
            }
        }
    }
    println!("{count} spec cases");
    assert!(failures.is_empty(), "{} of {count} differ:\n{}", failures.len(), failures.join("\n"));
}

// ---------------------------------------------------------------------------------------
// Findings. Each test fails today.
// ---------------------------------------------------------------------------------------

/// SPEC-01 (major): coroutine.close drops an error raised by a pending `__close`.
/// Expected `false "closing"`, actual `true`. `Lua::close_with_error` reports a close
/// error as its Ok value (vm.rs:790-805), and stdlib/coroutine.rs:123-129 only looks at Err.
#[test]
fn spec01_coroutine_close_reports_close_error() {
    same(
        r#"local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function() error("closing", 0) end})
  coroutine.yield(1)
end)
coroutine.resume(co)
return coroutine.close(co)"#,
    );
}

/// SPEC-02 (major): a to-be-closed value whose `__close` is gone at closing time is skipped
/// silently. C Lua raises "attempt to call a nil value (metamethod 'close')".
/// vm.rs:808-811 (`call_close` returns Ok when the metamethod is nil).
#[test]
fn spec02_close_metamethod_removed_is_error() {
    same(
        r#"local mt = {__close = function() end}
return pcall(function()
  local x <close> = setmetatable({}, mt)
  mt.__close = nil
end)"#,
    );
}

/// SPEC-03 (major): an error raised by a `load` reader function propagates out of `load`
/// instead of being returned as `nil, message` (C: `true nil "t:1: rd"`; RemedyLua:
/// `false "t:1: rd"`). stdlib/base.rs:320 uses `?` on the reader call.
#[test]
fn spec03_load_reader_error_is_returned() {
    same(r#"return pcall(load, function() error("rd") end)"#);
}

/// SPEC-04 (major): utf8.codepoint and utf8.len accept positions C Lua rejects:
/// codepoint with i > #s (j defaults to i) or i == 0 returns nothing; utf8.len(s, 0) and
/// utf8.len(s, -10) count from 1 instead of raising "initial position out of bounds"
/// (and len("", -10, 3) blames argument #3, not #2). stdlib/utf8.rs:75-76 use string.sub's
/// position rules (posrelat_i maps 0 to 1, end_pos clamps to #s) where C uses u_posrelat
/// and then checks posi >= 1 and pose <= len.
#[test]
fn spec04_utf8_codepoint_bounds() {
    same(
        r#"return E(utf8.codepoint, "abc", 4), E(utf8.codepoint, "abc", 0), E(utf8.codepoint, "abc", 1, 4),
  E(utf8.len, "abc", 0, 3), E(utf8.len, "abc", -10), E(utf8.len, "", -10, 3)"#,
    );
}

/// SPEC-05 (major): the `%z` class (deprecated, still implemented by Lua 5.4's lstrlib)
/// is treated as a literal 'z', so `%f[%z]` never matches at the end of a subject and
/// `%f[^%z]` never at its start. stdlib/pattern.rs:35-47 (class match has no 'z' arm).
#[test]
fn spec05_pattern_class_z() {
    same(
        r#"return ("x"):find("%f[%z]"), ("ab"):gsub("%f[^%z]", "|"), ("a\0b"):find("%z"), ("a\0b"):gsub("%Z", "."), ("z"):match("[%z]")"#,
    );
}

/// SPEC-06 (critical in debug builds, major in release): `select(math.mininteger, ...)`
/// negates i64::MIN at stdlib/base.rs:203: a panic ("attempt to negate with overflow")
/// wherever overflow checks are on (every test/dev build). With checks off (the release
/// profile) it wraps and returns no values instead of raising "index out of range".
#[test]
fn spec06_select_mininteger() {
    same(r#"return pcall(select, math.mininteger, 1)"#);
}

/// SPEC-07 (major): xpcall whose message handler itself errors returns the handler's error
/// ("t:1: b"); C Lua gives status LUA_ERRERR: `false "error in error handling"`.
/// vm.rs:1013-1019 (`call_handler`, documented there as intended).
#[test]
fn spec07_xpcall_handler_error() {
    same(r#"return xpcall(function() error("a") end, function(m) error("b") end)"#);
}

/// SPEC-08 (major): `load` of a string skips a first line starting with '#'. Only
/// luaL_loadfile does that; C's `load("#!x\nreturn 1")` fails ("unexpected symbol near
/// '#'"). lexer.rs:255-260 does it for every chunk.
#[test]
fn spec08_load_does_not_skip_shebang() {
    same(r##"return L("#!/bin/lua\nreturn 1")"##);
}

/// SPEC-09 (major): 150 nested table constructors compile in C Lua but RemedyLua refuses
/// the chunk (parser.rs:126: MAX_DEPTH or the native stack guard trips first).
#[test]
fn spec09_nested_constructors_150() {
    same(
        r#"local f, e = load("return " .. string.rep("{", 150) .. string.rep("}", 150)) return f and type(f()) or e"#,
    );
}

/// SPEC-10 (minor): string arithmetic coercion is built into the VM instead of living in
/// the string metatable as in Lua 5.4 (manual §3.4.3; lstrlib `stringmetamethods`):
/// `getmetatable("").__add` is nil, and setting it does not change `"10" + 1`.
/// C: `true "added"`; RemedyLua: `false 11`.
#[test]
fn spec10_string_arith_metamethods() {
    same(
        r#"local smt = getmetatable("")
local has = smt.__add ~= nil and smt.__unm ~= nil and smt.__idiv ~= nil
smt.__add = function() return "added" end
local r = "10" + 1
return has, r"#,
    );
}

/// SPEC-11: C Lua stops at 200 local variables in one function ("too many local
/// variables (limit is 200) ..."). RemedyLua is a strict superset with no such limit
/// (user decision): the program runs, and the comparison accepts exactly that difference.
#[test]
fn spec11_local_variable_limit() {
    let src = r#"return L(string.rep("local a;", 201))"#;
    same(src);
    let (ours, theirs) = both(src);
    assert!(theirs.as_ref().is_ok_and(|s| s.contains("too many local variables")), "{theirs:?}");
    assert!(ours.as_ref().is_ok_and(|s| !s.contains("error")), "{ours:?}");
}

/// SPEC-12 (minor): argument checks C Lua makes and RemedyLua skips: `setmetatable({})`
/// (C: "bad argument #2 to 'setmetatable' (nil or table expected)"; base.rs:98 treats
/// none as nil) and `warn()` (C: "bad argument #1 to 'warn' (string expected, got no
/// value)"; base.rs:293).
#[test]
fn spec12_missing_argument_checks() {
    same(r#"return E(setmetatable, {}), E(warn)"#);
}

/// SPEC-13 (minor): collectgarbage("generational"/"incremental") must return the previous
/// mode; RemedyLua always answers "incremental" (base.rs:396). C: incremental,
/// incremental, generational.
#[test]
fn spec13_collectgarbage_mode_switch() {
    same(
        r#"return collectgarbage("incremental"), collectgarbage("generational"), collectgarbage("incremental")"#,
    );
}

/// SPEC-14 (minor): errors found by the compiler (const assignment, goto, break) name a
/// `load`ed string chunk by its raw source instead of `[string "..."]`; parser errors are
/// right. compiler.rs:311 and the other `self.err` sites skip the chunkid formatting.
#[test]
fn spec14_compiler_error_chunkid() {
    same(
        r#"return L("local x <const> = 1; x = 2"), L("goto nowhere"), L("break"), L("::a:: local function f() goto a end")"#,
    );
}

/// SPEC-15 (minor): lexer/parser message text. C: `near <eof>` (no quotes); escape and
/// string errors name the token (`near ''\x4''`, strings keep their quote); "invalid long
/// string delimiter near '[='"; "'in' expected"; "break outside loop"; "no visible label
/// 'a' for <goto>"; "syntax error near '='" for `f() = 1`; a malformed numeral is quoted
/// up to one letter past the digits ("near '8000000000000000p'"), not the whole word.
/// lexer.rs, parser.rs:345,404,413, compiler.rs:311,831.
#[test]
fn spec15_syntax_messages() {
    same(
        r#"return L("if x then"), L("return '\\x4'"), L("return '\\q'"), L("return 'abc"), L("return [=x"),
  L("for a, b = 1, 2 do end"), L("f() = 1"), L("'s' = 1"), L("x = 'abc\ndef'"), L("return '\\u41'"),
  L("return 8000000000000000pEX 1")"#,
    );
}

/// SPEC-16 (minor): runtime message text: "'__index' chain too long; possible loop"
/// (vm.rs:2301,2342,2374,2407 say "possibly a loop"); "bad 'for' initial value (number
/// expected, got nil)" (vm.rs:2223); "invalid capture index %0" (no "in pattern").
#[test]
fn spec16_runtime_messages() {
    same(
        r#"local t = setmetatable({}, {}) getmetatable(t).__index = t
return E(function() return t.x end), E(function() for i = nil, 2 do end end), E(string.match, "aa", "%0")"#,
    );
}

/// SPEC-17 (minor): positions added where C Lua adds none: a non-callable called from C
/// (pcall) and resuming a dead coroutine.wrap function give "t:N: ..." in RemedyLua.
#[test]
fn spec17_no_position_for_c_level_errors() {
    same(
        r#"local w = coroutine.wrap(function() end) w()
return E(setmetatable({}, {__call = 5})), E(w)"#,
    );
}

/// SPEC-18 (minor): `load` details. The mode error always says "(mode is 'b')" whatever
/// the mode (base.rs:352); a number chunk name is refused (C converts it); a reader that
/// returns a number is refused (C's lua_isstring accepts numbers). C: `nil "attempt to
/// load a text chunk (mode is 'x')"`, `true`, `42`.
#[test]
fn spec18_load_details() {
    same(
        r#"local parts = {"return ", 42}
local i = 0
local f = load(function() i = i + 1 return parts[i] end)
return select(2, load("return 1", nil, "x")), pcall(load, "return 1", 5), f and f()"#,
    );
}

/// SPEC-19 (minor): argument error wording: xpcall without handler ("function expected",
/// base.rs:254 says "value"), coroutine.isyieldable(1) ("thread expected"), string.pack
/// 'X' misuse lacks "bad argument #1 to 'string.pack' (...)", os.time "field 'day' is not
/// an integer" (RemedyLua adds "in date table"), `%q` of a table lacks the "#2"
/// (strformat.rs:360).
#[test]
fn spec19_argument_messages() {
    same(
        r#"return E(xpcall, print), E(coroutine.isyieldable, 1), E(string.pack, "Xc1", 1),
  E(os.time, {year = 2020, month = 1, day = 1.5}), E(string.format, "%q", {})"#,
    );
}

/// SPEC-20 (minor): os.date accepts conversions C Lua rejects ("invalid conversion
/// specifier '%Ez'"). stdlib/os.rs strftime option check.
#[test]
fn spec20_os_date_invalid_conversion() {
    same(r#"return pcall(os.date, "%Ez", 0)"#);
}

/// SPEC-21 (minor): string.gsub with a capture that is never closed: C Lua matches (an
/// open capture is only an error when its value is used) and returns `"a" 2`; RemedyLua
/// raises "unfinished capture" up front (stdlib/pattern.rs).
#[test]
fn spec21_unclosed_capture_unused() {
    same(r#"return pcall(string.gsub, "a", "(", "")"#);
}

/// SPEC-22 (minor): string.format accepts '%F', which Lua 5.4's lstrlib does not list
/// (L_FMTFLAGSF covers 'a','A','f','e','E','g','G' only): C raises "invalid conversion
/// '%F' to 'format'". stdlib/strformat.rs:158,234. Found by the sweep fuzzer.
#[test]
fn spec22_format_upper_f_is_invalid() {
    same(r#"return E(string.format, "%F", 1.5), E(string.format, "%.3F", 2)"#);
}

/// SPEC-24 (major): string.format '%u' formats its integer as signed. Lua 5.4 passes it to
/// C's %llu: `("%u"):format(-7)` is "18446744073709551609". strformat.rs:190 handles 'u'
/// with 'd'/'i' (n.unsigned_abs() plus a sign). Found by the sweep fuzzer.
#[test]
fn spec24_format_u_is_unsigned() {
    same(r#"return string.format("%u|%022u|%-5u|", -7, -7, 3), string.format("%u", math.mininteger)"#);
}

/// SPEC-25 (major): tonumber(s, base) refuses a leading '+', which C Lua's luaB_tonumber
/// skips: `tonumber("+p1", 26)` is 651, `tonumber("+ff", 16)` is 255; RemedyLua gives nil.
/// number.rs:493-496 (`str_to_int_base` only knows '-'). Found by the sweep fuzzer.
#[test]
fn spec25_tonumber_base_plus_sign() {
    same(r#"return tonumber("+p1", 26), tonumber("+ff", 16), tonumber(" +10 ", 2), tonumber("+-1", 10)"#);
}

/// SPEC-26 (major, edge): the '#' flag on zero. C: `%#.0g`/`%#.1g` of 0 give "0." (the
/// alternate form always keeps the point; number.rs:164 adds it only when p > 1) and
/// `%#.0o` of 0 gives "0" (the alternate form forces a leading zero; strformat.rs:218
/// adds it only when n != 0, after precision 0 has emptied the digits). Found by the
/// sweep fuzzer.
#[test]
fn spec26_alt_flag_on_zero() {
    same(
        r#"return string.format("%#.0g|%#.1g|%+#.0g|%#.0o|%#o|%#5.0o|%#.0e|%#.0f", 0, 0.0, -0.0, 0, 0, 0, 0, 0)"#,
    );
}

/// SPEC-27 (major): when a coroutine dies by an error, C Lua leaves its pending
/// to-be-closed variables open (the stack is kept for inspection) until coroutine.close;
/// coroutine.wrap closes them itself (auxwrap calls lua_closethread). RemedyLua closes
/// them at once inside resume (vm.rs:1346-1353), so `__close` runs before resume returns
/// (log length 1 instead of 0). Found by 08_extra.lua.
#[test]
fn spec27_resume_error_keeps_tbc_until_close() {
    same(
        r#"local log = {}
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function(_, e) log[#log + 1] = "closed:" .. tostring(e) end})
  error("boom", 0)
end)
local r1 = {coroutine.resume(co)}
local before = #log
local r2 = {coroutine.close(co)}
return r1[1], r1[2], before, log[1], r2[1], r2[2]"#,
    );
}

/// SPEC-23 (major): integer `for` with a NaN limit and a negative step. C Lua's forlimit
/// treats NaN like "below every integer" (`0 < NaN` is false): with step < 0 the limit
/// becomes math.mininteger and `for i = math.mininteger, 0/0, -1` runs once. RemedyLua
/// skips the loop (vm.rs:2192 returns early on NaN). Found by the sweep fuzzer.
#[test]
fn spec23_for_nan_limit_negative_step() {
    same(
        r#"local n, m = 0, 0
for i = math.mininteger, 0/0, -1 do n = n + 1 end
for i = math.mininteger, 0/0, math.mininteger do m = m + 1 end
return n, m"#,
    );
}

// ---------------------------------------------------------------------------------------
// A second differential generator for the areas tests/differential.rs does not reach:
// random string.format specs, random patterns on random subjects, string.pack formats,
// numeral parsing, table-library call sequences, utf8 on random bytes and numeric for
// loops with extreme bounds. Short by default; long runs:
// `SWEEP_FUZZ_ITERS=20000 SWEEP_FUZZ_SEED=9 cargo test -p remedy-lua --test all sweep1_spec::fuzz -- --nocapture`
// (`SWEEP_FUZZ_ONLY=<seed>` replays one program).
// ---------------------------------------------------------------------------------------

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

const NUMS: &[&str] = &[
    "0",
    "1",
    "-1",
    "7",
    "-7",
    "255",
    "65",
    "1000000",
    "math.maxinteger",
    "math.mininteger",
    "0.0",
    "-0.0",
    "0.5",
    "1.5",
    "-2.25",
    "3.0",
    "1e10",
    "1e300",
    "1e-300",
    "2^53",
    "2^63",
    "-2^63",
    "math.huge",
    "-math.huge",
    "0.1",
    "1/3",
    "123456.789",
    "5e-324",
    "(0/0)",
];
const STRS: &[&str] =
    &["\"\"", "\"abc\"", "\"10\"", "\"0x1F\"", "\" 5 \"", "\"a\\0b\"", "\"\\255\\1\"", "\"3.5\"", "\"%d\""];

fn value(r: &mut Rng) -> String {
    match r.below(10) {
        0..=6 => r.pick(NUMS).to_owned(),
        7 | 8 => r.pick(STRS).to_owned(),
        _ => r.pick(&["true", "nil", "{}"]).to_owned(),
    }
}

fn format_case(r: &mut Rng) -> String {
    // Not '%a': the oracle's MSVC build has no modifiers for it and prints subnormals
    // differently from glibc.
    let conv = r.pick(&["d", "i", "u", "o", "x", "X", "c", "f", "F", "e", "E", "g", "G", "q", "s", "%"]);
    let mut spec = String::from("%");
    // C Lua's oracle build cannot format '%a' with modifiers, and '%q' takes none.
    if !matches!(conv, "a" | "A" | "q" | "%") {
        for f in ["-", "+", " ", "#", "0"] {
            if r.chance(15) {
                spec.push_str(f);
            }
        }
        if r.chance(50) {
            spec.push_str(&r.below(25).to_string());
        }
        if r.chance(40) {
            spec.push('.');
            if r.chance(80) {
                spec.push_str(&r.below(25).to_string());
            }
        }
    }
    spec.push_str(conv);
    let mut v = value(r);
    // MSVC pads "-nan(ind)", a longer text than glibc's "-nan", so widths differ; '%s' of
    // a table prints an address; and
    // '%q' writes floats as hex, so the subnormal prints the MSVC way in C.
    // MSVC also writes "%#E" of infinity as "-.INF" (a C library bug, not Lua's).
    while v == "(0/0)"
        || (conv == "s" && v == "{}")
        || (conv == "q" && v == "5e-324")
        || (spec.contains('#') && v.contains("huge"))
    {
        v = value(r);
    }
    format!("string.format({:?}, {})", spec, v)
}

fn pattern(r: &mut Rng, depth: usize) -> String {
    let mut p = String::new();
    if r.chance(15) {
        p.push('^');
    }
    for _ in 0..1 + r.below(4) {
        let atom = match r.below(12) {
            0 => r
                .pick(&["%a", "%d", "%s", "%w", "%p", "%l", "%u", "%x", "%c", "%g", "%A", "%S", "%W"])
                .to_owned(),
            1 => r
                .pick(&[".", "a", "b", "(", ")", " ", "1", "%.", "%%", "%(", "%)"])
                .replace('(', "%(")
                .replace(')', "%)"),
            2 => r
                .pick(&["[a-c]", "[^%s]", "[%d_]", "[]]", "[^]a]", "[a-]", "[%a%d]", "[^ab]", "[(]"])
                .to_owned(),
            3 => r.pick(&["%f[%w]", "%f[%W]", "%f[%a]", "%f[ ]", "%f[^%d]"]).to_owned(),
            4 => r.pick(&["%b()", "%bab", "%b[]"]).to_owned(),
            5 if depth > 0 => {
                format!("({})", pattern(r, depth - 1).trim_start_matches('^').trim_end_matches('$'))
            }
            6 => "()".to_owned(),
            _ => r.pick(&["a", "b", "c", ".", "%a", "x", " "]).to_owned(),
        };
        p.push_str(&atom);
        if !atom.starts_with("%f") && !atom.starts_with("%b") && !atom.ends_with(')') && r.chance(40) {
            p.push_str(r.pick(&["*", "+", "-", "?"]));
        }
    }
    if r.chance(10) {
        p.push('$');
    }
    p
}

fn subject(r: &mut Rng) -> String {
    let n = r.below(12);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(r.pick(&["a", "b", "c", "(", ")", " ", "1", "x", "[", "]", "ab", "\\0", "A", "_"]));
    }
    s
}

fn pattern_case(r: &mut Rng) -> String {
    let s = subject(r);
    let p = pattern(r, 2);
    let init = r.pick(&["", ", 1", ", 2", ", -2", ", 0", ", 20"]);
    match r.below(5) {
        0 => format!("string.find(\"{s}\", {p:?}{init})"),
        1 => format!("string.match(\"{s}\", {p:?}{init})"),
        2 => format!(
            "(function() local o = {{}} for a, b in string.gmatch(\"{s}\", {p:?}) do o[#o + 1] = tostring(a) .. ',' .. tostring(b) end return table.concat(o, '|') end)()"
        ),
        3 => {
            let rep = r.pick(&[
                "\"<%0>\"",
                "\"%1\"",
                "\"\"",
                "{a = 'A', b = false}",
                "function(x) return x and #x end",
                "\"%%\"",
            ]);
            let n = r.pick(&["", ", 1", ", 0", ", 3"]);
            format!("string.gsub(\"{s}\", {p:?}, {rep}{n})")
        }
        _ => format!("string.find(\"{s}\", {p:?}, 1, true)"),
    }
}

fn pack_case(r: &mut Rng) -> String {
    let opts: &[(&str, &str)] = &[
        ("b", "-5"),
        ("B", "200"),
        ("h", "-300"),
        ("H", "60000"),
        ("i", "-70000"),
        ("I", "70000"),
        ("l", "1"),
        ("j", "math.mininteger"),
        ("J", "-1"),
        ("T", "3"),
        ("f", "0.5"),
        ("d", "-1.25"),
        ("n", "1e300"),
        ("i3", "-8388608"),
        ("I5", "1099511627775"),
        ("i16", "-2"),
        ("I9", "5"),
        ("s1", "'hey'"),
        ("s2", "''"),
        ("z", "'zed'"),
        ("c3", "'ab'"),
        ("x", ""),
        ("Xi4", ""),
        ("Xh", ""),
    ];
    let mut fmt = String::from(r.pick(&["", "<", ">", "=", "!", "!4", "<!8", ">!2"]));
    let mut vals = Vec::new();
    for _ in 0..1 + r.below(5) {
        let (o, v) = opts[r.below(opts.len())];
        // 'l' differs by platform (C long), so it never gets a value that would show it.
        if o == "l" {
            continue;
        }
        fmt.push_str(o);
        if r.chance(10) {
            fmt.push(' ');
        }
        if !v.is_empty() {
            vals.push(if r.chance(10) { value(r) } else { v.to_owned() });
        }
    }
    let args = vals.join(", ");
    let sep = if args.is_empty() { "" } else { ", " };
    format!(
        "(function() local s = string.pack({fmt:?}{sep}{args}) return #s, string.packsize({fmt:?}), string.unpack({fmt:?}, s) end)()"
    )
}

fn numeral_case(r: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..1 + r.below(9) {
        s.push_str(r.pick(&[
            "0",
            "1",
            "9",
            "7",
            "x",
            "X",
            "a",
            "f",
            "F",
            "p",
            "P",
            ".",
            "+",
            "-",
            "e",
            "E",
            " ",
            "z",
            "ff",
            "8000000000000000",
        ]));
    }
    match r.below(5) {
        0 => format!("tonumber({s:?})"),
        1 => format!("tonumber({s:?}, {})", 2 + r.below(35)),
        2 => format!("({s:?} + 0)"),
        3 => format!("select(2, load('return ' .. {s:?}))"),
        _ => format!("math.tointeger({s:?})"),
    }
}

fn table_case(r: &mut Rng) -> String {
    let mut p = String::from(
        "(function() local t = {1, 2, 3, 4, 5} local log = {} local function L(ok, ...) log[#log + 1] = tostring(ok) .. ':' .. table.concat({tostring((...)), tostring(select(2, ...))}, ',') end\n",
    );
    let pos = || {
        [
            "0",
            "1",
            "2",
            "-1",
            "#t",
            "#t + 1",
            "#t + 2",
            "3.0",
            "1.5",
            "math.maxinteger",
            "math.mininteger",
            "'2'",
        ]
    };
    for _ in 0..1 + r.below(6) {
        let a = r.pick(&pos());
        let b = r.pick(&pos());
        let call = match r.below(7) {
            0 => format!("table.insert(t, {a}, 'x')"),
            1 => format!("table.insert(t, 'y')"),
            2 => format!("table.remove(t, {a})"),
            // C Lua's table.move runs to completion inside one C call (no step hook), so
            // huge ranges would hang the oracle: small positions only.
            3 => format!(
                "table.move(t, {}, {}, {})",
                r.pick(&pos()[..8]),
                r.pick(&pos()[..8]),
                r.pick(&pos()[..8])
            ),
            4 => format!("table.concat(t, ',', {a}, {b})"),
            5 => format!("select('#', table.unpack(t, {a}, {b}))"),
            _ => "table.remove(t)".to_owned(),
        };
        p.push_str(&format!("L(pcall(function() return {call} end))\n"));
    }
    p.push_str("local n = {} for i = 1, #t do n[i] = tostring(t[i]) end return table.concat(log, ' ') .. ' | ' .. table.concat(n, ',') end)()");
    p
}

fn utf8_case(r: &mut Rng) -> String {
    let mut s = String::new();
    for _ in 0..r.below(8) {
        s.push_str(r.pick(&[
            "a",
            "\\x80",
            "\\xbf",
            "\\xc2",
            "\\xc0",
            "\\xe2\\x82\\xac",
            "\\xed\\xa0\\x80",
            "\\xf4\\x90\\x80\\x80",
            "\\xf0\\x9f\\x98\\x80",
            "\\xff",
            "\\xe4\\xb8",
            "\\u{7FFFFFFF}",
            "\\0",
        ]));
    }
    let i = r.pick(&["1", "-1", "2", "0", "-10", "10"]);
    let j = r.pick(&["-1", "1", "3", "-3", "20"]);
    let lax = r.pick(&["", ", true"]);
    match r.below(4) {
        0 => format!("utf8.len(\"{s}\", {}, {j}{lax})", if s.is_empty() { "1" } else { i }),
        1 => format!("utf8.codepoint(\"{s}\", {i}, {j}{lax})"),
        2 => format!(
            "utf8.offset(\"{s}\", {}, {})",
            r.pick(&["0", "1", "2", "-1", "-2", "5"]),
            r.pick(&["1", "2", "3", "-1"])
        ),
        _ => format!(
            "(function() local o = {{}} for p, c in utf8.codes(\"{s}\"{lax}) do o[#o + 1] = p .. '=' .. c end return table.concat(o, ' ') end)()"
        ),
    }
}

fn for_case(r: &mut Rng) -> String {
    let b = &[
        "0",
        "1",
        "-1",
        "3",
        "10",
        "0.5",
        "2.5",
        "-2.5",
        "math.maxinteger",
        "math.mininteger",
        "math.maxinteger - 1",
        "math.mininteger + 1",
        "2^53",
        "1e300",
        "-1e300",
        "math.huge",
        "-math.huge",
        "'3'",
        "0/0",
    ];
    let (a, l) = (r.pick(b), r.pick(b));
    let step = r.pick(&[
        "",
        ", 1",
        ", -1",
        ", 2",
        ", -3",
        ", 0.5",
        ", -0.25",
        ", math.maxinteger",
        ", math.mininteger",
        ", 2^62",
        ", 0",
    ]);
    format!(
        "(function() local o, n = {{}}, 0 for i = {a}, {l}{step} do n = n + 1 if n > 6 then break end o[n] = math.type(i) .. ':' .. tostring(i) end return n, table.concat(o, ' ') end)()"
    )
}

fn sweep_program(r: &mut Rng) -> String {
    let mut p = String::from(
        "local out = {}\nlocal function try(f) local r = table.pack(pcall(f)) r.n = nil return r end\n",
    );
    for _ in 0..3 + r.below(8) {
        let e = match r.below(7) {
            0 => format_case(r),
            1 => pattern_case(r),
            2 => pack_case(r),
            3 => numeral_case(r),
            4 => table_case(r),
            5 => utf8_case(r),
            _ => for_case(r),
        };
        p.push_str(&format!("out[#out + 1] = try(function() return {e} end)\n"));
    }
    p.push_str("return out");
    p
}

fn fuzz_normalize(r: Result<String, String>) -> Result<String, String> {
    // MSVC prints NaN as "-nan(ind)" / "nan(ind)"; RemedyLua prints it as glibc does.
    let nan = |s: String| s.replace("nan(ind)", "nan").replace("NAN(IND)", "NAN");
    r.map(nan).map_err(nan)
}

#[test]
fn fuzz_library_areas_match_c_lua() {
    let env = |k: &str| std::env::var(k).ok().and_then(|s| s.parse::<u64>().ok());
    let iters = env("SWEEP_FUZZ_ITERS").unwrap_or(200);
    let base = env("SWEEP_FUZZ_SEED").unwrap_or(11);
    let seeds: Vec<u64> = match env("SWEEP_FUZZ_ONLY") {
        Some(s) => vec![s],
        None => (0..iters).map(|i| base.wrapping_mul(1_000_003).wrapping_add(i)).collect(),
    };
    let mut failures = Vec::new();
    for seed in &seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let program = sweep_program(&mut r);
        if std::env::var_os("SWEEP_TRACE").is_some() {
            eprintln!("seed {seed}\n{program}");
        }
        let theirs = fuzz_normalize(common::oracle(&program, 5_000_000));
        if theirs.as_ref().is_ok_and(|s| s.contains("step limit") || s.contains("not implemented")) {
            continue;
        }
        let ours = fuzz_normalize(common::remedy(&program, 50_000_000));
        if ours != theirs {
            failures.push(format!("seed {seed}:\n{program}\n  remedy: {ours:?}\n  c lua:  {theirs:?}"));
            if failures.len() >= 8 {
                break;
            }
        }
    }
    println!("{} sweep programs", seeds.len());
    assert!(failures.is_empty(), "{} mismatches:\n{}", failures.len(), failures.join("\n\n"));
}
