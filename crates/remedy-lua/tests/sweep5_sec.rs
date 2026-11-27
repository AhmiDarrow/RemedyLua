//! Sweep 5, area "budgets and metering": fuel and memory bypasses found by
//! attacking every budget again, and the cycle-4 structural guards checked by planting the
//! bugs they exist to catch. Each test is a finding (`S5SEC-n`). Every program runs with
//! the fast loop on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::time::Instant;

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

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

/// Each case may cost at most this many times plain code per unit of fuel (as sweep 4).
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

/// S5SEC-1 (critical): an interpolated expression's text was copied and lexed again once
/// per enclosing interpolation, and skipping a nested one copied every inner level's text
/// again: compiling n nested interpolations took time cubic in n and held memory quadratic
/// in n, against `load`'s charge linear in the source. 4,000 levels (20 KB) took 15 s for
/// 140k units of fuel (6,500x plain code), and 8,000 levels held 325 MB under a 16 MiB
/// memory limit (unbounded with unmetered fuel). Now the copies are scan work paid as
/// fuel, the skip copies nothing, and the levels' texts and tokens count against the
/// memory the parse may use.
#[test]
fn s5sec_1_nested_interpolations_are_charged_and_capped() {
    // Results unchanged.
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        lua.set_fast_loop(fast);
        let v = lua.exec(nested_interp(300), "=t.rlua").unwrap();
        assert!(matches!(v[0], Value::Int(301)), "{:?}", v[0]);
        let v = lua.exec("return `a{` {1} `}b`, `{ 'x' }`", "=t.rlua").unwrap();
        assert_eq!(lua.to_string_lossy(v[0]).unwrap(), "a 1 b");
        // A blank expression is still refused, also nested inside another's text.
        assert!(lua.load("return `a{ `b{  }` }`", "=t.rlua").is_err());
        assert!(lua.load("return `a{ \u{a0} }`", "=t.rlua").is_err());
    }
    // Fuel: time per unit like plain code at every depth.
    let setups: Vec<String> = [1000, 3000].iter().map(|n| format!("src = {:?}", nested_interp(*n))).collect();
    let cases: Vec<(&str, &str)> =
        setups.iter().map(|s| (s.as_str(), "assert(load(src, '=x.rlua'))")).collect();
    assert_metered_with(&cases, 20_000_000);
    // Memory: 8,000 levels do not fit in 16 MiB, from a script or from the host, and the
    // state works afterwards.
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::unmetered().memory(16 << 20), Libs::ALL);
        lua.set_fast_loop(fast);
        let src = nested_interp(8000);
        assert!(matches!(lua.load(&src, "=t.rlua"), Err(Error::Limit(LimitKind::Memory))));
        let g = lua.create_string(src.as_bytes()).unwrap();
        lua.set_global("src", g).unwrap();
        let r = lua.exec("return load(src, '=t.rlua')", "=t");
        assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{r:?}");
        assert!(lua.exec("return 1 + 1", "=after").is_ok());
    }
    // The interrupt stops a long one (the host's own load, no fuel, room for 1 GiB).
    let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::ALL);
    let src = nested_interp(20_000);
    let h = lua.interrupt_handle();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        h.interrupt();
        Instant::now()
    });
    let r = lua.load(&src, "=t.rlua").map(|_| ());
    let end = Instant::now();
    let at = t.join().unwrap();
    assert!(matches!(r, Err(Error::Limit(LimitKind::Interrupted))), "{r:?}");
    assert!(end.saturating_duration_since(at) < std::time::Duration::from_millis(50));
}

