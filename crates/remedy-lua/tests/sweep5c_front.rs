//! Sweep 5c, area "front end": the lexer, parser, compiler, `check` and `format` attacked
//! as a hostile script author, through `load` inside a call (fuel, memory, deadline and
//! interrupt apply), the host's `Lua::load` (memory and interrupt) and the stateless
//! `remedy_lua::check` and `remedy_lua::format`. Each test is a finding (`S5C-FRONT-n`),
//! named `s5c_front_<n>_<what>`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::time::{Duration, Instant};

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Pos};

/// Runs `setup`, then `body` in a loop until `fuel` is spent: nanoseconds per unit of fuel.
fn ns_per_unit(setup: &str, body: &str, fuel: u64, fast: bool) -> f64 {
    let mut lua = Lua::new(Limits::default().memory(512 << 20).fuel(1 << 40), Libs::ALL);
    lua.set_fast_loop(fast);
    lua.set_print(|_| {});
    lua.exec(setup, "=setup").unwrap();
    lua.set_limits(lua.limits().fuel(fuel));
    let f = lua.load(format!("while true do {body} end"), "=body").unwrap();
    let start = Instant::now();
    let r = lua.call(f.value(), &[]);
    let t = start.elapsed();
    assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{body}: {r:?}");
    t.as_nanos() as f64 / fuel as f64
}

/// A typical library call's cost per unit on this machine and build.
fn baseline(fast: bool) -> f64 {
    ns_per_unit("t = {} s = 'abc'", "local x = tostring(#s) .. s t[1] = x", 300_000, fast)
}

/// Each case may cost at most this many times plain code per unit of fuel (as sweep 5).
const BOUND: f64 = 12.0;

/// Every `(setup, body)` costs at most `BOUND` times plain code per unit of `fuel`.
fn assert_metered_with(cases: &[(&str, &str)], fuel: u64) {
    for fast in [true, false] {
        let base = baseline(fast);
        let mut bad = Vec::new();
        for (setup, body) in cases {
            let mut ns = ns_per_unit(setup, body, fuel, fast);
            if ns > BOUND * base {
                // Noisy machine: the best of three.
                ns = ns.min(ns_per_unit(setup, body, fuel, fast)).min(ns_per_unit(setup, body, fuel, fast));
            }
            if ns > BOUND * base {
                bad.push(format!("{body}: {ns:.0} ns/unit = {:.0}x plain code", ns / base));
            }
        }
        assert!(bad.is_empty(), "fast {fast}, plain {base:.1} ns/unit:\n{}", bad.join("\n"));
    }
}

/// A RemedyLua chunk returning the length of `n` nested interpolations (`n + 1`).
fn nested_interp(n: usize) -> String {
    format!("return #{}1{}", "`a{".repeat(n), "}`".repeat(n))
}

/// Runs `src` (a chunk named `name`) before and after `format`: the same results.
fn same_after_format(src: &str, name: &str) {
    let f = remedy_lua::format(src, name).unwrap();
    let run = |s: &str| {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        match lua.exec(s, &format!("={name}")) {
            Ok(v) => v.iter().map(|x| format!("{:?}", lua.to_string_lossy(*x))).collect::<Vec<_>>().join(", "),
            Err(e) => format!("error: {e}"),
        }
    };
    assert_eq!(run(src), run(&f), "format changed what\n{src:?}\ndoes; formatted:\n{f:?}");
}

