//! Sweep 4, area "security": metering gaps found by an inventory of every
//! place the VM and stdlib do work proportional to input, and the cycle-3 additions
//! (5.3 compatibility, `set_warnings`, stack shrinking, slab trimming and generations,
//! panic poisoning). Each test is a finding of this sweep or an attack that
//! is stopped correctly. Every program runs with the fast loop on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::time::{Duration, Instant};

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua};

/// Runs `setup`, then `body` in a loop until `fuel` is spent: nanoseconds per unit of fuel.
fn ns_per_unit(setup: &str, body: &str, fuel: u64, fast: bool) -> f64 {
    let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40), Libs::ALL);
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

/// A typical library call's cost per unit on this machine and build (about the median
/// of sweep 3's calibration).
fn baseline(fast: bool) -> f64 {
    ns_per_unit("t = {} s = 'abc'", "local x = tostring(#s) .. s t[1] = x", 300_000, fast)
}

/// Each case must cost at most `BOUND` times plain code per unit of fuel (sweep 3's
/// calibrated maximum is about 3.5x; the property test's bound is 10x).
const BOUND: f64 = 12.0;

fn assert_metered(cases: &[(&str, &str)]) {
    assert_metered_with(cases, 200_000);
}

/// [`assert_metered`] with `fuel` for each case (for bodies that cost more than 200k).
fn assert_metered_with(cases: &[(&str, &str)], fuel: u64) {
    for fast in [true, false] {
        let base = baseline(fast);
        let mut bad = Vec::new();
        for (setup, body) in cases {
            let mut ns = ns_per_unit(setup, body, fuel, fast);
            if ns > BOUND * base {
                // Idle priority is noisy: the best of three.
                ns = ns.min(ns_per_unit(setup, body, fuel, fast)).min(ns_per_unit(setup, body, fuel, fast));
            }
            if ns > BOUND * base {
                bad.push(format!("{body}: {ns:.0} ns/unit = {:.0}x plain code", ns / base));
            }
        }
        assert!(bad.is_empty(), "fast {fast}, plain {base:.1} ns/unit:\n{}", bad.join("\n"));
    }
}

/// Another thread interrupts `work` 50 ms in: how late the call stops after that.
fn interrupt_late(setup: &str, work: &str, memory: usize) -> Duration {
    let mut lua = Lua::new(Limits::unmetered().memory(memory), Libs::ALL);
    lua.exec(setup, "=setup").unwrap();
    let f = lua.load(work, "=work").unwrap();
    let h = lua.interrupt_handle();
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        h.interrupt();
        Instant::now()
    });
    let r = lua.call(f.value(), &[]);
    let end = Instant::now();
    let at = t.join().unwrap();
    assert!(matches!(r, Err(Error::Limit(LimitKind::Interrupted))), "{work}: {r:?}");
    end.saturating_duration_since(at)
}

/// Probe (not a test): interrupt latency of collections over structures the atomic phase
/// walks in one step. `cargo test -p remedy-lua --test all sweep4_sec::probe_atomic -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_atomic() {
    let n = std::env::var("SWEEP4_N").ok().and_then(|s| s.parse().ok()).unwrap_or(1_000_000usize);
    let cases = [
        format!(
            "w = setmetatable({{}}, {{__mode = 'k'}}) keep = {{}} for i = 1, {n} do local k = {{}} keep[i] = k w[k] = {{}} end"
        ),
        format!(
            "w = setmetatable({{}}, {{__mode = 'v'}}) keep = {{}} for i = 1, {n} do local t = {{}} keep[i] = t w[i] = t end"
        ),
        format!("ws = {{}} for i = 1, {n} // 4 do ws[i] = setmetatable({{{{}}}}, {{__mode = 'k'}}) end"),
        format!(
            "cs = {{}} for i = 1, {n} // 5000 do local co = coroutine.create(function(...) coroutine.yield() end) coroutine.resume(co, table.unpack({{}}, 1, 7000)) cs[i] = co end"
        ),
        format!(
            "cs = {{}} for i = 1, {n} // 4 do local co = coroutine.wrap(function() coroutine.yield() end) co() cs[i] = co end"
        ),
    ];
    for setup in &cases {
        let late = interrupt_late(setup, "while true do collectgarbage() end", 4 << 30);
        println!("{late:?} late: {}", &setup[..setup.len().min(90)]);
    }
}

