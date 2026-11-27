//! Sweep 6, area "front end": regression tests for what it found in the lexer, parser,
//! compiler, `load`, `check` and `format`. Each test is named `s6_front_<n>_<what>` after
//! its finding (`S6-FRONT-n`) and fails without its fix.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

/// The process's peak resident memory (Linux `VmHWM`), in bytes; 0 elsewhere.
fn peak_rss() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            let l = s.lines().find(|l| l.starts_with("VmHWM"))?.to_owned();
            l.split_whitespace().nth(1)?.parse::<usize>().ok()
        })
        .map_or(0, |kb| kb << 10)
}

/// A reader that gives `pieces` pieces of `piece_kb` KiB each, then (when `nest`) loads
/// another such reader from inside itself, recording the depth reached in `depth`.
const NESTED_READER: &str = r#"
local piece_kb, pieces, nest = ...
local piece = string.rep("-", piece_kb * 1024)
depth = 0
local function mk()
  local n = 0
  return function()
    n = n + 1
    if n <= pieces then return piece end
    if n == pieces + 1 and nest then
      depth = depth + 1
      local f, e = load(mk())
      if e then error(e, 0) end
    end
    return nil
  end
end
return load(mk())
"#;

fn run_reader(lua: &mut Lua, piece_kb: i64, pieces: i64, nest: bool) -> Result<Vec<Value>, Error> {
    let f = lua.load(NESTED_READER, "=r").unwrap();
    lua.call(f.value(), &[Value::Int(piece_kb), Value::Int(pieces), Value::Bool(nest)])
}

/// S6-FRONT-1 (critical): `load` with a reader gathered the pieces in a buffer of its own
/// that was not charged to the memory account (only bounded by `string_len`, 16 MiB), and
/// a reader may call `load` with another reader, each level holding its own buffer: 15
/// pieces of 1 MiB a level then a nested `load`, under a 4 MiB cap, went 75 levels deep
/// and held 1.13 GB (280x the cap). The buffer (and the copy of a string chunk) is now
/// charged while it is held and released when `load` returns: a memory error after the
/// first few MiB, 12 MB peak.
#[test]
fn s6_front_1_load_source_buffers_are_charged() {
    // The charge itself: inside the reader, after 3 MiB of pieces, the account holds them.
    let mut lua = Lua::new(Limits::default().memory(64 << 20).fuel(1 << 32), Libs::ALL);
    let v = lua
        .exec(
            "local piece = string.rep('-', 1 << 20) collectgarbage() collectgarbage('stop')
             local before, during = collectgarbage('count')
             local n = 0
             local f = load(function()
               n = n + 1
               if n <= 3 then return piece end
               during = collectgarbage('count')
             end)
             assert(f)
             return (during - before) / 1024, (collectgarbage('count') - before) / 1024",
            "=t",
        )
        .unwrap();
    let held = match v[0] {
        Value::Num(x) => x,
        ref o => panic!("{o:?}"),
    };
    assert!(held >= 3.0, "the reader's buffer was charged {held:.2} MiB, not its 3 MiB");
    let after = match v[1] {
        Value::Num(x) => x,
        ref o => panic!("{o:?}"),
    };
    assert!(after < 0.5, "{after:.2} MiB still charged after load returned");
    // A string chunk's copy too, and released after.
    let used = lua.memory_used();
    let src = format!("--[[{}]] return 1", "x".repeat(2 << 20));
    let s = lua.create_string(src.as_bytes()).unwrap();
    lua.set_global("src", s).unwrap();
    lua.set_limits(lua.limits().memory(used + (3 << 20) / 2));
    let r = lua.exec("return load(src)", "=c");
    assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{:?}", r.map(|_| ()));
    // Nested readers under a 4 MiB cap: a memory error, not 75 levels of 15 MiB.
    let mut lua = Lua::new(Limits::default().memory(4 << 20).fuel(1_000_000_000), Libs::ALL);
    let r = run_reader(&mut lua, 1024, 15, true);
    assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{:?}", r.map(|_| ()));
    // What fits still loads, and leaves nothing charged.
    let mut lua = Lua::new(Limits::default().memory(16 << 20).fuel(1_000_000_000), Libs::ALL);
    lua.exec("collectgarbage()", "=gc").unwrap();
    let base = lua.memory_used();
    let r = run_reader(&mut lua, 256, 8, false).unwrap();
    assert!(matches!(r[0], Value::Func(_)), "{r:?}");
    lua.exec("collectgarbage()", "=gc").unwrap();
    assert!(lua.memory_used() < base + (64 << 10), "{} -> {}", base, lua.memory_used());
}

/// S6-FRONT-1, measured: the nested readers' peak resident memory stays near the cap.
#[test]
#[ignore = "measures resident memory: run alone"]
fn s6_front_1_nested_readers_stay_resident_within_the_cap() {
    let mut lua = Lua::new(Limits::default().memory(4 << 20).fuel(1_000_000_000), Libs::ALL);
    let r = run_reader(&mut lua, 1024, 15, true);
    assert!(r.is_err());
    let peak = peak_rss();
    assert!(peak < 64 << 20, "peak RSS {} MB", peak >> 20);
}

/// S6-FRONT-2 (critical): `format` wrote four spaces a nesting level on every line with no
/// bound: `"do\n" * n .. "end\n" * n` at n = 16,000 (112 KB of source) gave 1.02 GB of
/// output. The output is now bounded like the parse (`host_memory`: 1 KiB per source byte
/// plus 64 MiB) and fails with "not enough memory" past it; normal formatting is the same.
#[test]
fn s6_front_2_format_output_is_bounded() {
    let n = 16_000;
    let src = format!("{}{}", "do\n".repeat(n), "end\n".repeat(n));
    let r = remedy_lua::format(&src, "t.lua");
    assert!(matches!(&r, Err(d) if d.message == "not enough memory"), "{:?}", r.map(|s| s.len()));
    // Nesting whose indentation fits is formatted as before.
    let n = 2000;
    let src = format!("{}{}", "do\n".repeat(n), "end\n".repeat(n));
    let f = remedy_lua::format(&src, "t.lua").unwrap();
    let want: String = (0..n)
        .map(|i| format!("{}do\n", "    ".repeat(i)))
        .chain((0..n).rev().map(|i| format!("{}end\n", "    ".repeat(i))))
        .collect();
    assert_eq!(f, want);
    assert_eq!(remedy_lua::format("if x then\ny = 1\nend", "t.lua").unwrap(), "if x then\n    y = 1\nend");
}