/// S5C-FRONT-1 (critical): an interpolated string's token was given the span `0..end`, so
/// every end position taken of it (`token_end`, after each expression) walked the whole
/// source before it: n interpolated strings parsed in time quadratic in the source, at
/// any nesting, for every entry (`load` in a call, the host's `load`, `check`, `format`).
/// `{`a`, `a`, ...}` with 80,000 of them (320 KB) took 6.8 s against 2.2M units of fuel:
/// 177x plain code per unit, growing with the source. Their end positions were wrong too
/// (the walk counted the line breaks before the string). The span now starts at the
/// backtick.
#[test]
fn s5c_front_1_interpolated_strings_parse_in_linear_time() {
    // The end of the token is where it ends: a message about it gives its own line.
    let d = remedy_lua::check(b"\n\nx `a`\n", "t.rlua");
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].pos, Pos { line: 3, col: 3 });
    assert_eq!(d[0].end, Pos { line: 3, col: 6 }, "{d:?}");
    assert_eq!(d[0].lua_message(), "t.rlua:3: syntax error near '`...`'");
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let v = lua.exec("local t = {`a`, `b{1}`, `\nc`} return #t, t[2] .. t[3]", "=t.rlua").unwrap();
    assert_eq!(lua.to_string_lossy(v[1]).unwrap(), "b1\nc");
    // Time per unit of fuel like plain code, many or nested.
    let flat = "src = 'local t = {' .. string.rep('`a`,', 40000) .. '}'";
    let calls = "src = string.rep('f(`a{1}`)\\n', 20000)";
    let cases = [(flat, "assert(load(src, '=x.rlua'))"), (calls, "assert(load(src, '=x.rlua'))")];
    assert_metered_with(&cases, 4_000_000);
    // The host's entries without fuel: a fraction of a second, not seconds.
    let src = format!("local t = {{{}}}", "`a`,".repeat(80_000));
    let t = Instant::now();
    assert!(remedy_lua::check(src.as_bytes(), "t.rlua").is_empty());
    assert!(remedy_lua::format(&src, "t.rlua").is_ok());
    assert!(Lua::new(Limits::unmetered(), Libs::NONE).load(&src, "=t.rlua").is_ok());
    assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
}

/// S5C-FRONT-2 (high): the compiler's scan work (a name looked up among many locals, a
/// label among many gotos: `diag::compile_work`) was paid as fuel but was not steps
/// between looks at the host's interrupt and the call's deadline, which came every 4,096
/// steps however long each was. With 80,000 locals each lookup scans 80,000, so a host's
/// `load` interrupted mid-compile stopped 54-131 ms late, growing with the locals and the
/// length of their names (anywhere in a window of 4,096 lookups: with 200,000 short
/// names 47 ms to 2.4 s; here, 100,000 long names, about 1 s). Every 16 units of scan
/// work are now a step: under 20 ms here, freeing the tree and the compiler's tables.
#[test]
fn s5c_front_2_scan_work_reads_the_interrupt() {
    // A short parse, a long compile: each use of the first local compares its long name
    // with 100,000 others of the same length.
    let name = |i: usize| format!("{}{i:07}", "v".repeat(64));
    let mut src: String =
        (0..500).map(|j| format!("local {}\n", (0..200).map(|i| name(j * 200 + i)).collect::<Vec<_>>().join(","))).collect();
    src += &format!("x = {}\n", name(0)).repeat(5000);
    for at in [400, 700] {
        let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::NONE);
        let h = lua.interrupt_handle();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(at));
            h.interrupt();
            Instant::now()
        });
        let r = lua.load(&src, "=t").map(|_| ());
        let end = Instant::now();
        let at = t.join().unwrap();
        assert!(matches!(r, Err(Error::Limit(LimitKind::Interrupted))), "{r:?}");
        let late = end.saturating_duration_since(at);
        assert!(late < Duration::from_millis(50), "stopped {late:?} after the interrupt");
    }
    // The same inside a call, by its deadline.
    let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::ALL);
    let s = lua.create_string(src.as_bytes()).unwrap();
    lua.set_global("src", s).unwrap();
    let start = Instant::now();
    let clock: remedy_lua::Clock = std::sync::Arc::new(move || start.elapsed());
    lua.set_time_limit(Some((clock, Duration::from_millis(500))));
    let r = lua.exec("return load(src, '=t')", "=c");
    assert!(matches!(r, Err(Error::Limit(LimitKind::Time))), "{r:?}");
    let late = start.elapsed().saturating_sub(Duration::from_millis(500));
    assert!(late < Duration::from_millis(50), "stopped {late:?} after the deadline");
}