/// SEC4-1..7: work done on an error path, or on input never charged, was free. Each of
/// these ran at 1,000x to 50,000x plain code per unit of fuel (a 4 MB string: 15 to 60 s
/// for 1M fuel against 30 ms).
#[test]
fn sec4_error_paths_and_option_strings_are_charged() {
    let s = "s = string.rep('x', 1 << 20)";
    assert_metered(&[
        // SEC4-1: an option string was copied whole for the message.
        (s, "pcall(collectgarbage, s)"),
        // SEC4-2: `load`'s mode was scanned and copied whole.
        (s, "load('x = 1', nil, s)"),
        (&format!("{s} m = s .. 't'"), "load('x = 1', nil, m)"),
        // SEC4-3: `string.unpack` copied the whole data string per call, and 'c'/'s'/'z'
        // fields were copied (and 'z' scanned) free.
        (s, "string.unpack('b', s)"),
        (s, "string.unpack('c' .. #s, s)"),
        (s, "pcall(string.unpack, 'z', s)"),
        // SEC4-4: `string.pack` built its output, then raised on a later argument free.
        (s, "pcall(string.pack, 'zz', s, {})"),
        (s, "pcall(string.pack, 's4c1', s, {})"),
        (&format!("{s} z = s .. string.char(0)"), "pcall(string.pack, 'z', z)"),
        // SEC4-5: `table.concat` built its output, then raised on a bad value free.
        (s, "pcall(table.concat, {s, {}})"),
        // SEC4-6: `gsub` with a function or table that raises after the output has grown.
        (s, "pcall(string.gsub, 'ab', '.', function(c) if c == 'b' then error() end return s end)"),
        (
            s,
            "pcall(string.gsub, 'ab', '.', setmetatable({}, {__index = function(_, c) if c == 'b' then error() end return s end}))",
        ),
        // SEC4-7: `string.format` built its output (or copied a long format), then raised.
        (s, "pcall(string.format, '%s%d', s, {})"),
        (s, "pcall(string.format, s .. '%d', {})"),
        (s, "pcall(string.format, '%y' .. s)"),
        (s, "pcall(string.format, '%q%d', s, {})"),
    ]);
}

/// SEC4-8: the collector's atomic phase ran as one step that never looked at the
/// interrupt: 78 ms with a million ephemerons, and it grows with the heap (unbounded under
/// a large memory limit with unmetered fuel). Now the interrupt abandons the cycle; the
/// state is sound afterwards (objects intact, weak semantics kept, memory freed).
#[test]
fn sec4_8_atomic_phase_stops_on_the_interrupt() {
    let n = 600_000;
    // An ephemeron chain spread over many weak tables: each value is the next entry's key,
    // so all of it is marked in the atomic phase. (One table's own traversal is a single
    // unit of work, bounded by its size; see the report.) `count` counts the entries.
    let setup = format!(
        "ws = {{}} for j = 1, {n} // 1000 do ws[j] = setmetatable({{}}, {{__mode = 'k'}}) end \
         root = {{}} local prev = root for i = 1, {n} do local k = {{}} ws[i % #ws + 1][prev] = k prev = k end \
         function count() local c = 0 for _, w in ipairs(ws) do for _ in pairs(w) do c = c + 1 end end return c end"
    );
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::unmetered().memory(2 << 30), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(&setup, "=setup").unwrap();
        let f = lua.load("while true do collectgarbage() end", "=work").unwrap();
        let mut worst = Duration::ZERO;
        for _ in 0..3 {
            let h = lua.interrupt_handle();
            let t = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(60));
                h.interrupt();
                Instant::now()
            });
            let r = lua.call(f.value(), &[]);
            let end = Instant::now();
            let at = t.join().unwrap();
            assert!(matches!(r, Err(Error::Limit(LimitKind::Interrupted))), "{r:?}");
            worst = worst.max(end.saturating_duration_since(at));
            lua.interrupt_handle().reset();
            // Sound after an abandoned cycle: every object is there, the ephemerons hold.
            let out = lua
                .exec(&format!("assert(count() == {n}) return collectgarbage('count')"), "=check")
                .unwrap();
            assert!(!out.is_empty());
        }
        assert!(worst <= Duration::from_millis(30), "fast {fast}: stopped {worst:?} after the interrupt");
        // Dropping the chain's root still frees every entry.
        let out = lua.exec("root = nil collectgarbage() collectgarbage() return count()", "=drop").unwrap();
        assert_eq!(common_int(&lua, out[0]), 0);
    }
}