/// S5SEC-2 (medium): a float passed where a library function wants a string was written
/// as text for free (`check_str`): `warn` with 200k float arguments, reused through a
/// vararg function, ran at 12x plain code per unit of fuel.
#[test]
fn s5sec_2_float_string_arguments_are_charged() {
    let setup = "t = {} for i = 1, 100000 do t[i] = i + 0.123456789 end";
    let cases: Vec<(String, &str)> = ["warn", "string.char"]
        .iter()
        .map(|f| {
            (
                format!("{setup} function run(...) while true do pcall({f}, ...) end end"),
                "run(table.unpack(t))",
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = cases.iter().map(|(s, b)| (s.as_str(), *b)).collect();
    assert_metered_with(&refs, 4_000_000);
}

/// S5SEC-3 (high): output a library function built was charged only when it returned
/// (`OutBuf`, SEC4-1..7's fix), so one call ran on past the fuel limit for up to a whole
/// string's length of output (`string_len` / 32 bytes a unit: half a million units under
/// the default limits). A call with 20,000 units left did 15x to 45x that much work:
/// `string.format` of fifteen 1 MiB strings, `string.pack` of them, `table.concat` with a
/// 4 KB separator, `gsub` with a function returning 1 MiB. The buffer is now paid as it
/// grows (`OutBuf::pay`, at each element, conversion, item or match).
#[test]
fn s5sec_3_output_is_paid_as_it_grows() {
    let s = "s = string.rep('x', 1 << 20) f = string.rep('%s', 15)";
    let args = "s,s,s,s,s,s,s,s,s,s,s,s,s,s,s";
    let format = format!("local _ = string.format(f, {args})");
    let pack = format!("local _ = string.pack(string.rep('s4', 15), {args})");
    let cases = [
        (s, format.as_str()),
        (s, pack.as_str()),
        (
            "t = {} for i = 1, 4000 do t[i] = i end sep = string.rep(',', 4000)",
            "local _ = table.concat(t, sep)",
        ),
        (s, "local _ = string.gsub('abcdefghijklmnop', '.', function() return s end)"),
        (s, "local _ = string.gsub('abcdefghijklmnop', '.', {a = s, b = s, c = s, d = s, e = s, f = s})"),
    ];
    assert_metered_with(&cases, 20_000);
}

/// S5SEC-4 (guard gap): no small-stack run held deep data of most shapes, so a traversal
/// made recursive per element went unseen. Planted: the collector marking a closure's
/// upvalues at once (one native frame per closure in a chain). Every small-stack test and
/// fuzzer passed; a 100k closure chain then killed the process. This runs every deep
/// shape a script can build through every traversal of it (collection, full and in steps,
/// weak tables, errors and tracebacks, `tostring`, `to_data`, closing) on a 64 KiB stack.
#[test]
fn s5sec_4_deep_data_on_a_small_stack() {
    const N: usize = 100_000;
    let shapes = [
        // Tables nested through values, keys and metatables.
        format!("local t = {{}} for i = 1, {N} do t = {{t}} end keep = t"),
        format!("local t = {{}} for i = 1, {N} do t = {{[t] = true}} end keep = t"),
        format!("local t = {{}} for i = 1, {N} do t = setmetatable({{}}, t) end keep = t"),
        format!("local t = {{}} for i = 1, {N} do t = setmetatable({{}}, {{__index = t}}) end keep = t"),
        // Closures through upvalues; native closures (iterators) holding the next.
        format!(
            "local f = function() end for i = 1, {N} do local g = f f = function() return g end end keep = f"
        ),
        format!(
            "local f = {{}} for i = 1, {N} do local p = f f = coroutine.wrap(function() return p end) end keep = f"
        ),
        format!(
            "local f = print for i = 1, {N} do f = select(1, ipairs({{f}})) == f and f or {{f, string.gmatch('a', '.')}} end keep = f"
        ),
        // Coroutines whose stacks hold the previous one, suspended and dead.
        format!(
            "local c = 1 for i = 1, {} do local p = c c = coroutine.create(function() coroutine.yield(p) end) coroutine.resume(c) end keep = c",
            N / 4
        ),
        // Weak tables over chains: values, keys (ephemerons), both.
        format!(
            "local w = setmetatable({{}}, {{__mode = 'v'}}) local t = {{}} for i = 1, {N} do t = {{t}} w[i] = t end keep = {{w, t}}"
        ),
        format!(
            "local w = setmetatable({{}}, {{__mode = 'k'}}) local p = {{}} keep = p for i = 1, {N} do local k = {{}} w[p] = k p = k end keep2 = w"
        ),
        format!(
            "local w = setmetatable({{}}, {{__mode = 'kv'}}) local t = {{}} for i = 1, {N} do local u = {{t}} w[u] = t t = u end keep = {{w, t}}"
        ),
    ];
    let traverse = "collectgarbage() collectgarbage('step', 0) collectgarbage() \
                    local ok, e = pcall(error, keep) assert(not ok) \
                    local _ = tostring(keep) \
                    keep, keep2 = nil, nil collectgarbage() collectgarbage() return 'ok'";
    for fast in [true, false] {
        for shape in &shapes {
            let src = format!("{shape} {traverse}");
            let r = super::common::small_stack(move || {
                let mut lua = Lua::new(Limits::default().memory(512 << 20).fuel(1 << 32), Libs::ALL);
                lua.set_fast_loop(fast);
                lua.set_gc_stress(false);
                let r = lua.exec(&src, "=deep").map(|v| lua.to_string_lossy(v[0]).unwrap_or_default());
                // The host's walks over it too.
                let keep = lua.exec(src.replace(traverse, "return keep"), "=deep2");
                if let Ok(v) = keep {
                    let _ = lua.to_data(v[0], remedy_lua::DataLimits::default());
                }
                drop(lua);
                r
            });
            assert_eq!(r.as_deref(), Ok("ok"), "fast {fast}: {}", &shape[..shape.len().min(80)]);
        }
    }
    // Deep call stacks: an error and its traceback through every frame, `__close` chains
    // run on an error and on `coroutine.close`, nested `pcall`s.
    let stacks = [
        "local function f(n) if n == 0 then error('x') end return (f(n - 1)) end return select(2, pcall(f, 1900))",
        "local m = {__close = function() end} local function f(n) local x <close> = setmetatable({}, m) \
         if n == 0 then error('x') end return (f(n - 1)) end return select(2, pcall(f, 1900))",
        "local m = {__close = function() end} local function f(n) local x <close> = setmetatable({}, m) \
         if n == 0 then coroutine.yield() end return (f(n - 1)) end \
         local co = coroutine.create(f) coroutine.resume(co, 1900) return tostring(coroutine.close(co))",
        "local function f(n) if n == 0 then error('x') end return select(2, pcall(f, n - 1)) end return f(1900)",
    ];
    for fast in [true, false] {
        for src in stacks {
            let r = super::common::small_stack(move || {
                let mut lua = Lua::new(Limits::default(), Libs::ALL);
                lua.set_fast_loop(fast);
                let r = lua.exec(src, "=deep").map(|v| lua.to_string_lossy(v[0]).unwrap_or_default());
                // And uncaught, with its traceback.
                let _ = lua.exec(src.replace("pcall(f, 1900)", "f(1900)"), "=deep");
                r
            });
            assert!(r.is_ok(), "fast {fast}: {src}: {r:?}");
        }
    }
}

/// Guard for SEC4-1..7's class, broadened (S5SEC-5): `stdlib_never_charges_from_a_built_buffer`
/// looked for five buffer names only, so a planted `lua.charge(v.len() as u64 / 64)?` after
/// `table.concat`'s work (skipped by any error before it) passed it, and so did the
/// fuel-against-time property (no family raised after building). Now any `charge` of a
/// length of a buffer the same function makes or grows is refused, whatever its name.
#[test]
fn s5sec_5_stdlib_never_charges_from_a_built_buffer() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/stdlib");
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        bad.extend(
            built_buffer_charges(&text)
                .into_iter()
                .filter(|l| !ALLOWED.iter().any(|(f, code)| *f == file && l.contains(code)))
                .map(|l| format!("{}: {l}", path.display())),
        );
    }
    assert!(bad.is_empty(), "charge from a built buffer (use OutBuf or owe):\n{}", bad.join("\n"));
    // The check itself catches the planted shapes.
    let planted = [
        "fn f(lua: &mut Lua) -> R {\n    let mut v = Vec::new();\n    build(&mut v)?;\n    lua.charge(v.len() as u64 / 64)?;\n}",
        "fn f(lua: &mut Lua) -> R {\n    let mut s = String::new();\n    s.push_str(\"x\");\n    lua.charge_bytes(s.len())?;\n}",
        "fn f(lua: &mut Lua) -> R {\n    let mut result = Vec::with_capacity(8);\n    lua.charge(result.len() as u64)?;\n}",
        "fn f(lua: &mut Lua, b: &[u8]) -> R {\n    let mut o = b.to_vec();\n    o.extend_from_slice(b);\n    lua.charge(o.len() as u64)?;\n}",
    ];
    for p in planted {
        assert_eq!(built_buffer_charges(p).len(), 1, "not caught:\n{p}");
    }
}

/// Charges from a built buffer's length that are right: (file, code).
const ALLOWED: &[(&str, &str)] = &[
    // `load`'s compile, paid before it starts; the source's pieces were paid as each was
    // read from the reader function.
    ("base.rs", "lua.charge(src.len() as u64 + LOAD_TOKEN_FUEL"),
];

/// Lines of `text` (Rust source) that charge fuel from the length of a buffer their
/// function builds: made with `Vec::new`, `with_capacity`, `String::new`, `vec!` or an
/// `OutBuf`, or grown with `push`, `extend`, `insert`, `append` or `write!`.
fn built_buffer_charges(text: &str) -> Vec<String> {
    let mut bad = Vec::new();
    // Split into functions: each starts at a line whose code begins a `fn`.
    let mut fns: Vec<Vec<(usize, &str)>> = vec![Vec::new()];
    for (i, line) in text.lines().enumerate() {
        let t = line.trim_start();
        let t = t
            .strip_prefix("pub(crate) ")
            .or_else(|| t.strip_prefix("pub(super) "))
            .or_else(|| t.strip_prefix("pub "))
            .unwrap_or(t);
        if t.starts_with("fn ") {
            fns.push(Vec::new());
        }
        if let Some(f) = fns.last_mut() {
            f.push((i + 1, line.split("//").next().unwrap_or("")));
        }
    }
    let is_id = |c: char| c.is_alphanumeric() || c == '_';
    let ident = |s: &str| {
        s.chars().rev().take_while(|c| is_id(*c)).collect::<String>().chars().rev().collect::<String>()
    };
    // `let mut name: T = ...` -> `name`.
    let let_name = |code: &str| -> String {
        let Some(at) = code.find("let ") else { return String::new() };
        let rest = code[at + 4..].trim_start();
        let rest = rest.strip_prefix("mut ").unwrap_or(rest);
        rest.chars().take_while(|c| is_id(*c)).collect()
    };
    // `name.len()` as a whole identifier in `arg`.
    let uses_len = |arg: &str, name: &str| {
        let pat = format!("{name}.len()");
        arg.match_indices(&pat)
            .any(|(i, _)| !arg[..i].chars().next_back().is_some_and(|c| is_id(c) || c == '.'))
    };
    for f in &fns {
        let mut buffers: Vec<String> = Vec::new();
        for (_, code) in f {
            for maker in [
                "Vec::new()",
                "Vec::with_capacity(",
                "String::new()",
                "String::with_capacity(",
                "vec![",
                "OutBuf::new(",
            ] {
                if code.contains(maker) {
                    buffers.push(let_name(code));
                }
            }
            for grow in [".push(", ".push_str(", ".extend(", ".extend_from_slice(", ".insert(", ".append("] {
                let mut rest = *code;
                while let Some(at) = rest.find(grow) {
                    buffers.push(ident(&rest[..at]));
                    rest = &rest[at + grow.len()..];
                }
            }
            if let Some(at) = code.find("write!(") {
                let arg = code[at + 7..]
                    .split(',')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .trim_start_matches("&mut ")
                    .trim_start_matches('*');
                buffers.push(arg.to_owned());
            }
        }
        buffers.retain(|b| !b.is_empty());
        for (line, code) in f {
            for call in ["charge(", "charge_bytes("] {
                let Some(at) = code.find(call) else { continue };
                let arg = &code[at..];
                if buffers.iter().any(|b| uses_len(arg, b)) {
                    bad.push(format!("{line}: {}", code.trim()));
                }
            }
        }
    }
    bad
}