/// S5C-FRONT-3 (high): `check` and `format`, which have no state, parsed with no memory
/// limit at all, so the quadratic memory of nested interpolations (S5SEC-1) was unbounded
/// there: 20,000 levels (100 KB) held 1.9 GB, 8,000 levels 315 MB. They now parse within
/// 1 KiB per source byte plus 64 MiB (far more than any other construct takes) and report
/// "not enough memory" past it.
#[test]
fn s5c_front_3_check_and_format_parse_within_memory() {
    let src = nested_interp(8000);
    let d = remedy_lua::check(src.as_bytes(), "t.rlua");
    assert!(d.len() == 1 && d[0].message == "not enough memory", "{d:?}");
    let r = remedy_lua::format(&src, "t.rlua");
    assert!(matches!(&r, Err(d) if d.message == "not enough memory"), "{:?}", r.map(|s| s.len()));
    // What fits is unchanged: shallower interpolations, and deep nesting of every other kind.
    let src = nested_interp(1000);
    assert!(remedy_lua::check(src.as_bytes(), "t.rlua").is_empty());
    assert_eq!(remedy_lua::format(&src, "t.rlua").unwrap(), src);
    let n = 200_000;
    for src in [
        format!("return {}1{}", "(".repeat(n), ")".repeat(n)),
        format!("return {}1{}", "{".repeat(n), "}".repeat(n)),
        format!("{}{}", "do ".repeat(n), "end ".repeat(n)),
    ] {
        assert!(remedy_lua::check(src.as_bytes(), "t.lua").is_empty(), "{}", &src[..40]);
        assert!(remedy_lua::format(&src, "t.lua").is_ok(), "{}", &src[..40]);
    }
    // The deep annotation parses within memory, and the type checker (written since)
    // reports the one real type error in it, not a memory or work limit.
    let src = format!("local x: {}number{} = 1", "{".repeat(n), "}".repeat(n));
    let d = remedy_lua::check(src.as_bytes(), "t.rlua");
    assert!(d.len() == 1 && d[0].message.starts_with("local 'x': expected {{{") && d[0].message.ends_with("got integer"), "{d:?}");
    assert!(remedy_lua::format(&src, "t.rlua").is_ok());
}

/// S5C-FRONT-4 (medium): `format` found the lines inside strings by scanning the text for
/// quotes again, and missed that a backtick string may break lines without a backslash:
/// the lines after its first were re-indented and trimmed, which changed the string. A
/// line inside a long string ending in a lone `\r` lost it (`\r\r\n` is two line breaks
/// there, `\r\n` one). The lines inside tokens now come from the tokens' spans.
#[test]
fn s5c_front_4_format_keeps_what_strings_hold() {
    let cases = [
        ("return `a\n   b  \n  c`", "t.rlua"),
        ("local x = 1\nif x then\nreturn `{x}\n      {`\n  {x}`}\n   `\nend", "t.rlua"),
        ("local s = `{ '`' }\n   x`\nreturn s", "t.rlua"),
        ("return [[a\r\r\n  b]]", "t.lua"),
        ("return \"a\\\n   b\", [==[\n x ]] ]=]\n  y]==]", "t.lua"),
        ("--[==[ a\n ]] ]=] ]==] return [[\n  z]]\n--[[\n x]]", "t.lua"),
        ("return \"a\\z\n     b\"", "t.lua"),
    ];
    for (src, name) in cases {
        same_after_format(src, name);
    }
    // Re-indenting still happens outside strings.
    assert_eq!(remedy_lua::format("if x then\ny = `a\n  b`\nend", "t.rlua").unwrap(), "if x then\n    y = `a\n  b`\nend");
}

/// The message `load` gives for `src` (chunk `=t`), in RemedyLua (inside a call and from
/// the host, which must agree) and in C Lua; "ok" when it compiles.
fn load_messages(src: &str) -> (String, String) {
    let quoted: String = src.bytes().map(|b| format!("\\{b}")).collect();
    let prog = format!("local f, e = load(\"{quoted}\", '=t') return e or 'ok'");
    let theirs = remedy_lua_oracle::run(&prog, 1_000_000).unwrap();
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let v = lua.exec(&prog, "=c").unwrap();
    let ours = lua.to_string_lossy(v[0]).unwrap();
    let host = match Lua::new(Limits::default(), Libs::NONE).load(src, "=t") {
        Ok(_) => "ok".to_owned(),
        Err(Error::Syntax(d)) => d.lua_message(),
        Err(e) => e.to_string(),
    };
    assert_eq!(ours, host, "load in a call and from the host differ for {src:?}");
    (ours, theirs)
}