fn common_int(_lua: &Lua, v: remedy_lua::Value) -> i64 {
    match v {
        remedy_lua::Value::Int(i) => i,
        other => panic!("not an integer: {other:?}"),
    }
}

/// SEC4-9 (process crash): marking an ephemeron's key marked the values waiting for it
/// at once, recursively, one native frame per link. A chain of 50,000 links (each value
/// the next entry's key) under the default limits overflowed the native stack and killed
/// the process during any collection. Now the values are released when the key is
/// traversed, off the gray list. Run on a 64 KiB thread (`common::small_stack`).
#[test]
fn sec4_9_ephemeron_chain_does_not_overflow_the_native_stack() {
    let src = "local w = setmetatable({}, {__mode = 'k'}) local ks = {} \
               for i = 1, 200000 do ks[i] = {} end for i = 200000, 2, -1 do w[ks[i - 1]] = ks[i] end \
               root = ks[1] ks = nil collectgarbage() \
               local c = 0 for _ in pairs(w) do c = c + 1 end \
               root = nil collectgarbage() \
               local d = 0 for _ in pairs(w) do d = d + 1 end return c * 1000000 + d";
    for fast in [true, false] {
        let r = super::common::small_stack(move || {
            // This checks the native stack, not a budget. Its collections were paid 5.2M
            // units at 3x plain code per unit; sweep 6 charges ephemeron entries what they
            // cost (S6GC-2d): 10.0M units at 1.3-1.6x, just past the default fuel.
            let mut lua = Lua::new(Limits::default().fuel(100_000_000), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(src, "=chain").map(|v| common_int(&lua, v[0]))
        });
        assert_eq!(r.unwrap(), 199_999 * 1_000_000, "fast {fast}");
    }
}

/// SEC4-12: `next` skipped up to 1024 empty slots a call for free, so `pairs` over an
/// array with one entry every 1024 slots ran at 30x plain code per unit of fuel. The
/// array part now costs a unit per 16 positions crossed (dense traversals cost nothing
/// more), and a hash-part run past 64 empty slots a unit per 16.
#[test]
fn sec4_12_sparse_traversals_are_charged() {
    let sparse_arr = "t = {} for i = 1, 1 << 18 do t[i] = i end for i = 1, 1 << 18 do if i % 1000 ~= 0 then t[i] = nil end end";
    let sparse_hash = "t = {} for i = 1, 1 << 17 do t['k' .. i] = i end for i = 1, 1 << 17 do if i % 1000 ~= 0 then t['k' .. i] = nil end end";
    assert_metered(&[
        (sparse_arr, "for k in pairs(t) do end"),
        (sparse_arr, "local k = next(t) while k do k = next(t, k) end"),
        (sparse_hash, "for k in pairs(t) do end"),
        (sparse_hash, "local k = next(t) while k do k = next(t, k) end"),
    ]);
}

/// SEC4-13 (careless host): `Lua::load` compiled on a state a panic had poisoned (every
/// other entry refuses it).
#[test]
fn sec4_13_poisoned_state_refuses_load() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    lua.register("boom", |_, _| panic!("host bug")).unwrap();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lua.exec("boom()", "=t")));
    assert!(r.is_err());
    assert!(matches!(lua.load("return 1", "=t"), Err(Error::Host(_))));
}