/// S6-FRONT-5 (medium): the text of an interpolated expression went through
/// `String::from_utf8_lossy`, so a byte that is not UTF-8 written raw inside it became
/// U+FFFD: `` #`{"\xFF"}` `` (the byte itself in the source) was 3, not 1. The bytes are
/// kept now, in both front ends.
#[test]
fn s6_front_5_interpolated_expressions_keep_raw_bytes() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let src: &[u8] = b"return #`{\"\xFF\"}`, `{'a\xFE\xFFb'}`";
    let v = lua.exec(src, "=t.rlua").unwrap();
    assert!(matches!(v[0], Value::Int(1)), "{v:?}");
    assert_eq!(lua.str_bytes(v[1]).unwrap(), b"a\xFE\xFFb");
    // Valid UTF-8 and blanks are as before.
    let v = lua.exec("return #`{'\u{e9}'}`, `{ 1 }`", "=t.rlua").unwrap();
    assert!(matches!(v[0], Value::Int(2)), "{v:?}");
    let d = remedy_lua::check(b"return `{ \t }`", "t.rlua");
    assert!(d.len() == 1 && d[0].message.contains("empty expression"), "{d:?}");
}

/// S6-FRONT-6 (medium): a bad escape in a backtick string (`interp`, and `skip_interp` for
/// one nested in an expression) did not set where the escape and the string start, as
/// `short_string` does, so they were left from the last quoted string read:
/// `local s = "x"\nreturn `ab\q`` reported column 4294967247, and the "near" text was all
/// the source since that string (a 2.4 MB message after 2.4 MB of code). Now column 11
/// and near '`ab\q'.
#[test]
fn s6_front_6_backtick_escape_errors_point_at_the_escape() {
    let src = "local s = \"x\"\nreturn `ab\\q`";
    let d = remedy_lua::check(src.as_bytes(), "t.rlua");
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].pos, remedy_lua::Pos { line: 2, col: 11 }, "{d:?}");
    assert_eq!(d[0].lua_message(), "t.rlua:2: invalid escape sequence near '`ab\\q'");
    // Nested in an expression, and after an expression part.
    let d = remedy_lua::check(b"local s = \"x\"\nreturn `{ `a{1}b\\q` }`", "t.rlua");
    assert_eq!(d[0].pos, remedy_lua::Pos { line: 2, col: 17 }, "{d:?}");
    assert_eq!(d[0].lua_message(), "t.rlua:2: invalid escape sequence near '`\\q'");
    let d = remedy_lua::check(b"local s = \"x\"\nreturn `a{1}b\\q`", "t.rlua");
    assert_eq!(d[0].lua_message(), "t.rlua:2: invalid escape sequence near '`b\\q'");
    // A long program between: the message stays short (it held all of it).
    let src = format!("local s = \"x\"\n{}return `ab\\q`", "x = 1\n".repeat(100_000));
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    match lua.load(&src, "=t.rlua") {
        Err(Error::Syntax(d)) => {
            assert_eq!(d.lua_message(), "t.rlua:100002: invalid escape sequence near '`ab\\q'")
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// The message `load` inside a call gives for `src` (chunk `=t`), as hex bytes, in
/// RemedyLua (with the fast loop on and off) and in C Lua; "ok" when it compiles.
fn load_message_bytes(src: &[u8]) -> (Result<String, String>, Result<String, String>) {
    let quoted: String = src.iter().map(|b| format!("\\{b}")).collect();
    let prog = format!(
        "local f, e = load(\"{quoted}\", '=t') \
         return e and (e:gsub('.', function(c) return string.format('%02x', c:byte()) end)) or 'ok'"
    );
    (super::common::remedy(&prog, 1_000_000), super::common::oracle(&prog, 1_000_000))
}

/// S6-FRONT-7 (low): three messages differed from C Lua's. A `goto`'s line (in "no
/// visible label" and "jumps into the scope") was the `goto` keyword's, where C Lua's
/// `gotostat` takes the line of the label's name (`"goto\nl"`: 1, C Lua 2). An assignment
/// to a const variable was reported at the variable's line, where C Lua's `restassign`
/// checks it with the token after it read (`"local x <const> = 1\nx\n=\n2"`: 2, C Lua 3).
/// An escape error's "near" text had bytes that are not UTF-8 replaced by U+FFFD, where C
/// Lua's message has them as written. All as C Lua now, in both front ends.
#[test]
fn s6_front_7_goto_const_and_escape_messages_match_c_lua() {
    let cases: &[&[u8]] = &[
        b"goto\nl",
        b"do goto\n\nl end",
        b"do goto\nl\nlocal x\n::l:: print(x) end",
        b"::a::\n::b::\ngoto\na\n::a::",
        b"local x <const> = 1\nx\n=\n2",
        b"local x <const> = 1\nx\n,y=\n2",
        b"local x <const> = 1\nlocal y\ny\n,\nx\n=\n2",
        b"local x <const> = 1\nfunction f() x\n-- c\n= 2 end",
        b"local x <close> = nil\nx = 1",
        b"x = '\xff\\q'",
        b"x = 'a\xfe\xff\\xzz'",
        b"x = '\xc3\xa9\\400'",
        b"x = '\xff\nb'",
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter_map(|src| {
            let (ours, theirs) = load_message_bytes(src);
            (ours != theirs).then(|| {
                format!("{:?}:\n  ours   {ours:?}\n  C Lua  {theirs:?}", String::from_utf8_lossy(src))
            })
        })
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    // The host's message is text: the bytes replaced, as before.
    match Lua::new(Limits::default(), Libs::NONE).load(b"x = '\xff\\q'", "=t") {
        Err(Error::Syntax(d)) => {
            assert_eq!(d.lua_message(), "t:1: invalid escape sequence near ''\u{fffd}\\q'")
        }
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// S6-FRONT-7, the case left open: `function x() ... end` with `x` const was reported at
/// the `function` line, before the body was compiled; C Lua's `funcstat` checks it after
/// the body, with the token after `end` read (`"local x <const> = 1\nfunction x()\nend\n\n"`:
/// ours 2, C Lua 5), so an error in the body comes first. Both front ends.
#[test]
fn s6_front_7_const_function_statement_line_matches_c_lua() {
    let cases: &[&[u8]] = &[
        b"local x <const> = 1\nfunction x()\nend\n\n",
        b"local x <const> = 1\nfunction x()\nend y = 2",
        b"local x <const> = 1\nfunction x()\ngoto q\nend\n\n",
        b"local x <close> = nil\nfunction x() end\n\n-- c\ny = 1",
        b"local x <const> = 1\nfunction f() function x()\nend\nend",
        b"local x <const> = 1\nfunction x()",
    ];
    let bad: Vec<String> = cases
        .iter()
        .filter_map(|src| {
            let (ours, theirs) = load_message_bytes(src);
            (ours != theirs).then(|| {
                format!("{:?}:\n  ours   {ours:?}\n  C Lua  {theirs:?}", String::from_utf8_lossy(src))
            })
        })
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// A source of `n` levels of the nesting `kind` (S6-FRONT-3's measurements).
fn nested_source(kind: &str, n: usize) -> String {
    match kind {
        "ifelse" => format!("local x\n{}{}", "if x then x=1 else ".repeat(n), "end ".repeat(n)),
        "do" => format!("{}{}", "do ".repeat(n), "end ".repeat(n)),
        "local_fn" => format!("{}{}", "local function f() ".repeat(n), "end ".repeat(n)),
        "stmts" => format!("local x\n{}", "x = 1\n".repeat(n)),
        "locals" => (0..n).map(|i| format!("local v{i} = 1\n")).collect(),
        "dots" => format!("return a{}", ".b".repeat(n)),
        _ => unreachable!(),
    }
}

/// S6-FRONT-3 (high): compiling took 2 to 3.3 times the memory limit. The parse counted 128
/// bytes a token for the token and the tree, where a block's vector of statements alone
/// took 1.5 KB (at least four 384-byte statements), a one-letter name 64 bytes counted as
/// 2, and growing a vector held its old buffer and the new at once; the tree was not
/// charged while it was compiled, and a function being compiled was charged only its
/// struct, not its code, locals and scopes (about 1 KB a level). An `if x then x=1 else`
/// chain 45,000 deep (1 MB) held 222 MB under a 64 MiB limit (3.3x), `do` x 100,000 197 MB
/// (2.9x), 100,000 locals 146 MB (2.2x). The tree's vectors and boxes are counted as
/// allocated (one element first, then doubling, checked before they grow), the lexer's,
/// parser's and compiler's stacks before they grow, the tree is charged while it is
/// compiled, and an open function's buffers every 64 steps: at most 1.1x now for every
/// construct measured that loads (`examples/frontmem.rs`), 1.16x for one that fails.
/// Statements are 128 bytes, not 384 (numeric `for` and `op=` boxed), so ordinary code
/// takes less than before: 1 MB of it loads under 64 MiB as before, in 63 MB, not 94.
///
/// Deterministic part: what really needs more than the limit no longer loads under it,
/// and ordinary code still does.
#[test]
fn s6_front_3_compile_memory_is_charged_as_allocated() {
    // Each of these takes 45 to 60 MB to compile (resident, as allocated now): they loaded
    // under 32 MiB before (charged 128 bytes a token), and must not now. (`ifelse` 30,000
    // took 45 MB while every token was kept, 31 MB once only those still read are: 40,000
    // takes 46 MB.)
    for (kind, n) in [("ifelse", 40_000), ("do", 90_000), ("local_fn", 35_000), ("stmts", 80_000)] {
        let src = nested_source(kind, n);
        let r = Lua::new(Limits::unmetered().memory(32 << 20), Libs::NONE).load(&src, "=t").map(|_| ());
        assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{kind} x {n}: {r:?}");
        // Inside a call too.
        let mut lua = Lua::new(Limits::default().memory(32 << 20).fuel(1 << 34), Libs::ALL);
        let s = lua.create_string(src.as_bytes()).unwrap();
        lua.set_global("src", s).unwrap();
        let r = lua.exec("return load(src)", "=c").map(|_| ());
        assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{kind} x {n} in a call: {r:?}");
        // With room for what it really takes, it loads.
        assert!(
            Lua::new(Limits::unmetered().memory(160 << 20), Libs::NONE).load(&src, "=t").is_ok(),
            "{kind} x {n}"
        );
    }
    // Ordinary code: 200 KB of it still loads under 32 MiB, and deep nesting on a small stack.
    let unit = "local function f(a, b)\n  local t = {x = a, y = b, \"s\", [a] = b}\n  for j = 1, #t do\n    \
                if t[j] then t.x = t.x + j elseif j > 2 then b = b or {} else t[j] = nil end\n  end\n  \
                return function(x) return x + a * b end\nend\n";
    let src = unit.repeat(200_000 / unit.len());
    assert!(Lua::new(Limits::default().memory(32 << 20), Libs::NONE).load(&src, "=t").is_ok());
    super::common::small_stack(|| {
        for kind in ["ifelse", "do", "local_fn"] {
            let src = nested_source(kind, 3000);
            assert!(Lua::new(Limits::default(), Libs::NONE).load(&src, "=t").is_ok(), "{kind}");
            assert!(remedy_lua::check(src.as_bytes(), "t.lua").is_empty(), "{kind}");
        }
    });
}

/// Runs `kind` x `n` loaded under `limit` in a child process: (loaded, peak resident bytes).
fn load_in_child(kind: &str, n: usize, limit: usize) -> (bool, usize) {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args(["sweep6_front::s6_front_3_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env("S6_FRONT_3_CHILD", format!("{kind}:{n}:{limit}"))
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let field = |k: &str| text.lines().find_map(|l| l.split(k).nth(1).map(str::to_owned)).unwrap_or_default();
    (field("S6F3_LOADED=") == "true", field("S6F3_PEAK=").parse().unwrap_or(0))
}

/// The child of `s6_front_3_compile_stays_resident_within_the_cap` (run only from it).
#[test]
#[ignore = "run in a child process by s6_front_3_compile_stays_resident_within_the_cap"]
fn s6_front_3_child() {
    let Ok(spec) = std::env::var("S6_FRONT_3_CHILD") else { return };
    let mut it = spec.split(':');
    let kind = it.next().unwrap();
    let n: usize = it.next().unwrap().parse().unwrap();
    let limit: usize = it.next().unwrap().parse().unwrap();
    let src = nested_source(kind, n);
    let r = Lua::new(Limits::unmetered().memory(limit), Libs::NONE).load(&src, "=t");
    println!("S6F3_LOADED={}", r.is_ok());
    println!("S6F3_PEAK={}", peak_rss());
}

/// S6-FRONT-3, measured: compiling each construct at its worst size (the largest that
/// loads, and a little larger) stays within 1.3 times the limit, resident, in a process of
/// its own (Linux only). Before: 3.3x for the `if` chain, 2.9x for `do`, 2.2x for locals.
#[test]
fn s6_front_3_compile_stays_resident_within_the_cap() {
    if !std::path::Path::new("/proc/self/status").exists() {
        return;
    }
    let limit = 64 << 20;
    let mut bad = Vec::new();
    for (kind, ns) in [
        ("ifelse", &[39_000, 45_000][..]),
        ("do", &[110_000, 125_000][..]),
        ("local_fn", &[36_000, 39_000][..]),
        ("locals", &[79_000, 100_000][..]),
        ("stmts", &[107_000, 125_000][..]),
        ("dots", &[130_000, 190_000][..]),
    ] {
        for &n in ns {
            let (loaded, peak) = load_in_child(kind, n, limit);
            assert!(peak > 0, "{kind} x {n}: no measurement");
            if peak as f64 > 1.3 * limit as f64 {
                bad.push(format!("{kind} x {n} (loaded {loaded}): {:.2}x", peak as f64 / limit as f64));
            }
        }
    }
    assert!(bad.is_empty(), "peak resident memory past 1.3x the limit:\n{}", bad.join("\n"));
}

/// How late a host `load` of `src` stops after the interrupt, at its worst over `points`
/// moments spread along the load.
fn worst_stop(src: &str, points: u32) -> std::time::Duration {
    use std::time::{Duration, Instant};
    let full = {
        let mut lua = Lua::new(Limits::unmetered().memory(4 << 30), Libs::NONE);
        let t = Instant::now();
        assert!(lua.load(src, "=t").is_ok());
        t.elapsed()
    };
    let mut worst = Duration::ZERO;
    for k in 1..=points {
        let at = full * k / (points + 1);
        let mut lua = Lua::new(Limits::unmetered().memory(4 << 30), Libs::NONE);
        let h = lua.interrupt_handle();
        let t = std::thread::spawn(move || {
            std::thread::sleep(at);
            h.interrupt();
            Instant::now()
        });
        let r = lua.load(src, "=t").map(|_| ());
        let end = Instant::now();
        let fired = t.join().unwrap();
        if matches!(r, Err(Error::Limit(LimitKind::Interrupted))) {
            worst = worst.max(end.saturating_duration_since(fired));
        }
        // Usable after: what the load left is freed here.
        lua.interrupt_handle().reset();
        assert!(lua.load("return 1", "=ok").is_ok());
    }
    worst
}

/// S6-FRONT-4 (high): a host `load` of a few MB of hostile source stopped 80 to 660 ms
/// after the interrupt: a `.name` chain was parsed in one step with no look at the
/// interrupt (`a.b.b...` x 3M: 661 ms), a table constructor's field list grew by one
/// `realloc` of hundreds of MB (`{1,1,...}` x 3M: 190 ms), its fields were counted in one
/// pass, a function's string-constant map grew in one insert (58 ms at 460,000), the
/// tokens and syntax tree were freed unwatched (after a successful compile, and the
/// tokens after a stop in the lexer), and freeing a big tree left glibc 300 ms of merging
/// for the next allocation. Now every one of those is a step or a stretch of steps, what a
/// host compile built is freed at its next call a piece at a time, watching the interrupt
/// (or when the state is dropped), and on 6 MB sources of nested `do`, `.name` chains,
/// 3M-entry tables, 400k locals, 500k keys and 600k string constants (examples/
/// frontstop.rs) the worst stop is 17 to 36 ms on a loaded 4-core machine.
#[test]
fn s6_front_4_huge_hostile_loads_stop_promptly() {
    let _heavy = super::common::memory_heavy();
    let cases = [
        format!("return a{}", ".b".repeat(1_500_000)),
        format!("return {{{}}}", "1,".repeat(1_500_000)),
        format!("return {{{}}}", (0..300_000).map(|i| format!("\"s{i}\",")).collect::<String>()),
    ];
    for src in &cases {
        let worst = worst_stop(src, 6);
        assert!(
            worst < std::time::Duration::from_millis(60),
            "{}...: stopped {worst:?} after the interrupt",
            &src[..12]
        );
    }
}

/// S6-FRONT-4, follow-up: what a host `load` leaves to free a piece at a time (its syntax
/// tree, a big function's constant maps, what a stopped compile built) sat uncharged until
/// the next call from the host: right after loading 400,000 constants the account was
/// 50 MB, the same as once they were freed. Now 222 MB, back to 50 MB once freed. It is charged to the state's memory now
/// as the compile returns, and released as it is freed (at the next call from the host).
#[test]
fn s6_front_4_deferred_frees_are_charged_until_freed() {
    let src = format!(
        "return {{{}}}",
        (0..200_000).map(|i| format!("\"s{i}\", a{i} = {i}.5,")).collect::<String>()
    );
    let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::NONE);
    lua.exec("return 1", "=warm").unwrap();
    let before = lua.memory_used();
    let f = lua.load(&src, "=t").unwrap();
    let loaded = lua.memory_used();
    // The next call from the host frees what waited, and gives its charge back.
    lua.exec("return 1", "=after").unwrap();
    let after = lua.memory_used();
    let waited = loaded - after;
    assert!(waited > 20 << 20, "{} MB charged for what the load left to free", waited >> 20);
    // What stays is the function and its constants.
    assert!(after > before, "{before} -> {after}");
    drop(f);
    // A stopped load's leftovers are charged too, and freed likewise.
    let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::NONE);
    lua.exec("return 1", "=warm").unwrap();
    let base = lua.memory_used();
    let h = lua.interrupt_handle();
    let full = {
        let t = std::time::Instant::now();
        let _ = Lua::new(Limits::unmetered().memory(1 << 30), Libs::NONE).load(&src, "=t");
        t.elapsed()
    };
    let t = std::thread::spawn(move || {
        std::thread::sleep(full * 2 / 3);
        h.interrupt();
    });
    let r = lua.load(&src, "=t").map(|_| ());
    t.join().unwrap();
    if matches!(r, Err(Error::Limit(LimitKind::Interrupted))) {
        let stopped = lua.memory_used();
        lua.interrupt_handle().reset();
        lua.exec("return 1", "=after").unwrap();
        // (The constants it had made are garbage, the collector's to free.)
        lua.collect_garbage().unwrap();
        let freed = lua.memory_used();
        assert!(stopped > freed + (4 << 20), "{base}: stopped {stopped}, freed {freed}");
        assert!(freed < base + (1 << 20), "{base}: stopped {stopped}, freed {freed}");
    }
}

/// LR-6 (long-run fuzz, `.rlua`): `x ..= e` is `x = x .. (e)`, but a runtime error in it
/// was numbered with the statement's first line, where the desugaring numbers it with the
/// line of the concatenation `e` ends with (C Lua's `codeconcat` merges the two into that
/// instruction): `g2 ..= ('x') .. (("a\z⏎ '") .. "b")` said line 1, `g2 = g2 .. (...)`
/// line 2. The compound form now merges too, in both front ends.
#[test]
fn s6_front_lr6_compound_concat_errors_where_its_desugaring_does() {
    let rhs = [
        "('x') .. ((\"a\\z\n '\") .. \"b\")",
        "\"a\" .. (\"b\" ..\n\"c\")",
        "\"a\" ..\n\"b\"",
        "(\"a\" .. \"b\"\n.. 'c')",
        "{}\n",
        "('x') .. (('y') ..\n\n 'z' .. {})",
    ];
    let message = |src: &str| -> String {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        match lua.exec(src, "=t.rlua") {
            Err(Error::Runtime { message, .. }) => message,
            other => format!("{:?}", other.map(|v| v.len())),
        }
    };
    for target in ["g2", "t.k", "l"] {
        let pre = match target {
            "t.k" => "t = {} ",
            "l" => "local l ",
            _ => "",
        };
        for e in rhs {
            let src = format!("{pre}{target} ..= {e}");
            let compound = message(&src);
            let desugared = message(&format!("{pre}{target} = {target} .. ({e})"));
            assert_eq!(compound, desugared, "{target} ..= {e:?}");
            // The recursive front end numbers it the same (its code and lines).
            #[cfg(feature = "front-oracle")]
            assert_eq!(
                remedy_lua::__front::compile_old(src.as_bytes(), "t.rlua"),
                remedy_lua::__front::compile_new(src.as_bytes(), "t.rlua"),
                "{src:?}"
            );
            // And the result when it runs.
            let ok = message(&format!("{pre}{target} = '' {target} ..= {e} return {target}"));
            let ok2 = message(&format!("{pre}{target} = '' {target} = {target} .. ({e}) return {target}"));
            assert_eq!(ok, ok2, "{target} ..= {e:?}");
        }
    }
    // Interpolated right-hand sides (the long-run fuzzer, `SWEEP1_SEED=10010`): `..=` was
    // numbered with the statement's line where its desugaring into plain Lua (parts joined
    // by `..`, each expression in `tostring`) is numbered with the line after the part
    // before the last, as C Lua numbers the last `..` (`g3["k"] ..= `a{(1⏎)}b``: 2, not 3).
    // The interpolation is built in its target register and its concatenation numbered
    // there, so the `..` of `..=` merges into it. Both front ends.
    let interps = [
        ("`a{(1\n)}b`", "(\"a\" .. tostring((1\n)) .. \"b\")"),
        ("`a{1}\nb`", "(\"a\" .. tostring(1) .. \"\\nb\")"),
        ("`{(1\n)}{2}`", "(tostring((1\n)) .. tostring(2))"),
        ("`a{(1\n)}`", "(\"a\" .. tostring((1\n)))"),
        ("`{(1\n)}`", "(tostring((1\n)))"),
        ("`a{1}{(2\n)}c`", "(\"a\" .. tostring(1) .. tostring((2\n)) .. \"c\")"),
        ("`a{`b{(1\n)}c`}d`", "(\"a\" .. tostring((\"b\" .. tostring((1\n)) .. \"c\")) .. \"d\")"),
    ];
    let line = |src: &str, name: &str| -> String {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        match lua.exec(src, name) {
            Err(Error::Runtime { message, .. }) => message.split(':').nth(1).unwrap_or_default().to_owned(),
            other => format!("{:?}", other.map(|v| v.len())),
        }
    };
    for target in ["g3[\"k\"]", "gq", "lq"] {
        let pre = if target == "lq" { "local lq = nil\n" } else { "g3 = {} gq = nil\n" };
        for (r, l) in interps {
            let src = format!("{pre}{target} ..= {r}");
            let ours = line(&src, "=t.rlua");
            let sugar = line(&format!("{pre}{target} = {target} .. {l}"), "=t");
            assert_eq!(ours, sugar, "{src:?}");
            let plain = line(&format!("{pre}{target} = {target} .. {r}"), "=t.rlua");
            assert_eq!(plain, sugar, "{target} = {target} .. {r:?}");
            #[cfg(feature = "front-oracle")]
            assert_eq!(
                remedy_lua::__front::compile_old(src.as_bytes(), "t.rlua"),
                remedy_lua::__front::compile_new(src.as_bytes(), "t.rlua"),
                "{src:?}"
            );
        }
    }
}

/// The process's resident memory now (Linux `VmRSS`), in bytes; 0 elsewhere.
fn rss_now() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<usize>().ok())
        })
        .map_or(0, |kb| kb << 10)
}

/// The 400,000-constant source of the deferred-free tests.
fn constants_source() -> String {
    format!("return {{{}}}", (0..200_000).map(|i| format!("\"s{i}\", a{i} = {i}.5,")).collect::<String>())
}

/// The child of `s6_front_4_deferred_charge_is_what_is_held` (run only from it): loads the
/// source and prints the account and resident memory around it.
#[test]
#[ignore = "run in a child process by s6_front_4_deferred_charge_is_what_is_held"]
fn s6_front_4_exact_child() {
    if std::env::var("S6_FRONT_4_EXACT").is_err() {
        return;
    }
    let src = constants_source();
    let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::NONE);
    lua.exec("return 1", "=warm").unwrap();
    let (live0, rss0) = (lua.memory_used(), rss_now());
    let f = lua.load(&src, "=t").unwrap();
    let (loaded, rss1) = (lua.memory_used(), rss_now());
    // A cap that holds what is really live and left: what the load added, resident (an
    // upper bound of the bytes really allocated), and 8 MB for the string below.
    let cap = live0 + rss1.saturating_sub(rss0) + (8 << 20);
    lua.set_limits(lua.limits().memory(cap));
    let s = lua.create_string(vec![b'x'; 4 << 20]).map(|_| ());
    lua.set_limits(lua.limits().memory(1 << 30));
    let with_string = lua.memory_used();
    lua.exec("return 1", "=after").unwrap();
    let after = lua.memory_used();
    drop(f);
    println!("S6F4X {live0} {loaded} {} {after} {with_string} {}", rss1.saturating_sub(rss0), s.is_ok());
}

/// S6-FRONT-4, follow-up 2: what a load left to free was charged an estimate (172 MB for
/// the 400,000 constants, more than the load had made resident), so a host allocation right
/// after a huge load could meet the memory limit with the real bytes well under it. It is
/// charged what it holds now: every buffer at its capacity, every box and string of the
/// tree (`ast::nodes_bytes`), a map's bucket array as std allocates it and its keys, given
/// back piece by piece as they are freed. Checked in a child process: the leftovers'
/// charge is at most what the load added to resident memory (an upper bound of what it
/// allocated; tolerance 0) and at least half the resident bytes besides what stays live
/// (the rest is allocator overhead: 109 MB charged against 134 MB here), and a 4 MB
/// string fits under a cap of the live and resident bytes plus 8 MB right after the load.
/// Before: 172 MB charged, more than the 188 MB the whole load made resident less live.
#[test]
fn s6_front_4_deferred_charge_is_what_is_held() {
    if !std::path::Path::new("/proc/self/status").exists() {
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args([
            "sweep6_front::s6_front_4_exact_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("S6_FRONT_4_EXACT", "1")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // (The harness may print the test's name on the same line first.)
    let line =
        text.lines().find_map(|l| l.split("S6F4X ").nth(1)).unwrap_or_else(|| panic!("no result: {text}"));
    let v: Vec<&str> = line.split(' ').collect();
    let n = |i: usize| v[i].parse::<usize>().unwrap();
    let (live0, loaded, resident, after, with_string) = (n(0), n(1), n(2), n(3), n(4));
    let fits = v[5] == "true";
    let leftovers = loaded - after;
    assert!(leftovers > 8 << 20, "{} MB charged for the leftovers", leftovers >> 20);
    assert!(
        loaded - live0 <= resident,
        "charged {} MB after the load, {} MB made resident",
        (loaded - live0) >> 20,
        resident >> 20
    );
    assert!(
        fits,
        "a 4 MB string did not fit under the live and resident bytes plus 8 MB ({} MB charged)",
        (loaded - live0) >> 20
    );
    // Measured, not a fraction of it: at least half of what the load made resident
    // besides what stays live (the rest being the allocator's own overhead and pages).
    let others = resident.saturating_sub(after - live0);
    assert!(
        leftovers * 2 >= others,
        "{} MB charged for leftovers, {} MB resident besides what stays",
        leftovers >> 20,
        others >> 20
    );
    let _ = with_string;
}

/// S6-FRONT-4 with the residuals' piecewise frees: every deferred piece (a host load's tree
/// and maps, a stopped compile's leftovers, a tree freed piecewise inside a call) is charged
/// once, by what it holds, and given back as it is freed. Right after a host load the
/// account is what stays live (as a load inside a call that finished leaves it, nothing
/// waiting) plus exactly what is released at the next call from the host; a compile stopped
/// inside a call leaves at most what the host load leaves; and every path is back at the
/// baseline once all is freed and collected. A piece charged twice (on deferring and again
/// when adopted, or kept charged past its free) breaks one of these.
#[test]
fn s6_front_4_deferred_pieces_are_charged_once() {
    let _heavy = super::common::memory_heavy();
    let src = format!(
        "return {{{}}}",
        (0..200_000).map(|i| format!("\"s{i}\", a{i} = {i}.5,")).collect::<String>()
    );
    // (memory right after, memory once the next host call freed what waited, whether it loaded)
    let run = |fuel: Option<u64>| {
        let mut lua = Lua::new(Limits::default().fuel(1 << 40).memory(1 << 30), Libs::ALL);
        let s = lua.create_string(src.as_str()).unwrap();
        lua.set_global("SRC", s).unwrap();
        lua.exec("return 1", "=warm").unwrap();
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        let ok = match fuel {
            None => lua.load(&src, "=t").is_ok(),
            Some(fuel) => {
                lua.set_limits(lua.limits().fuel(fuel));
                let r = lua.exec("local f = load(SRC) return 1", "=t").is_ok();
                lua.set_limits(lua.limits().fuel(1 << 40));
                r
            }
        };
        let right = lua.memory_used();
        lua.exec("return 1", "=free").unwrap();
        let freed = lua.memory_used();
        lua.collect_garbage().unwrap();
        assert_eq!(lua.memory_used(), base, "fuel {fuel:?}: not back at the baseline once freed");
        (right, freed, ok)
    };
    // A load inside a call that finishes frees its tree there: nothing waits. (The `exec`
    // that frees what waited makes a little of its own, the same on every path.)
    let (live, live_freed, ok) = run(Some(1 << 40));
    assert!(ok);
    let exec_cost = live_freed - live;
    let (host_right, host_freed, ok) = run(None);
    assert!(ok);
    let host_left = host_right - (host_freed - exec_cost);
    assert!(host_left > 20 << 20, "{} MB left by the host load", host_left >> 20);
    // Each piece once: what stays live, plus what the next call gives back (within the few
    // hundred bytes of garbage the two ways of loading leave apart; any piece is megabytes).
    let each_once = (live + host_left).abs_diff(host_right);
    assert!(each_once < 4096, "a host load: live {live} plus leftovers {host_left}, charged {host_right}");
    // A load inside a call stopped by the fuel limit while compiling: what it built waits.
    let (mut lo, mut hi) = (1u64, 1u64 << 34);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if run(Some(mid)).2 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    let mut stopped = 0;
    for k in 1..=400u64 {
        let (right, freed, ok) = run(Some(hi.saturating_sub(k * 20_000)));
        assert!(!ok);
        let left = right - (freed - exec_cost);
        if left > 1 << 20 {
            assert!(
                left <= host_left,
                "a stopped compile charged {left} for its leftovers, the whole host load's are {host_left}"
            );
            stopped += 1;
            if stopped == 3 {
                break;
            }
        }
    }
    assert!(stopped > 0, "no stop left anything to free (needed {hi})");
}

/// LR-6, the long-run fuzzer's cousins (`fuzz_rlua`, `SWEEP1_SEED=10015` and `10016`, found
/// on a tree without 2632807): an interpolated string whose parts span lines (a `\z` string
/// or a long string inside `{}`), as the last operand of an ordinary concatenation chain
/// (`g3 = (string.gsub(A .. (g1) .. (p6) .. B .. C .. (`{"..\z⏎ "}{..}`), ..))`: t:5, plain
/// Lua t:6) or the right-hand side of `..=` with a field target (`g2["k"] ..= ...`: t:14,
/// plain Lua t:15), was numbered where its desugaring is not. Both come from the line the
/// interpolation's own concatenation is given (`ast::interp_join_line`, the line of its last
/// `..` written out), which a chain or a `..=` merges into. Each shape is compared with its
/// plain-Lua desugaring (parts joined by `..`, expressions in `tostring`) and with C Lua, in
/// both front ends.
#[test]
fn s6_front_lr6_interp_operand_lines_match_desugaring() {
    // (`.rlua` operand, its plain-Lua desugaring)
    let interps = [
        ("`{\"\\093\\x64\\z\n \"}{1}`", "(tostring(\"\\093\\x64\\z\n \") .. tostring(1))"),
        ("`{\"a\\z\n\n \"}`", "(tostring(\"a\\z\n\n \"))"),
        ("`x{[[a\nb]]}y`", "(\"x\" .. tostring([[a\nb]]) .. \"y\")"),
        ("`{1}{[[a\n\nb]]}`", "(tostring(1) .. tostring([[a\n\nb]]))"),
        ("`{[[a\n]]}{\"b\\z\n\"}c`", "(tostring([[a\n]]) .. tostring(\"b\\z\n\") .. \"c\")"),
        ("`a{(1\n)}b`", "(\"a\" .. tostring((1\n)) .. \"b\")"),
        ("`{`{\"q\\z\n \"}`}{2}`", "(tostring((tostring(\"q\\z\n \"))) .. tostring(2))"),
    ];
    // (`.rlua` statement, its plain-Lua form) with the operand at `@`.
    let shapes = [
        (
            "g3 = (string.gsub('a' .. (g1) .. (p6) .. 'b' .. 'c' .. (@), 'x', 'y'))",
            "g3 = (string.gsub('a' .. (g1) .. (p6) .. 'b' .. 'c' .. (@), 'x', 'y'))",
        ),
        ("g3 = 'a' .. (p6) .. @", "g3 = 'a' .. (p6) .. @"),
        ("g3 = @ .. (p6)", "g3 = @ .. (p6)"),
        ("g3 = 'a' .. @ .. (p6) .. 'b'", "g3 = 'a' .. @ .. (p6) .. 'b'"),
        ("g2['k'] ..= @", "g2['k'] = g2['k'] .. @"),
        ("g2.k ..= @ .. 'z'", "g2.k = g2.k .. (@ .. 'z')"),
        ("gq ..= @", "gq = gq .. @"),
        ("local lq\nlq ..= 'a' .. @", "local lq\nlq = lq .. ('a' .. @)"),
    ];
    let pre = "g1 = 'g' p6 = {} g2 = {} gq = nil\n";
    let message = |src: &str, name: &str| -> String {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        match lua.exec(src, name) {
            Err(Error::Runtime { message, .. }) => message.replacen("t.rlua:", "t:", 1),
            other => format!("{:?}", other.map(|v| v.len())),
        }
    };
    let mut bad = vec![];
    for (sr, sl) in shapes {
        for (r, l) in interps {
            let rl = format!("{pre}{}", sr.replace('@', r));
            let plain = format!("{pre}{}", sl.replace('@', l));
            let ours = message(&rl, "=t.rlua");
            let sugar = message(&plain, "=t");
            let quoted: String = plain.bytes().map(|b| format!("\\{b}")).collect();
            let prog = format!("local f = assert(load(\"{quoted}\", '=t')) local ok, e = pcall(f) return e");
            let c = super::common::oracle(&prog, 1_000_000).map(|s| s.trim_matches('"').to_owned());
            if ours != sugar || c.as_deref() != Ok(sugar.as_str()) {
                bad.push(format!("{rl:?}:\n  .rlua  {ours}\n  plain  {sugar}\n  C Lua  {c:?}"));
            }
            // The recursive front end numbers it the same (its code and lines).
            #[cfg(feature = "front-oracle")]
            assert_eq!(
                remedy_lua::__front::compile_old(rl.as_bytes(), "t.rlua"),
                remedy_lua::__front::compile_new(rl.as_bytes(), "t.rlua"),
                "{rl:?}"
            );
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// A reader giving `src` `step` bytes at a time, counting its calls in `calls`.
const READER: &str = "local calls = 0 local function reader(src, step) local i = 0 return function() \
                      calls = calls + 1 if i >= #src then return nil end local j = math.min(i + step, #src) \
                      local p = src:sub(i + 1, j) i = j return p end end ";

/// Sources for `load`'s reader: valid, with syntax errors early and late, lexical errors,
/// and tokens a piece boundary can cut (names, numbers, long brackets, `..`, `...`, `==`).
const READ_CASES: &[&str] = &[
    "return 1",
    "local t = {a = 1, [2] = 3; 'x', b = 2} return t.a + #t, ... --[==[ c ]==] .. 'z'",
    "local s = [[\nlong ]] .. [==[ ]] ]==] return #s, 0x1p4, 3.25e-1, 1 // 2, 2^3 ~= 7, ...",
    "x = = 1",
    "return 1 return 2",
    "local a = 1\nlocal b = a + \nfunction",
    "return 'unfinished",
    "return [[unfinished",
    "return 3..4",
    "-- only a comment",
    "",
];

/// FX2-3 (`fuzz_mutated` seed 900000, mut-001): `load` read its reader to the end before
/// parsing, where C Lua reads it as its lexer needs (`zgetc`) and stops at the first
/// syntax error. A reader that never ends (`function() return "return 7" end`) filled
/// memory where C Lua gives a syntax error, and a reader's calls were seen to differ.
/// Now the lexer pulls pieces as it reads (one character past each token, as `llex`),
/// and the parser reads a token as it moves onto it (`luaX_next`): the same calls, results
/// and messages as C Lua, whole, a few bytes or one byte at a time. (Not yet for an error
/// the compiler finds, such as an assignment to a `<const>`: C Lua compiles as it parses
/// and stops reading there; RemedyLua compiles once the text is parsed, so it reads on to
/// the end. The message is C Lua's, checked below; the reader's calls are more.)
#[test]
fn fx2_3_load_reads_its_reader_as_c_lua_does() {
    let mut bad = Vec::new();
    for src in READ_CASES {
        for step in [1 << 20, 1, 2, 3, 7] {
            let p = format!(
                "{READER} local f, e = load(reader({src:?}, {step})) return calls, f and select('#', f()) or e"
            );
            let (ours, theirs) = (super::common::remedy(&p, 1 << 30), super::common::oracle(&p, 1 << 30));
            if ours != theirs {
                bad.push(format!("{src:?} step {step}: ours {ours:?}, C Lua {theirs:?}"));
            }
        }
    }
    // Readers that never end: the first syntax error stops them.
    for piece in ["return 7", "x = 1 y ", "local t = {", "--[[ c ]] return"] {
        let p =
            format!("local n = 0 local f, e = load(function() n = n + 1 return {piece:?} end) return n, e");
        let (ours, theirs) = (super::common::remedy(&p, 1 << 30), super::common::oracle(&p, 1 << 30));
        if ours != theirs {
            bad.push(format!("endless {piece:?}: ours {ours:?}, C Lua {theirs:?}"));
        }
    }
    // LR-28 (fuzz_mutated seed 10033): an endless reader, and finite ones failing or not.
    for p in [
        "local f, e = load(function() return 'return 7' end) return e",
        "local n = 0 local f, e = load(function() n = n + 1 if n < 3 then return 'return 7 ' end end) return n, e",
        "local n = 0 local f = load(function() n = n + 1 if n < 3 then return 'x = 1 ' end end) return n, type(f)",
    ] {
        let (ours, theirs) = (super::common::remedy(p, 1 << 30), super::common::oracle(p, 1 << 30));
        if ours != theirs {
            bad.push(format!("{p}: ours {ours:?}, C Lua {theirs:?}"));
        }
    }
    // Errors the compiler finds: C Lua's message, whole and a byte at a time.
    for src in ["local x <const> = 1 x = 2", "for i = 1, 3 do local y = i end goto nowhere"] {
        for step in [1 << 20, 1] {
            let p = format!("{READER} local f, e = load(reader({src:?}, {step})) return e");
            let (ours, theirs) = (super::common::remedy(&p, 1 << 30), super::common::oracle(&p, 1 << 30));
            if ours != theirs {
                bad.push(format!("{src:?} step {step}: ours {ours:?}, C Lua {theirs:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// FX2-3: what `load` charges for a reader's text is the same however the text is split:
/// past each call (the reader's own work and a unit), a split costs what it costs for as
/// many spaces, on both paths.
#[test]
fn fx2_3_load_charges_a_text_the_same_however_it_is_split() {
    for src in READ_CASES.iter().take(3) {
        for fast in [true, false] {
            let fuel = |s: &str, step: usize| {
                let p = format!("{READER} assert(load(reader({s:?}, {step}))) return calls");
                let (r, used, _) = super::common::remedy_measured(&p, 1 << 30, fast);
                assert!(r.is_ok(), "{r:?}");
                used
            };
            let spaces = " ".repeat(src.len());
            for step in [1, 2, 7] {
                let split = fuel(src, step) - fuel(src, 1 << 20);
                let calls = fuel(&spaces, step) - fuel(&spaces, 1 << 20);
                assert_eq!(split, calls, "{src:?} step {step} fast {fast}");
            }
        }
    }
}