/// Every source in `srcs` fails to load with C Lua's message.
fn same_load_errors_as_c(srcs: &[&str]) {
    let bad: Vec<String> = srcs
        .iter()
        .filter_map(|s| {
            let (ours, theirs) = load_messages(s);
            (ours != theirs).then(|| format!("{s:?}:\n  ours   {ours}\n  C Lua  {theirs}"))
        })
        .collect();
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// S5C-FRONT-5 (medium): a `local function` and a function expression were defined at the
/// line of `local`/`function`; C Lua's `localfunc` and `simpleexp` define them where the
/// token after the name (or after `function`) is, normally the `(`. Messages about an
/// unclosed one ("'end' expected (to close 'function' at line N)") and tracebacks
/// ("in function <t:N>") gave the wrong line when that was on a later line.
#[test]
fn s5c_front_5_functions_are_defined_where_their_parameters_start() {
    same_load_errors_as_c(&[
        "local function\nf\n(\n)\n",
        "local g = function\n(\n)\n",
        "return {function\n\n(a) return a",
        "function\nf\n(\n)\n",
        "local function f(\n)",
    ]);
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    match lua.exec("(function\n\n() error('x') end)()", "=t") {
        Err(Error::Runtime { traceback, .. }) => assert!(traceback.contains("function <t:3>"), "{traceback}"),
        other => panic!("{other:?}"),
    }
}

/// S5C-FRONT-6 (medium): a syntax error inside a label (`::a` then no `::`) after a label
/// of the same name was reported as "label 'a' already defined": the text before the
/// error is compiled to find what C Lua reports first, with what was open at the error
/// closed, and that closed the label. C Lua stops at the missing `::`, and it checks a
/// label only after reading the labels and `;` that follow, so the run of labels before
/// was never checked either.
#[test]
fn s5c_front_6_an_unfinished_label_is_not_checked() {
    same_load_errors_as_c(&[
        "::a:: ::a",
        "do goto l end ::l:: ::l",
        "::l::::l::::l::::l::::l:: ::for",
        "::a:: ;; ::a:: ; ::1",
        "local function f() goto a ::a:: ::",
        // Finished labels are still checked, last first.
        "::a:: ::a:: )",
        "::a::\n::a::\n::b::\n::b::\nx = 1",
    ]);
}

/// S5C-FRONT-7 (medium): "unknown attribute" and "multiple to-be-closed variables" were
/// reported before a lexical error in the token after the `>`, and at the line of the
/// name; C Lua reads that token before it checks (`checknext`), so the lexical error comes
/// first, and its message gives the line where that token ends.
#[test]
fn s5c_front_7_attribute_errors_come_after_the_next_token() {
    same_load_errors_as_c(&[
        "local t<eof>'",
        "local a <close>, b <close> \"abc",
        "local a <close>, b\n<close> = 1",
        "local a <foo> [[\n\n]]",
        "local a <foo>\n\n= 1",
    ]);
}

/// S5C-FRONT-8 (low): the refusal of a binary chunk named the chunk as given (`=t:1:`),
/// not as every other message names it (`t:1:`).
#[test]
fn s5c_front_8_binary_chunk_message_names_the_chunk() {
    let r = Lua::new(Limits::default(), Libs::NONE).load("\x1bLua", "=t");
    assert!(
        matches!(&r, Err(Error::Syntax(d)) if d.lua_message() == "t:1: attempt to load a binary chunk (bytecode is not accepted)"),
        "{:?}",
        r.map(|_| ())
    );
}

/// S5C-FRONT-9 (low): a parse counted 128 bytes a token against the memory left, but not
/// the bytes of a name or string, which the token holds and the syntax tree copies: a
/// 15 MB string literal loaded by a script under a 32 MiB limit was charged 30 MB (the
/// source and the constant) and held about 60 MB while compiling. They count now.
#[test]
fn s5c_front_9_string_bytes_count_against_the_parse() {
    for (mb, fits) in [(15, false), (4, true)] {
        let mut lua = Lua::new(Limits::default().fuel(1 << 30).memory(32 << 20).string_len(64 << 20), Libs::ALL);
        let src = format!("return [[{}]]", "x".repeat(mb << 20));
        let s = lua.create_string(src.as_bytes()).unwrap();
        lua.set_global("src", s).unwrap();
        let r = lua.exec("local f = assert(load(src)) return #f()", "=c");
        if fits {
            assert!(matches!(r.as_deref(), Ok([remedy_lua::Value::Int(n)]) if *n == (mb << 20) as i64), "{r:?}");
        } else {
            assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{r:?}");
        }
        assert!(lua.exec("return 1", "=after").is_ok());
    }
}

/// S5C-FRONT-10 (medium): a syntax error near a long string quoted it with its line breaks
/// as written; C Lua's message has each (`\r\n`, a lone `\r`) as one `\n`.
#[test]
fn s5c_front_10_long_string_near_text_has_lua_line_breaks() {
    same_load_errors_as_c(&["x = [[e]] [[a\r\nb\rc\n\rd]]", "local t = { [=[x]=] [==[\r\r\n\r]==] }", "x = [[\r\na\r\n]] [[\r\nb\r\n]]"]);
}

/// S5C-FRONT-11 (medium): `format` trimmed lines with `str::trim`, which also removes a
/// lone `\r`, a line break to Lua: the lines after it moved up one, so every error message
/// and traceback past it gave the wrong line. Only blanks are trimmed now, and not those
/// between a `\r` and a `\n` (which would make two line breaks one).
#[test]
fn s5c_front_11_format_keeps_lone_carriage_returns() {
    same_after_format("-- a\n    \r x = 1\nerror('here')", "t.lua");
    same_after_format("local x = 1 \r\nif x then\r  \rerror('here') end", "t.lua");
    same_after_format("local x = 1\r\r\nlocal y = 2 \r \nerror('here')", "t.lua");
}

/// S5C-FRONT-12: the host's interrupt stops a `load` of a very large source (here 300,000
/// locals in a run of `local` statements, about 2 MB) promptly wherever it lands: lexing, the byte count of the tokens,
/// parsing, the compiler's task loop or the run of statements that finish at once (which
/// never looked at the stop: 800,000 locals went on for a quarter second) or closing the
/// chunk's block, and what the stopped compile built is freed at the next call instead of
/// before it returns (70 to 400 ms late before, under about 16 ms now in a test build).
#[test]
fn s5c_front_12_huge_load_stops_promptly_wherever_interrupted() {
    let n = 300_000;
    let src: String = (0..n / 200)
        .map(|j| format!("local {}\n", (0..200).map(|i| format!("v{}", j * 200 + i)).collect::<Vec<_>>().join(",")))
        .collect();
    // How long the whole load takes, to spread the interrupts over it.
    let full = {
        let mut lua = Lua::new(Limits::unmetered().memory(4 << 30), Libs::NONE);
        let t = Instant::now();
        let _ = lua.load(&src, "=t");
        t.elapsed()
    };
    let mut worst = Duration::ZERO;
    for k in 1..=8u32 {
        let at = full * k / 10;
        let mut lua = Lua::new(Limits::unmetered().memory(4 << 30), Libs::NONE);
        let h = lua.interrupt_handle();
        let t = std::thread::spawn(move || {
            std::thread::sleep(at);
            h.interrupt();
            Instant::now()
        });
        let r = lua.load(&src, "=t").map(|_| ());
        let end = Instant::now();
        let fired = t.join().unwrap();
        if matches!(r, Err(Error::Limit(LimitKind::Interrupted))) {
            worst = worst.max(end.saturating_duration_since(fired));
        }
        // The state is usable after (what the stopped load left is freed here).
        h_reset_and_run(&mut lua);
    }
    assert!(worst < Duration::from_millis(50), "stopped {worst:?} after the interrupt (load takes {full:?})");
}

fn h_reset_and_run(lua: &mut Lua) {
    lua.interrupt_handle().reset();
    assert!(lua.load("return 1", "=ok").is_ok());
}