/// SEC4-14 (host memory): `to_data` copied each table's entries out whole before its node
/// budget applied; now a 1M-entry table stops at the budget without copying it.
#[test]
fn sec4_14_to_data_stops_at_its_budget() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let v = lua.exec("local t = {} for i = 1, 1000000 do t[i] = i end return t", "=t").unwrap();
    let start = Instant::now();
    let r = lua.to_data(v[0], remedy_lua::DataLimits { depth: 4, nodes: 1000, bytes: 1 << 20 });
    assert!(matches!(r, Err(Error::Host(_))), "{r:?}");
    assert!(start.elapsed() < Duration::from_millis(50), "{:?}", start.elapsed());
}

/// SEC4-15: compiling was quadratic in the number of locals, labels, pending gotos and
/// upvalues (no limit on them in the superset), against `load`'s charge per byte and
/// token: 40,000 locals took 5 s for about 2M units of fuel (100x plain code). The
/// compiler's scans are now counted and paid for as fuel, and a compile stops at the
/// fuel limit.
#[test]
fn sec4_15_quadratic_compiles_are_charged() {
    let source = |what: &str| -> String {
        match what {
            "locals" => "local b = {} for i = 1, 12000 do b[#b + 1] = 'local a' .. i .. ' = 1' end \
                         for i = 1, 12000 do b[#b + 1] = 'a1 = a1 + 1' end src = table.concat(b, ' ')"
                .into(),
            "labels" => "local b = {} for i = 1, 12000 do b[#b + 1] = '::l' .. i .. '::' end \
                         for i = 1, 12000 do b[#b + 1] = 'do goto l' .. i .. ' end' end src = table.concat(b, ' ')"
                .into(),
            "gotos" => "local b = {} for i = 1, 12000 do b[#b + 1] = 'goto l' .. i end \
                        for i = 1, 12000 do b[#b + 1] = '::l' .. i .. '::' end src = table.concat(b, ' ')"
                .into(),
            "upvals" => "local b = {} for i = 1, 12000 do b[#b + 1] = 'local u' .. i .. ' = 1' end \
                         b[#b + 1] = 'return function() return u1' for i = 2, 12000 do b[#b + 1] = '+ u' .. i end \
                         b[#b + 1] = 'end' src = table.concat(b, ' ')"
                .into(),
            "consts" => "local b = {} for i = 1, 12000 do b[#b + 1] = 'local c' .. i .. ' <const> = 1' end \
                         for i = 1, 12000 do b[#b + 1] = 'local x = 1' end src = table.concat(b, ' ')"
                .into(),
            _ => "local b = {} for i = 1, 6000 do b[#b + 1] = 'local v' .. i end \
                  local t = {} for i = 1, 6000 do t[#t + 1] = 'v' .. i t[#t + 1] = 'q[' .. i .. ']' end \
                  b[#b + 1] = 'q = {} ' .. table.concat(t, ', ') .. ' = 1' src = table.concat(b, ' ')"
                .into(),
        }
    };
    let setups: Vec<String> =
        ["locals", "labels", "gotos", "upvals", "consts", "assign"].iter().map(|w| source(w)).collect();
    let cases: Vec<(&str, &str)> = setups.iter().map(|s| (s.as_str(), "assert(load(src))")).collect();
    // Fuel enough for `load`'s up-front charge (about 1M here) and some compiling.
    assert_metered_with(&cases, 6_000_000);
}

