//! Sweep 3, area "spec": the whole crate against the Lua 5.4 manual and C Lua 5.4.7.
//!
//! `tests/lua/sweep3/spec/*.lua` holds cases (split at `-- case:` lines) run in RemedyLua
//! (fast loop on and off, which must agree on result, fuel and memory) and in the C Lua
//! oracle; every result must be equal. `SWEEP3_SPEC_DIR` points the test at another
//! directory of cases.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

/// Every case may use `L(src)`: the load error message, or "ok"; and `E(f, ...)`: the
/// error (or first result) of `pcall(f, ...)`.
const PRELUDE: &str = "local function L(s, ...) local f, e = load(s, ...) if f then return 'ok' end return e end local function E(...) return select(2, pcall(...)) end ";

#[test]
fn spec3_cases_match_c_lua() {
    let dir = std::env::var("SWEEP3_SPEC_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua/sweep3/spec"));
    let mut files: Vec<_> =
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    let (mut count, mut failures) = (0, Vec::new());
    for f in files.iter().filter(|p| p.extension().is_some_and(|x| x == "lua")) {
        let src = format!("\n{}", std::fs::read_to_string(f).unwrap().replace("\r\n", "\n"));
        for chunk in src.split("\n-- case:").skip(1) {
            let (name, body) = chunk.split_once('\n').unwrap_or((chunk, ""));
            count += 1;
            let p = format!("{PRELUDE}{body}");
            let theirs = common::oracle(&p, 50_000_000);
            let ours = common::remedy(&p, 500_000_000);
            if ours != theirs && !common::c_limit_only_outcome(&ours, &theirs) {
                let file = f.file_name().unwrap().to_string_lossy();
                failures.push(format!("{file}: {}\n  remedy: {ours:?}\n  c lua:  {theirs:?}", name.trim()));
            }
        }
    }
    println!("{count} spec cases");
    assert!(failures.is_empty(), "{} of {count} differ:\n{}", failures.len(), failures.join("\n"));
}

/// S3SPEC-8: `%a` with modifiers, against glibc's output (the oracle's MSVC build has no
/// modifiers for `%a`, so these are fixed texts). `#` keeps the point: "0x1.p+0".
#[test]
fn spec3_format_a_modifiers() {
    let cases = [
        ("%#a", "1.0", "0x1.p+0"),
        ("%#.0a", "1.0", "0x1.p+0"),
        ("%#A", "2.0", "0X1.P+1"),
        ("%#a", "1.5", "0x1.8p+0"),
        ("%#a", "1/0", "inf"),
        ("%.0a", "1.5", "0x2p+0"),
        ("%.1a", "1.09375", "0x1.2p+0"),
        ("%010a", "1.0", "0x00001p+0"),
        ("%+a", "1.0", "+0x1p+0"),
        ("%-12a|", "1.0", "0x1p+0      |"),
        ("%.2a", "5e-324", "0x0.00p-1022"),
        ("%a", "5e-324", "0x0.0000000000001p-1022"),
    ];
    for (fmt, v, want) in cases {
        for fast in [true, false] {
            let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            let r = lua.exec(format!("return string.format('{fmt}', {v})"), "=t").unwrap();
            assert_eq!(lua.to_string_lossy(r[0]).unwrap(), want, "{fmt} of {v}");
        }
    }
}

/// S3SPEC-9: `warn` follows C's control messages: warnings start off (`luaL_newstate`),
/// "@on"/"@off" switch them, and only a one-piece message is a control message.
#[test]
fn spec3_warn_control_messages() {
    let src = r#"warn("a") warn("@on") warn("b", "c") warn("@on", "x") warn("@x") warn("@off") warn("d") warn("@on") warn("e")"#;
    for fast in [true, false] {
        let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(src, "=t").unwrap();
        assert_eq!(lua.take_output(), "Lua warning: bc\nLua warning: @onx\nLua warning: e\n");
    }
}