/// SEC4-16: library functions that read every argument charged nothing per argument, so a
/// vararg function passing the same 200k arguments again and again ran free:
/// `pcall(utf8.char, ...)` and `pcall(string.char, ...)` at 20-40x, `warn(...)` at 60x.
/// They now owe a share per argument, paid on every exit.
#[test]
fn sec4_16_per_argument_work_is_charged() {
    let setup = "t = {} for i = 1, 100000 do t[i] = 65 end t[#t] = -1";
    let cases =
        ["local _ = pcall(string.char, table.unpack(t))", "local _ = pcall(utf8.char, table.unpack(t))"];
    let mut all: Vec<(String, &str)> = cases.iter().map(|c| (setup.to_owned(), *c)).collect();
    // The same arguments reused: no `unpack` per call to pay for them.
    for f in ["string.char", "utf8.char", "warn"] {
        all.push((
            format!("{setup} function run(...) while true do pcall({f}, ...) end end"),
            "run(table.unpack(t))",
        ));
    }
    let refs: Vec<(&str, &str)> = all.iter().map(|(s, b)| (s.as_str(), *b)).collect();
    assert_metered_with(&refs, 2_000_000);
}

/// SEC4-17 (found by the recursion audit): naming the variable in an error message
/// followed `Move`s and key registers back through the code by recursion, one native
/// frame per step, as deep as the program's nested indexing (bounded by the register
/// window in practice, where naming applies). Now a loop; the names are unchanged.
#[test]
fn sec4_17_error_naming_is_not_recursive() {
    for n in [3, 150] {
        let src = format!(
            "local t = setmetatable({{}}, {{__index = function() return 1 end}}) local ok, e = pcall(function() local x = {}1{}() return x end) return e",
            "t[".repeat(n),
            "]".repeat(n)
        );
        let c = super::common::oracle(&super::common::wrap(&src), 100_000_000);
        for fast in [true, false] {
            let s = src.clone();
            let r = super::common::small_stack(move || {
                super::common::remedy_measured(&super::common::wrap(&s), 100_000_000, fast).0
            });
            assert_eq!(r, c, "n {n} fast {fast}");
        }
    }
}

/// Guard for SEC4-1..7's class: a library function's output is paid for through an
/// `OutBuf` (or `owe`), which `call_native` charges on every exit, never by a `charge`
/// computed from the output's length at the end (skipped by any error before it). This
/// fails if a library file charges from a built buffer's length again.
#[test]
fn stdlib_never_charges_from_a_built_buffer() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/stdlib");
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if code.contains("charge(")
                && ["out.len()", "buf.len()", "line.len()", "msg.len()", "res.len()"]
                    .iter()
                    .any(|b| code.contains(b))
            {
                bad.push(format!("{}:{}: {}", path.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(bad.is_empty(), "charge from a built buffer (use OutBuf or owe):\n{}", bad.join("\n"));
}

/// `strict_5_4` is the state's for life: `set_limits` cannot switch `__le` emulation
/// without the math table (or the other way round).
#[test]
fn strict_5_4_is_fixed_when_the_state_is_made() {
    let probe = "local m = {__lt = function() return true end} local a = setmetatable({}, m) \
                 return math.pow ~= nil, pcall(function() return a <= a end)";
    for strict in [false, true] {
        let mut lua = Lua::new(Limits::default().strict_5_4(strict), Libs::ALL);
        let before: Vec<String> = lua.exec(probe, "=p").unwrap().iter().map(|v| format!("{v:?}")).collect();
        lua.set_limits(lua.limits().strict_5_4(!strict).hash_seed(7));
        assert_eq!(lua.limits().strict_5_4, strict);
        let after: Vec<String> = lua.exec(probe, "=p").unwrap().iter().map(|v| format!("{v:?}")).collect();
        assert_eq!(before, after, "strict {strict}");
        assert_eq!(before[0], format!("{:?}", remedy_lua::Value::Bool(!strict)));
    }
}