/// S3SPEC-11 (critical in debug builds): a 0xFF lead byte followed by 7 continuation
/// bytes made `utf8.decode` shift a u32 by 35, which panics with overflow checks on. Every
/// utf8 function that decodes must refuse it as C Lua does.
#[test]
fn spec3_utf8_ff_lead_does_not_panic() {
    let p = r#"local s = "\xFF\x80\x80\x80\x80\x80\x80\x80"
        local t = "\xFE\x80\x80\x80\x80\x80\x80"
        return utf8.len(s), utf8.len(s, 1, -1, true), utf8.len(t, 1, -1, true), E(utf8.codepoint, s), E(utf8.codepoint, s, 1, 1, true),
            E(function() for _ in utf8.codes(s, true) do end end), E(function() for _ in utf8.codes(t) do end end), utf8.offset(s, 2)"#;
    let p = format!("local function E(...) return select(2, pcall(...)) end {p}");
    assert_eq!(common::remedy(&p, 10_000_000), common::oracle(&p, 10_000_000));
}

/// S3SPEC-12: a hexadecimal numeral with more digits than fit was rounded without the
/// dropped digits (and twice for subnormals). C99 requires hex conversions correctly
/// rounded; glibc's strtod does it. The oracle's MSVC strtod drops the digits too, so these
/// are fixed texts (glibc's results).
#[test]
fn spec3_hex_float_rounding_uses_dropped_digits() {
    let cases = [
        ("0x1.000000000000080000001", "1.0000000000000002"),
        ("0x1.0000000000000800000000000000000001", "1.0000000000000002"),
        ("0x1.00000000000008", "1"),
        ("0x1.fffffffffffff7ffffffff", "1.9999999999999998"),
        ("0x0.000000000000080000001p-1022", "4.9406564584124654e-324"),
        ("0x1.0000000001p-1075", "4.9406564584124654e-324"),
        ("0x1p-1075", "0"),
    ];
    for (s, want) in cases {
        for src in [
            format!("return string.format('%.17g', tonumber('{s}'))"),
            format!("return string.format('%.17g', {s})"),
        ] {
            let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
            let r = lua.exec(src.as_str(), "=t").unwrap();
            assert_eq!(lua.to_string_lossy(r[0]).unwrap(), want, "{src}");
        }
    }
}

/// Strict Lua 5.4 (`Limits::strict_5_4`): none of `LUA_COMPAT_5_3`. The oracle is built
/// with it (as the stock makefile builds Lua), so strict mode has fixed expectations, which
/// are what a 5.4 build without the compat flag gives.
#[test]
fn spec3_strict_5_4_has_no_compat() {
    let src = r#"
        local t = setmetatable({}, {__lt = function() return true end})
        local ok, e = pcall(function() return t <= t end)
        local names = {}
        for _, n in ipairs{"atan2", "cosh", "sinh", "tanh", "pow", "frexp", "ldexp", "log10"} do
            names[#names + 1] = tostring(math[n])
        end
        local u = setmetatable({}, {__le = function() return 1 end})
        return tostring(ok) .. "|" .. e .. "|" .. table.concat(names, ",") .. "|" .. tostring(u <= u)"#;
    for fast in [true, false] {
        let mut lua =
            remedy_lua::Lua::new(remedy_lua::Limits::default().strict_5_4(true), remedy_lua::Libs::ALL);
        lua.set_fast_loop(fast);
        let r = lua.exec(src, "=t").unwrap();
        assert_eq!(
            lua.to_string_lossy(r[0]).unwrap(),
            "false|t:3: attempt to compare two table values|nil,nil,nil,nil,nil,nil,nil,nil|true"
        );
    }
    // And the default has them.
    let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
    let r = lua
        .exec("return math.pow(2, 3), setmetatable({}, {__lt = function() return false end}) <= {}", "=t")
        .unwrap();
    assert_eq!(remedy_lua::Value::Num(8.0), r[0]);
    assert_eq!(remedy_lua::Value::Bool(true), r[1]);
}

/// `Lua::set_warnings`: the host turns warnings on (or off) without a script's "@on".
#[test]
fn spec3_host_warning_switch() {
    let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
    lua.set_warnings(true);
    lua.exec(r#"warn("a") warn("@off") warn("b")"#, "=t").unwrap();
    lua.set_warnings(true);
    lua.exec(r#"warn("c")"#, "=t").unwrap();
    lua.set_warnings(false);
    lua.exec(r#"warn("d")"#, "=t").unwrap();
    assert_eq!(lua.take_output(), "Lua warning: a\nLua warning: c\n");
}
