//! Sweep 3, area "security and the host API": every budget, the host API
//! as a careless or hostile host uses it, weak tables under GC stress, the pool allocator,
//! hash flooding and stdlib costs. Each test is a finding of this sweep or
//! an attack that is stopped correctly.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

/// Runs `f` on a worker thread; `Err` when it panicked or did not finish in `secs`.
fn bounded<T: Send + 'static>(secs: u64, f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    // (Never detached: `common::with_deadline`.)
    let r = super::common::with_deadline(16 << 20, Duration::from_secs(secs), move || {
        std::panic::catch_unwind(AssertUnwindSafe(f))
    });
    match r {
        Some(Ok(v)) => Ok(v),
        Some(Err(p)) => Err(format!(
            "PANIC: {}",
            p.downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default()
        )),
        None => Err(format!("HANG: still running after {secs} s")),
    }
}

fn outcome(r: Result<Vec<Value>, Error>) -> String {
    match r {
        Ok(_) => "ok".into(),
        Err(Error::Limit(k)) => format!("limit {k:?}"),
        Err(e) => format!("error {}", e.message()),
    }
}

/// Runs `setup` then `work` with unlimited fuel; another thread interrupts 100 ms into
/// `work`. The call must stop with `Interrupted` within `max_ms` of the interrupt in one
/// of three tries (a wall-clock measure: under load, Idle priority and other builds can
/// deschedule one try for longer than that, so a single slow try is not a failure), and
/// every try within a generous backstop of 5 s.
fn interrupted_within(setup: &str, work: &str, fast: bool, max_ms: u64) {
    let mut lates = Vec::new();
    for _ in 0..3 {
        let (setup, work) = (setup.to_owned(), work.to_owned());
        let label = work.clone();
        let r = bounded(60, move || {
            let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(&setup, "=setup").unwrap();
            let f = lua.load(&work, "=work").unwrap();
            let h = lua.interrupt_handle();
            let t = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(100));
                h.interrupt();
                Instant::now()
            });
            let r = outcome(lua.call(f.value(), &[]));
            let end = Instant::now();
            let at = t.join().unwrap();
            (r, end.saturating_duration_since(at))
        })
        .unwrap_or_else(|e| panic!("{e}: {label}"));
        assert_eq!(r.0, "limit Interrupted", "{label}");
        assert!(r.1 <= Duration::from_secs(5), "{label}: stopped {:?} after the interrupt", r.1);
        if r.1 <= Duration::from_millis(max_ms) {
            return;
        }
        lates.push(r.1);
    }
    panic!("{work}: stopped {lates:?} after the interrupt (3 tries, bound {max_ms} ms)");
}

/// Same with a time limit, measured by a work clock instead of the wall clock: the host's
/// clock advances 1 ms each time the state reads it, so the deadline (`max_ms` such
/// reads) passes after a fixed number of looks at it, whatever the machine's load. The
/// call must stop with `Time`, which proves the work looks at the deadline as it goes
/// (a native that never looked would run to its end or the 60 s backstop); the number
/// of reads must be exactly what the deadline needs, not hundreds more.
fn timed_within(setup: &str, work: &str, fast: bool, max_ms: u64) {
    let (setup, work) = (setup.to_owned(), work.to_owned());
    let label = work.clone();
    let r = bounded(60, move || {
        let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(&setup, "=setup").unwrap();
        let f = lua.load(&work, "=work").unwrap();
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let r2 = std::sync::Arc::clone(&reads);
        let clock = move || Duration::from_millis(r2.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1);
        lua.set_time_limit(Some((std::sync::Arc::new(clock), Duration::from_millis(max_ms))));
        let start = Instant::now();
        let r = outcome(lua.call(f.value(), &[]));
        (r, reads.load(std::sync::atomic::Ordering::Relaxed), start.elapsed())
    })
    .unwrap_or_else(|e| panic!("{e}: {label}"));
    assert_eq!(r.0, "limit Time", "{label}");
    // One read sets the deadline, `max_ms` more reach it.
    assert!(r.1 <= max_ms + 2, "{label}: {} clock reads for a deadline of {max_ms}", r.1);
    assert!(r.2 <= Duration::from_secs(30), "{label}: stopped after {:?}", r.2);
}

const SLOW_NATIVES: &[(&str, &str)] = &[
    ("s = string.rep('a', 4000000) p = string.rep('a', 40000) .. 'b'", "return s:find(p, 1, true)"),
    ("s = string.rep('a', 8000000)", "return s:find('.-.-.-.-b')"),
    ("s = string.rep('a', 8000000)", "return s:match('^(a-)(a-)(a-)b')"),
    ("s = string.rep('a', 8000000)", "return s:gsub('.-.-.-b', 'x')"),
    ("s = string.rep('a', 8000000)", "for x in s:gmatch('.-.-.-b') do end"),
    ("t = {} for i = 1, 3000000 do t[i] = {} end", "while true do collectgarbage() end"),
    ("t = {} for i = 1, 1000000 do t[i] = -i end", "table.sort(t, function(a, b) return a < b end)"),
    ("", "local co = coroutine.wrap(function() while true do end end) co()"),
    ("", "while true do end"),
    // SEC3-17: the lexer, parser and compiler look at the interrupt and deadline.
    ("s = 'return ' .. string.rep('(', 2^20) .. '1' .. string.rep(')', 2^20)", "while true do load(s) end"),
    (
        "s = string.rep('function f() local a = {x = 1, [2] = 3} ', 2^16) .. string.rep('end ', 2^16)",
        "while true do load(s) end",
    ),
    ("s = string.rep('x = 1 ', 2^20)", "while true do load(s) end"),
];

#[test]
fn sec3_1_2_interrupt_and_time_limit_stop_long_natives() {
    let _heavy = super::common::memory_heavy();
    let mut bad = Vec::new();
    for fast in [true, false] {
        for (setup, work) in SLOW_NATIVES {
            for f in [interrupted_within, timed_within] {
                if let Err(e) = std::panic::catch_unwind(|| f(setup, work, fast, 100)) {
                    bad.push(e.downcast_ref::<String>().cloned().unwrap_or_default());
                }
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// (setup, loop body): the body runs in `while true do ... end` until 1M fuel is spent.
/// Setup runs first with a large budget.
const COSTLY: &[(&str, &str)] = &[
    ("t = {} for i = 1, 2^20 do t[i] = i end for i = 1, 2^20 do t[i] = nil end t[2^21] = 1", "next(t)"),
    ("t = {} for i = 1, 2^20 do t['k'..i] = i end for i = 1, 2^20 do t['k'..i] = nil end t.z = 1", "next(t)"),
    ("t = {} for i = 1, 2^20 do t[i] = i end for i = 1, 2^20 - 1 do t[i] = nil end", "local _ = #t"),
    ("t = {} for i = 1, 2^20 do t[i * 2] = i end", "local _ = #t"),
    ("t = {} for i = 1, 2^20 do t[i] = i end", "table.insert(t, 1, 0) table.remove(t, 1)"),
    ("t = {} for i = 1, 2^20 do t[i] = i end", "table.move(t, 1, 2^20, 2) "),
    ("t = {} for i = 1, 2^20 do t[i] = 'x' end", "local _ = table.concat(t)"),
    ("s = string.rep('a', 2^23)", "local _ = s:upper()"),
    ("s = string.rep('a', 2^23)", "local _ = #s:rep(1, s)"),
    ("s = string.rep('a', 2^23) s2 = s .. ''", "local _ = s < s2"),
    ("s = string.rep('a', 2^23) s2 = s:sub(1, -2) .. 'b'", "local _ = s == s2"),
    ("s = string.rep('\\u{7FF}', 2^21)", "local _ = utf8.len(s)"),
    ("s = string.rep('\\u{7FF}', 2^21)", "local _ = utf8.offset(s, -1)"),
    ("s = string.rep('\\u{7FF}', 2^21)", "local _ = utf8.offset(s, 2^21)"),
    ("s = string.rep('a', 2^23)", "local _ = tonumber(s)"),
    ("s = string.rep('1', 2^23)", "local _ = tonumber(s, 36)"),
    ("s = string.rep(' ', 2^23) .. '1'", "local _ = tonumber(s)"),
    ("s = string.rep('a', 2^23)", "local _ = s:format()"),
    ("s = string.rep('%%', 2^22)", "local _ = s:format()"),
    ("s = string.rep('%a', 2^20)", "local _ = os.date(s, 0)"),
    ("s = string.rep('a', 2^23)", "local _ = s:byte(-1)"),
    ("s = string.rep('a', 2^23)", "local _ = s:sub(-2)"),
    ("s = string.rep('a', 2^22)", "local _ = load('return ' .. s)"),
    ("s = 'return ' .. string.rep('(', 2^16) .. string.rep(')', 2^16)", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('1+', 2^19) .. '1'", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('(', 2^12) .. string.rep(')', 2^12)", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('(', 2^14) .. string.rep(')', 2^14)", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('(', 2^18) .. string.rep(')', 2^18)", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('{', 2^16) .. string.rep('}', 2^16)", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('not ', 2^16) .. '1'", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('- ', 2^16) .. '1'", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('a..', 2^16) .. 'a'", "local _ = load(s)"),
    ("s = 'return ' .. string.rep('2^', 2^16) .. '2'", "local _ = load(s)"),
    ("s = 'return a' .. string.rep('.b', 2^16)", "local _ = load(s)"),
    ("s = 'return a' .. string.rep('[1]', 2^16)", "local _ = load(s)"),
    ("s = 'return a' .. string.rep('()', 2^16)", "local _ = load(s)"),
    ("s = string.rep('do ', 2^15) .. string.rep('end ', 2^15)", "local _ = load(s)"),
    ("s = string.rep('if x then ', 2^14) .. string.rep('end ', 2^14)", "local _ = load(s)"),
    ("s = string.rep('function f() ', 2^13) .. string.rep('end ', 2^13)", "local _ = load(s)"),
    ("s = string.rep('function f() ', 2^9) .. string.rep('end ', 2^9)", "local _ = load(s)"),
    ("s = string.rep('function f() ', 2^11) .. string.rep('end ', 2^11)", "local _ = load(s)"),
    ("s = string.rep('function f() ', 2^7) .. string.rep('end ', 2^7)", "local _ = load(s)"),
    (
        "s = string.rep('local x = function() ', 2^11) .. 'return x' .. string.rep(' end', 2^11)",
        "local _ = load(s)",
    ),
    ("s = string.rep('while x do ', 2^14) .. string.rep('end ', 2^14)", "local _ = load(s)"),
    ("s = 'local a ' .. string.rep('local a ', 2^16)", "local _ = load(s)"),
    ("s = 'local x = 1 ' .. string.rep('x = x + 1 ', 2^15) .. ' goto l ::l::'", "local _ = load(s)"),
    (
        "s = string.rep('::a' .. ':: goto a ', 1)  for i = 1, 2^12 do s = s .. ' ::l' .. i .. ':: goto l' .. i end",
        "local _ = load(s)",
    ),
    ("s = 'return {' .. string.rep('1,', 2^18) .. '}'", "local _ = load(s)"),
    ("s = 'return {' .. string.rep('a=1,', 2^16) .. '}'", "local _ = load(s)"),
    ("s = 'local t = {...} return ' .. string.rep('t[1],', 2^14) .. '1'", "local _ = load(s)"),
    ("s = string.rep('--[==[ ', 2^19)", "local _ = load(s)"),
    ("s = 'x = ' .. string.rep('a', 2^22)", "local _ = load(s)"),
    ("s = 'local s = \"' .. string.rep('\\\\65', 2^20) .. '\"'", "local _ = load(s)"),
    ("s = string.rep('a', 2^23)", "local _ = string.pack('s', s)"),
    ("s = string.rep('a', 2^23)", "local _ = string.unpack('z', s .. '\\0')"),
    ("", "local _ = string.packsize(string.rep('i16', 2^18))"),
    ("", "pcall(string.pack, string.rep('x', 2^20) .. 'c0')"),
    ("s = string.rep('a', 2^23)", "local _ = s:find('b', 1, true)"),
    ("s = string.rep('a', 2^23)", "local _ = s:find('[b]')"),
    ("s = string.rep('a', 2^23)", "local _ = s:find('a-b')"),
    ("s = string.rep('a', 2^23)", "local _ = s:find(string.rep('a', 2^16) .. 'b', 1, true)"),
    ("s = string.rep('a', 2^20) p = string.rep('[a]', 2^16)", "local _ = s:find(p)"),
    ("s = string.rep('a', 2^20) p = '[' .. string.rep('b', 2^20) .. ']'", "local _ = s:find(p)"),
    ("s = string.rep('a', 2^20) p = string.rep('%f[a]', 2^16)", "local _ = s:find(p)"),
    ("s = string.rep('ab', 2^19) p = string.rep('%bab', 2^10)", "local _ = s:find(p)"),
    ("s = string.rep('a', 2^20)", "local _ = s:gsub('', '')"),
    ("s = string.rep('a', 2^20)", "local _ = s:gsub('a', '%0%0%0%0%0%0%0%0%0%0%0%0%0%0%0%0')"),
    ("s = string.rep('a', 2^16) r = string.rep('%0', 2^12)", "local _ = s:gsub('a', r)"),
    ("s = string.rep('a', 2^20) c = string.rep('(a)', 32)", "local _ = s:gsub(c, '%9')"),
    ("s = string.rep('a', 2^20)", "for _ in s:gmatch('') do end"),
    ("s = string.rep('a', 2^20)", "local _ = s:match(string.rep('(a*)', 32))"),
    ("s = string.rep('a', 2^20)", "local _ = s:match('(' .. string.rep('a', 2^16) .. ')%1')"),
    ("t = setmetatable({}, {__index = function(t, k) return k end})", "local _ = t[1]"),
    ("a = {} for i = 1, 200 do a = setmetatable({}, {__index = a}) end", "local _ = a.x"),
    ("t = {} for i = 1, 2^16 do t[i] = i end", "local _ = select('#', table.unpack(t))"),
    ("t = {} for i = 1, 2^16 do t[i] = i end", "local _ = math.max(table.unpack(t))"),
    ("t = {} for i = 1, 2^16 do t[i] = i end", "local _ = string.char(table.unpack(t, 1, 255))"),
    ("t = {} for i = 1, 2^16 do t[i] = 9 end", "local _ = utf8.char(table.unpack(t))"),
    ("t = {} for i = 1, 2^16 do t[i] = i end", "local _ = {table.unpack(t)}"),
    ("t = {} for i = 1, 2^16 do t[i] = i end", "local _ = table.pack(table.unpack(t))"),
    (
        "t = {} for i = 1, 2^16 do t[i] = i end",
        "local co = coroutine.wrap(function(...) return ... end) co(table.unpack(t))",
    ),
    (
        "t = {} for i = 1, 2^16 do t[i] = i end",
        "local co = coroutine.wrap(function() while true do coroutine.yield(table.unpack(t)) end end) co()",
    ),
    (
        "t = {} for i = 1, 2^16 do t[i] = i end f = function(...) return select(-1, ...) end",
        "f(table.unpack(t))",
    ),
    ("t = {} for i = 1, 2^16 do t[i] = i end", "local _ = tostring(t)"),
    ("t = {} for i = 1, 2^16 do t[i] = i end", "pcall(error, t)"),
    ("s = string.rep('x', 2^22)", "pcall(error, s)"),
    ("s = string.rep('x', 2^22)", "pcall(error, s, 2)"),
    ("s = string.rep('x', 2^22)", "pcall(string.rep, s, -1, s)"),
    ("s = string.rep('x', 2^22)", "pcall(setmetatable, 1, s)"),
    ("s = string.rep('x', 2^22)", "pcall(s.bad)"),
    ("s = string.rep('x', 2^22) t = setmetatable({}, {__name = s})", "pcall(function() return t + 1 end)"),
    ("s = string.rep('x', 2^22) t = setmetatable({}, {__name = s})", "local _ = tostring(t)"),
    ("s = string.rep('x', 2^22)", "pcall(function() return {} + s end)"),
    ("s = string.rep('x', 2^22)", "pcall(function() local x = nil; x[s] = 1 end)"),
    ("s = string.rep('x', 2^22)", "pcall(function() return s() end)"),
    ("s = string.rep('x', 2^22)", "pcall(load, s)"),
    ("s = string.rep('x', 2^22)", "pcall(load, 'return 1', s)"),
    ("s = string.rep('x', 2^22)", "pcall(assert, false, s)"),
    ("s = string.rep('x', 2^22)", "warn(s)"),
    ("s = 'x' t = setmetatable({}, {__name = s})", "pcall(function() return t + 1 end)"),
    ("s = 'x'", "pcall(function() return s() end)"),
    ("s = 'x'", "pcall(setmetatable, 1, s)"),
    ("s = 'x' f = function() return s() end", "pcall(f)"),
    ("f = function() error('x') end", "pcall(f)"),
    ("f = function() error({}) end", "pcall(f)"),
    ("f = function() return 1 end", "pcall(f)"),
    ("s = 'x'", "pcall(function() local x = nil; x[s] = 1 end)"),
    ("s = string.rep('1', 300)", "local _ = math.tointeger(s) or tonumber(s) or 0"),
    (
        "t = {} for i = 1, 2^16 do t[i] = {} end",
        "local w = setmetatable({}, {__mode = 'k'}) for i = 1, 2^16 do w[t[i]] = t[i] end w = nil collectgarbage('step', 0)",
    ),
    (
        "",
        "local t = setmetatable({}, {__mode = 'k'}) for i = 1, 2^12 do local k = {} t[k] = {k} end collectgarbage()",
    ),
    ("", "local co = coroutine.create(function() end) coroutine.resume(co) coroutine.close(co)"),
    ("t = {} for i = 1, 2^20 do t[i] = i end", "table.sort(t)"),
    ("t = {} for i = 1, 2^20 do t[i] = 2^20 - i end", "table.sort(t)"),
    ("t = {} for i = 1, 2^20 do t[i] = 1 end", "table.sort(t)"),
    ("", "local t = {} t[2^53] = 1 t[1.5] = 2 t[-0.0] = 3 for k in pairs(t) do end"),
];

#[test]
#[ignore = "probe: prints the worst ns per fuel unit"]
fn probe_costly() {
    let fuel = 1_000_000u64;
    let only = std::env::var("PROBE_ONLY").ok();
    let mut worst = Vec::new();
    for (setup, body) in COSTLY {
        if only.as_ref().is_some_and(|o| !body.contains(o.as_str()) && !setup.contains(o.as_str())) {
            continue;
        }
        let (setup, body) = (setup.to_string(), body.to_string());
        let label = format!("{setup} || {body}");
        let r = bounded(120, move || {
            let mut lua =
                Lua::new(Limits::default().memory(1 << 30).string_len(1 << 26).fuel(1 << 40), Libs::ALL);
            lua.exec(&setup, "=setup").unwrap();
            lua.set_limits(lua.limits().fuel(fuel));
            let f = lua.load(format!("while true do {body} end"), "=work").unwrap();
            let start = Instant::now();
            let r = outcome(lua.call(f.value(), &[]));
            (r, start.elapsed())
        });
        match r {
            Ok((r, t)) => {
                let ns = t.as_nanos() as f64 / fuel as f64;
                println!("{ns:9.1} ns/unit  {r:<20} {label}");
                worst.push((ns, label, r));
            }
            Err(e) => {
                println!("{e} {label}");
                worst.push((1e9, label, e));
            }
        }
    }
    worst.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!("--- worst");
    for w in worst.iter().take(15) {
        println!("{:9.1} {} {}", w.0, w.2, &w.1[..w.1.len().min(150)]);
    }
}

// ---------------------------------------------------------------- host API

/// A careless host keeps handles it did not root and stores them back while the collector
/// is between marking and sweeping: a handle to an object already found dead must not
/// pass as live, or the store leaves the table pointing at a freed (later reused) slot.
#[test]
fn sec3_12_dead_unswept_handles_refused() {
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec("keep = {} junk = {} for i = 1, 20000 do junk[i] = {} end junk = nil", "=setup").unwrap();
        let keep = match lua.get_global("keep").unwrap() {
            Value::Table(t) => t,
            _ => unreachable!(),
        };
        let mut victims = Vec::new();
        for _ in 0..400 {
            victims.push(lua.create_table().unwrap());
        }
        let step = lua.load("collectgarbage('step', 0)", "=step").unwrap();
        for (i, v) in victims.iter().enumerate() {
            lua.call(step.value(), &[]).unwrap();
            // Fails for a collected one; must also fail for one found dead and not yet swept.
            let _ = lua.table_set(keep, Value::Int(i as i64 + 1), Value::Table(*v));
        }
        lua.collect_garbage().unwrap();
        lua.collect_garbage().unwrap();
        // Every table `keep` holds must still be live, and its own.
        let held = lua.table_pairs(keep);
        let stale: Vec<_> = held.iter().filter(|(_, v)| lua.get(*v, Value::Int(1)).is_err()).collect();
        assert!(stale.is_empty(), "fast {fast}: {} of {} stored handles went stale", stale.len(), held.len());
        // And the script sees them as distinct live tables.
        let r = lua.exec("local n, seen = 0, {} for _, t in pairs(keep) do assert(not seen[t]) seen[t] = true t.x = 1 n = n + 1 end for i = 1, 2000 do local _ = {y = i} end for _, t in pairs(keep) do assert(t.x == 1 and t.y == nil) end return n", "=check");
        assert!(r.is_ok(), "fast {fast}: {r:?}");
    }
}

/// A handle from one state used in another must not reach the other state's objects.
#[test]
fn sec3_13_foreign_handles_refused() {
    let mut a = Lua::new(Limits::default(), Libs::ALL);
    let mut b = Lua::new(Limits::default(), Libs::ALL);
    let ta = a.create_table().unwrap();
    let fa = a.create_function("f", |_, _| Ok(vec![])).unwrap();
    let sa = a.create_string("a foreign string that is long enough not to be interned").unwrap();
    let mut accepted = Vec::new();
    for v in [Value::Table(ta), fa, sa, Value::Table(a.globals())] {
        if b.set_global("x", v).is_ok() {
            accepted.push(format!("{v:?}"));
        }
    }
    // A host function that hands out a foreign handle.
    let g = a.globals();
    b.register("leak", move |_, _| Ok(vec![Value::Table(g)])).unwrap();
    let r = b.exec("return leak()", "=t");
    if r.is_ok() {
        accepted.push("returned from a host function".into());
    }
    assert!(accepted.is_empty(), "foreign handles accepted: {accepted:?}");
}

/// A host function that panics: a host that catches the panic must not get a state that
/// misbehaves (wrong fuel, stuck collector, wrong thread); it is refused from then on.
#[test]
fn sec3_14_panicking_host_function_poisons() {
    for place in [
        "boom()",
        "pcall(boom)",
        "table.sort({3, 2, 1}, function(a, b) boom() return a < b end)",
        "coroutine.wrap(function() boom() end)()",
        "local co = coroutine.wrap(function() pcall(boom) end) co()",
        "string.gsub('abc', '%w', function() boom() end)",
        "setmetatable({}, {__index = function() boom() end}).x = nil local _ = setmetatable({}, {__index = function() boom() end}).y",
    ] {
        let mut lua = Lua::new(Limits::default().fuel(1_000_000), Libs::ALL);
        lua.register("boom", |_, _| panic!("host bug")).unwrap();
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec(place, "=t")));
        assert!(r.is_err(), "{place}: expected the panic");
        // Afterwards: either refused, or fully working with its full budget each call.
        let mut after = Ok(vec![]);
        for _ in 0..20 {
            after = lua.exec(
                "local n = 0 for i = 1, 100000 do n = n + i end return n, coroutine.isyieldable()",
                "=after",
            );
        }
        match after {
            Err(Error::Host(_)) => {}
            Ok(v) => {
                assert_eq!(v.len(), 2, "{place}");
                assert!(matches!(v[0], Value::Int(5000050000)), "{place}: {v:?}");
                assert!(matches!(v[1], Value::Bool(false)), "{place}: runs as a coroutine: {v:?}");
                let more = lua.exec(
                    "local t = {} for i = 1, 10000 do t[i] = {} end collectgarbage() return #t",
                    "=more",
                );
                assert!(more.is_ok(), "{place}: {more:?}");
            }
            Err(e) => panic!("{place}: after the panic: {e:?}"),
        }
    }
}

/// The host's own `load` of a huge source stops when another thread interrupts it
/// (SEC3-17), and the state compiles again after a reset.
#[test]
fn sec3_17_host_load_is_interruptible() {
    let r = bounded(60, || {
        let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::ALL);
        let src = "return ".to_owned() + &"(".repeat(1 << 21) + "1" + &")".repeat(1 << 21);
        let h = lua.interrupt_handle();
        let h2 = h.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            h2.interrupt();
            Instant::now()
        });
        let r = lua.load(&src, "=big").map(|_| ());
        let end = Instant::now();
        let at = t.join().unwrap();
        h.reset();
        let again = lua.exec("return 1 + 1", "=again").is_ok();
        (r, end.saturating_duration_since(at), again)
    })
    .unwrap();
    assert!(matches!(r.0, Err(Error::Limit(LimitKind::Interrupted))), "{:?}", r.0);
    assert!(r.1 < Duration::from_millis(50), "stopped {:?} after the interrupt", r.1);
    assert!(r.2);
}

/// Host and Lua calling each other without end: an error, not a crash.
#[test]
fn host_lua_mutual_recursion_stops() {
    let r = bounded(60, || {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        lua.register("again", |lua, args| lua.call(args[0], &args[..1])).unwrap();
        outcome(lua.exec("local function f(self) return again(self) end return f(f)", "=t"))
    })
    .unwrap();
    assert!(r.starts_with("error") || r.starts_with("limit"), "{r}");
}

#[test]
#[ignore = "probe: compile time against nesting depth"]
fn probe_compile_scaling() {
    for (open, close) in [("function f() ", "end "), ("(", ")"), ("do ", "end "), ("{", "}")] {
        for k in [12, 14, 16, 17] {
            let n = 1usize << k;
            let mut src = String::from(if open == "(" || open == "{" { "return " } else { "" });
            src.push_str(&open.repeat(n));
            src.push_str(&close.repeat(n));
            let mut lua = Lua::new(Limits::unmetered().memory(4 << 30), Libs::ALL);
            let start = Instant::now();
            let r = lua.load(&src, "=deep").map(|_| ());
            let t = start.elapsed();
            println!(
                "{open:?} x{n}: {:?} {:.0} ns/byte {:?}",
                t,
                t.as_nanos() as f64 / src.len() as f64,
                r.err()
            );
        }
    }
}

// ---------------------------------------------------------------- collector

/// Weak tables and ephemerons with a collection at every allocation, fast and full: the
/// results C Lua gives, and the same fuel and memory both ways.
#[test]
fn weak_tables_and_ephemerons_under_gc_stress() {
    // On a 64 KiB stack: data-driven native recursion overflows here, not in production.
    super::common::small_stack(weak_tables_and_ephemerons_under_gc_stress_body);
}

fn weak_tables_and_ephemerons_under_gc_stress_body() {
    let src = r#"
        local out = {}
        -- An ephemeron chain: each value holds the next key; only the head is rooted.
        local e = setmetatable({}, {__mode = 'k'})
        local keys = {}
        for i = 1, 60 do keys[i] = {} end
        for i = 1, 59 do e[keys[i]] = keys[i + 1] end
        e[keys[60]] = 'end'
        local head = keys[1]
        keys = nil
        collectgarbage() collectgarbage()
        local n, k = 0, head
        while e[k] ~= nil and type(e[k]) == 'table' do n = n + 1 k = e[k] end
        out[#out + 1] = n
        -- A value that refers to its own key keeps nothing alive.
        local w = setmetatable({}, {__mode = 'k'})
        for i = 1, 50 do local key = {} w[key] = {key, i} end
        collectgarbage()
        local c = 0 for _ in pairs(w) do c = c + 1 end
        out[#out + 1] = c
        -- Weak values: strings stay, tables go; a rooted one stays.
        local v = setmetatable({}, {__mode = 'v'})
        local keep = {}
        v[1], v[2], v[3], v[4] = 'str', {}, keep, ('x'):rep(100)
        collectgarbage()
        out[#out + 1] = tostring(v[1]) .. ',' .. tostring(v[2]) .. ',' .. tostring(v[3] == keep) .. ',' .. #v[4]
        -- Mode changed while full, and keys reused after a collection.
        local m = {}
        for i = 1, 40 do m[{}] = i end
        setmetatable(m, {__mode = 'k'})
        collectgarbage()
        out[#out + 1] = tostring(next(m) == nil)
        for i = 1, 40 do m[{}] = i end
        local t2 = {} m[t2] = 'kept'
        collectgarbage()
        local c2 = 0 for k2, v2 in pairs(m) do c2 = c2 + 1 assert(k2 == t2 and v2 == 'kept') end
        out[#out + 1] = c2
        -- Coroutines as weak keys and a weak table inside a dead coroutine.
        local wc = setmetatable({}, {__mode = 'k'})
        for i = 1, 10 do wc[coroutine.create(function() coroutine.yield() end)] = i end
        collectgarbage()
        out[#out + 1] = tostring(next(wc) == nil)
        return (table.concat(out, ' '):gsub('true', 'T'))
    "#;
    let mut seen = Vec::new();
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().hash_seed(7), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.set_gc_stress(true);
        let r = lua.exec(src, "=t");
        let r = match r {
            Ok(v) => lua.to_string_lossy(v[0]).unwrap_or_default(),
            Err(e) => format!("{e:?}"),
        };
        seen.push((r, lua.fuel_left(), lua.memory_used()));
    }
    assert_eq!(seen[0].0, "59 0 str,nil,T,100 T 1 T", "{seen:?}");
    assert_eq!(seen[0], seen[1]);
}

/// Table buffers go back to the pool and come out again: a reused buffer is empty, and
/// after many rounds of churn the memory held returns to the same baseline.
#[test]
fn pool_reuse_is_clean_and_accounting_does_not_drift() {
    let churn = r#"
        local live = {}
        for r = 1, 300 do
            local n = (r * 7) % 70
            local t = {}
            for i = 1, n do t[i] = i end
            for i = 1, n % 33 do t['k' .. i] = i end
            live[r % 17] = t
            local fresh = {}
            assert(next(fresh) == nil and #fresh == 0)
            local fresh2 = {nil, nil, nil}
            assert(next(fresh2) == nil)
            for i = 1, n do t[i] = nil end
        end
        return #live
    "#;
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().hash_seed(3), Libs::ALL);
        lua.set_fast_loop(fast);
        let f = lua.load(churn, "=churn").unwrap();
        let f = lua.root(f.value());
        lua.call(f.value(), &[]).unwrap();
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        for round in 0..40 {
            lua.call(f.value(), &[]).unwrap();
            lua.collect_garbage().unwrap();
            assert_eq!(lua.memory_used(), base, "fast {fast}: drift after round {round}");
        }
    }
}

// ---------------------------------------------------------------- budgets

/// Fuel handed out in slices: what a program uses does not depend on the budget's size
/// (one slice, many, or effectively unlimited), and `fuel_left` is exact.
#[test]
fn fuel_slices_do_not_change_what_a_call_uses() {
    let src = "local t = {} for i = 1, 30000 do t[i] = tostring(i) .. 'x' end table.sort(t) \
               local n = 0 for w in table.concat(t, ' '):gmatch('%d+') do n = n + 1 end collectgarbage() return n";
    let mut used = Vec::new();
    for fuel in [5_000_000u64, 1 << 20 + 3, 1 << 40, u64::MAX] {
        for fast in [true, false] {
            let mut lua = Lua::new(Limits::default().fuel(fuel).hash_seed(5), Libs::ALL);
            lua.set_fast_loop(fast);
            let r = outcome(lua.exec(src, "=t"));
            assert_eq!(r, "ok", "fuel {fuel}");
            // Budgets past i64::MAX count as i64::MAX.
            used.push(fuel.min(i64::MAX as u64) - lua.fuel_left());
        }
    }
    assert!(used.windows(2).all(|w| w[0] == w[1]), "{used:?}");
    // And a budget just short of that stops it.
    let need = used[0];
    let mut lua = Lua::new(Limits::default().fuel(need - 1).hash_seed(5), Libs::ALL);
    assert_eq!(outcome(lua.exec(src, "=t")), "limit Fuel");
    let mut lua = Lua::new(Limits::default().fuel(need).hash_seed(5), Libs::ALL);
    assert_eq!(outcome(lua.exec(src, "=t")), "ok");
}

/// After an interrupt (in a loop, a coroutine, a sort, a pattern, a collection) and a
/// reset, the same state runs again with its full budget; until the reset every call stops.
#[test]
fn state_usable_after_interrupt_and_reset() {
    for work in [
        "while true do end",
        "coroutine.wrap(function() while true do end end)()",
        "local co = coroutine.wrap(function() while true do coroutine.yield() end end) while true do co() end",
        "local t = {} for i = 1, 200000 do t[i] = -i end while true do table.sort(t, function(a, b) return a > b end) end",
        "local s = ('a'):rep(2000000) while true do s:find('.-.-.-b') end",
        "local t = {} for i = 1, 300000 do t[i] = {} end while true do collectgarbage() end",
        "while true do pcall(error, 'x') end",
    ] {
        let w = work.to_owned();
        bounded(60, move || {
            let mut lua = Lua::new(Limits::unmetered().memory(1 << 30), Libs::ALL);
            let h = lua.interrupt_handle();
            let h2 = h.clone();
            let t = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                h2.interrupt();
            });
            assert_eq!(outcome(lua.exec(&w, "=w")), "limit Interrupted", "{w}");
            t.join().unwrap();
            assert_eq!(outcome(lua.exec("return 1", "=x")), "limit Interrupted", "{w}: still set");
            h.reset();
            lua.set_limits(Limits::default());
            let r = lua
                .exec("local n = 0 for i = 1, 1000 do n = n + i end return n, coroutine.running()", "=after")
                .unwrap();
            assert!(matches!(r[0], Value::Int(500500)), "{w}");
            assert!(matches!(r[1], Value::Thread(_)), "{w}");
            assert_eq!(outcome(lua.exec("while true do end", "=again")), "limit Fuel", "{w}");
        })
        .unwrap_or_else(|e| panic!("{e}: {work}"));
    }
}

/// A host clock that panics: the call unwinds, and the state is refused afterwards.
#[test]
fn panicking_clock_poisons_the_state() {
    let mut lua = Lua::new(Limits::unmetered(), Libs::ALL);
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let c = calls.clone();
    lua.set_time_limit(Some((
        std::sync::Arc::new(move || {
            if c.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > 0 {
                panic!("clock broke");
            }
            Duration::ZERO
        }),
        Duration::from_secs(10),
    )));
    let r = std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec("while true do end", "=t")));
    assert!(r.is_err());
    lua.set_time_limit(None);
    assert!(matches!(lua.exec("return 1", "=t"), Err(Error::Host(_))));
}

/// Error messages name variables by reading the function's bytecode up to the error:
/// charged, so an error raised over and over at the end of a huge function stops on fuel
/// in proportion to that work.
#[test]
fn error_naming_work_is_charged() {
    let r = bounded(60, || {
        // 2^17 statements take about 75 MB to compile, charged as allocated since
        // S6-FRONT-3 (they fit the default 64 MiB only while that was undercounted).
        let mut lua = Lua::new(Limits::default().fuel(1 << 40).memory(256 << 20), Libs::ALL);
        lua.exec("f = load('if x then ' .. string.rep('a = 1 ', 2^17) .. ' end return y()')", "=s").unwrap();
        lua.set_limits(lua.limits().fuel(2_000_000));
        let start = Instant::now();
        let r = outcome(lua.exec("while true do pcall(f) end", "=t"));
        (r, start.elapsed())
    })
    .unwrap();
    assert_eq!(r.0, "limit Fuel");
    assert!(r.1 < Duration::from_secs(2), "2M fuel took {:?}", r.1);
}

/// Natives whose work was not proportional to the fuel charged (SEC3-3 to SEC3-11): each
/// now runs out of 300k fuel within a second (they took minutes or more before).
#[test]
fn sec3_stdlib_work_is_charged() {
    const CASES: &[(&str, &str)] = &[
        ("s = string.rep('\\u{7FF}', 2^20)", "local _ = utf8.offset(s, -1)"),
        ("s = string.rep('\\u{7FF}', 2^20)", "local _ = utf8.offset(s, 2^20)"),
        ("s = string.rep('\\x80', 2^20)", "local _ = utf8.offset(s, 0, -1)"),
        ("s = string.rep('1', 2^22)", "local _ = tonumber(s, 36)"),
        ("s = string.rep('%a', 2^19)", "local _ = os.date(s, 0)"),
        ("s = string.rep('a', 2^18) p = '[' .. string.rep('b', 2^18) .. ']'", "local _ = s:find(p)"),
        ("s = string.rep('a', 2^18) p = '[' .. string.rep('b', 2^18) .. ']*c'", "local _ = s:find(p)"),
        ("s = string.rep('a', 2^18) p = '%f[' .. string.rep('b', 2^18) .. ']'", "local _ = s:find(p)"),
        ("s = string.rep('x', 2^22)", "pcall(load, 'return 1', s)"),
        ("", "pcall(string.pack, string.rep('x', 2^20) .. 'c0')"),
        ("", "pcall(string.packsize, string.rep('x', 2^20) .. 'z')"),
        ("s = string.rep('a', 2^16) r = string.rep('%0', 2^12)", "local _ = s:gsub('a', r)"),
        ("s = 'return ' .. string.rep('(', 2^14) .. '1' .. string.rep(')', 2^14)", "local _ = load(s)"),
        ("s = string.rep('function f() ', 2^14) .. string.rep('end ', 2^14)", "local _ = load(s)"),
        ("f = load('if x then ' .. string.rep('a = 1 ', 2^16) .. ' end return y()')", "pcall(f)"),
        (
            "s = string.rep('x', 2^20) f = load('if x then ' .. string.rep('a = 1 ', 2^10) .. ' end return t[\"' .. s .. '\"]')",
            "pcall(f)",
        ),
    ];
    for (setup, body) in CASES {
        for fast in [true, false] {
            let (s, b) = (setup.to_string(), body.to_string());
            let r = bounded(60, move || {
                let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40), Libs::ALL);
                lua.set_fast_loop(fast);
                lua.exec(&s, "=setup").unwrap();
                lua.set_limits(lua.limits().fuel(300_000));
                let f = lua.load(format!("while true do {b} end"), "=work").unwrap();
                let start = Instant::now();
                let r = outcome(lua.call(f.value(), &[]));
                (r, start.elapsed())
            })
            .unwrap_or_else(|e| panic!("{e}: {body}"));
            assert!(r.0 == "limit Fuel" || r.0 == "limit Memory", "{body}: {}", r.0);
            assert!(r.1 < Duration::from_secs(1), "{body}: 300k fuel took {:?}", r.1);
        }
    }
}

// ---------------------------------------------------------------- hostile fuzz

/// A small xorshift generator (no dependencies).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

/// A random hostile statement over the stdlib: sizes up to megabytes, odd patterns,
/// errors caught in loops, deep nesting, collections and coroutines.
fn hostile_stmt(r: &mut Rng) -> String {
    let n = 1usize << r.below(21);
    let small = 1 + r.below(300);
    let pats = [
        "'.-b'",
        "'[%a%d]+'",
        "'(a*)%1'",
        "'%f[%w]%w+'",
        "'[' .. string.rep('b', N) .. ']'",
        "'^(a-)(a-)b'",
        "''",
        "'%b()'",
        "'a?a?a?a?b'",
    ];
    let pat = r.pick(&pats).replace('N', &small.to_string());
    let s = format!("string.rep({}, {n})", r.pick(&["'a'", "'ab'", "'\\u{7FF}'", "'(()'", "'%a'", "' '"]));
    let stmts = [
        format!("local s = {s} local _ = s:find({pat})"),
        format!("local s = {s} local _ = s:find(string.rep('a', {small}) .. 'b', 1, true)"),
        format!("local s = {s} local _ = s:gsub({pat}, '%0%0')"),
        format!("local s = {s} for _ in s:gmatch({pat}) do end"),
        format!("local s = {s} local _ = s:match({pat})"),
        format!("local s = {s} local _ = utf8.offset(s, {}, -1)", r.below(3) as i64 - 1),
        format!("local s = {s} local _ = utf8.len(s)"),
        format!("local _ = tonumber(string.rep('7', {n}), {})", 2 + r.below(35)),
        format!("local _ = os.date(string.rep('%c', {small}), 0)"),
        format!("pcall(string.pack, string.rep('x', {n}) .. 'i')"),
        format!("pcall(load, 'return ' .. string.rep('(', {small}) .. '1' .. string.rep(')', {small}))"),
        format!("pcall(load, string.rep('function f() ', {small}) .. string.rep('end ', {small}))"),
        format!("pcall(load, 'return 1', string.rep('n', {n}))"),
        format!("pcall(function() local t = setmetatable({{}}, {{__name = string.rep('x', {n})}}) return t + 1 end)"),
        format!("pcall(function() return ({s})() end)"),
        "local w = setmetatable({}, {__mode = 'k'}) for i = 1, 300 do w[{}] = {} end collectgarbage()".into(),
        format!("local t = {{}} for i = 1, {small} do t[i] = {{}} end t = nil collectgarbage('step', {small})"),
        format!("local co = coroutine.wrap(function() for i = 1, {small} do coroutine.yield(i) end end) for i = 1, {small} do co() end"),
        format!("local t = {{}} for i = 1, {small} do t[i] = -i end table.sort(t, function(a, b) return a > b end)"),
        "local e = setmetatable({}, {__mode = 'k'}) local k = {} for i = 1, 50 do local n = {} e[k] = n k = n end collectgarbage()".into(),
        format!("pcall(error, string.rep('e', {n}))"),
        format!("local _ = string.format('%q', string.rep('\\0', {small}))"),
    ];
    stmts[r.below(stmts.len())].clone()
}

fn fuzz_program(r: &mut Rng) -> String {
    let mut src = String::from("for _ = 1, 3 do\n");
    for _ in 0..1 + r.below(4) {
        src.push_str(&hostile_stmt(r));
        src.push('\n');
    }
    src.push_str("end\nreturn 'done'");
    src
}

/// Random hostile programs under small budgets, fast against full: no panic, no hang,
/// the same outcome, fuel and memory, and the state still runs code afterwards. Every
/// fourth program is also interrupted from another thread and must stop at once.
/// Long run: `SWEEP3_SEC_ITERS=3000 SWEEP3_SEC_SEED=9 cargo test -p remedy-lua --test all sweep3_sec::hostile -- --nocapture`
#[test]
fn hostile_fuzz_budgets_and_natives() {
    let iters: usize = std::env::var("SWEEP3_SEC_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let seed: u64 = std::env::var("SWEEP3_SEC_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x5eed_3);
    let mut r = Rng(seed | 1);
    for i in 0..iters {
        let src = fuzz_program(&mut r);
        let limits =
            Limits::default().fuel(200_000 + r.below(800_000) as u64).memory(32 << 20).hash_seed(seed);
        let s = src.clone();
        let res = bounded(30, move || super::common::small_stack(move || {
            let mut out = Vec::new();
            for fast in [true, false] {
                let mut lua = Lua::new(limits, Libs::ALL);
                lua.set_fast_loop(fast);
                let start = Instant::now();
                let o = outcome(lua.exec(&s, "=fuzz"));
                let t = start.elapsed();
                let after = outcome(lua.exec("return 1", "=after"));
                out.push((o, limits.fuel - lua.fuel_left(), lua.memory_used(), after, t));
            }
            out
        }))
        .unwrap_or_else(|e| panic!("iteration {i}: {e}\n{src}"));
        let (a, b) = (&res[0], &res[1]);
        assert_eq!((&a.0, a.1, a.2), (&b.0, b.1, b.2), "iteration {i}: fast and full differ\n{src}");
        assert_eq!(a.3, "ok", "iteration {i}: state unusable after\n{src}");
        let worst = a.4.max(b.4);
        assert!(worst < Duration::from_secs(5), "iteration {i}: took {worst:?}\n{src}");
        if i % 4 == 0 {
            let s = format!("while true do\n{}\nend", fuzz_program(&mut r).replace("return 'done'", ""));
            let delay = r.below(40) as u64;
            let label = s.clone();
            let res = bounded(30, move || super::common::small_stack(move || {
                let mut lua = Lua::new(Limits::unmetered().memory(64 << 20), Libs::ALL);
                let h = lua.interrupt_handle();
                let t = std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(delay));
                    h.interrupt();
                    Instant::now()
                });
                let o = outcome(lua.exec(&s, "=fuzz"));
                let end = Instant::now();
                let at = t.join().unwrap();
                (o, end.saturating_duration_since(at))
            }))
            .unwrap_or_else(|e| panic!("iteration {i} (interrupted): {e}\n{label}"));
            assert!(
                res.0 == "limit Interrupted" || res.0 == "limit Memory" || res.0.starts_with("error"),
                "{}\n{label}",
                res.0
            );
            assert!(
                res.1 < Duration::from_millis(300),
                "iteration {i}: stopped {:?} after the interrupt\n{label}",
                res.1
            );
        }
    }
}

// ---------------------------------------------------------------- fuel against time

/// A family of calls for the fuel-against-time property: what it covers (a library
/// function's name, or `op:` and an opcode family), and a generator that, for an input
/// size `n`, makes setup code and a loop body with random arguments of about that size.
type Gen = fn(usize, &mut Rng) -> (String, String);

/// A random string expression of about `n` bytes (setup code).
fn rand_str(n: usize, r: &mut Rng) -> String {
    let piece =
        r.pick(&["'a'", "'ab'", "'%a('", "'\\u{7FF}'", "'1'", "'\\0x'", "' '", "'[%]'", "'aaab'", "'\\x80'"]);
    format!("string.rep({piece}, {})", n.max(1))
}

/// A random pattern of about `n` bytes where the pattern's size matters, else short.
fn rand_pat(n: usize, r: &mut Rng) -> String {
    let k = 1 + r.below(n.clamp(1, 64));
    match r.below(12) {
        0 => "'a-b'".into(),
        1 => "'[%a%d]+'".into(),
        2 => "'(a*)%1'".into(),
        3 => "'%f[%w]%w+'".into(),
        4 => format!("'[' .. string.rep('b', {n}) .. ']'"),
        5 => "'.-.-b'".into(),
        6 => "''".into(),
        7 => "'%b()'".into(),
        8 => format!("string.rep('a?', {k}) .. string.rep('a', {k})"),
        9 => format!("string.rep('(a)', {})", k.min(31)),
        10 => format!("'%f[' .. string.rep('%a', {n}) .. ']'"),
        _ => format!("string.rep('a', {n}) .. 'b'"),
    }
}

fn rand_fmt(n: usize, r: &mut Rng) -> String {
    let spec = r.pick(&[
        "%d", "%5.2f", "%g", "%a", "%q", "%s", "%.3s", "%99.99f", "%x", "%c", "%-20s", "%i", "%%", "%e",
    ]);
    format!("string.rep('{spec} ', {})", n.clamp(1, 1 << 16))
}

/// `t`: a table of `n` items of a random kind (setup code).
fn rand_tab(n: usize, r: &mut Rng) -> String {
    match r.below(5) {
        0 => format!("t = {{}} for i = 1, {n} do t[i] = i end"),
        1 => format!("t = {{}} for i = 1, {n} do t[i] = tostring(i) end"),
        2 => format!("t = {{}} for i = 1, {n} do t['k' .. i] = i end"),
        3 => format!("t = {{}} for i = 1, {n} do t[i * 2] = i * 0.5 end"),
        _ => format!("t = {{}} for i = 1, {n} do t[i] = -i end for i = 1, {n}, 3 do t[i] = nil end"),
    }
}

fn s_and(n: usize, r: &mut Rng, extra: &str) -> String {
    format!("s = {} s2 = {} {extra}", rand_str(n, r), rand_str(n, r))
}

const FAMILIES: &[(&str, Gen)] = &[
    // ---- base
    ("assert", |n, r| (s_and(n, r, ""), "pcall(assert, false, s)".into())),
    ("collectgarbage", |n, r| {
        (
            rand_tab(n, r),
            r.pick(&[
                "collectgarbage()",
                "collectgarbage('step', 1)",
                "local _ = collectgarbage('count')",
                "collectgarbage('incremental')",
                "pcall(collectgarbage, string.rep('x', #t + 1))",
            ])
            .into(),
        )
    }),
    ("error", |n, r| {
        (
            s_and(n, r, ""),
            r.pick(&["pcall(error, s)", "pcall(error, s, 2)", "pcall(error, {})", "pcall(error)"]).into(),
        )
    }),
    ("getmetatable", |_, _| {
        (
            "t = setmetatable({}, {__metatable = 1})".into(),
            "local _ = getmetatable(t), getmetatable('')".into(),
        )
    }),
    ("ipairs", |n, r| (rand_tab(n, r), "for _ in ipairs(t) do end".into())),
    ("load", |n, r| {
        // A reader function handing the source over in pieces (S5SEC-5).
        if r.below(4) == 0 {
            return (
                format!(
                    "piece = string.rep('local a = 1 ', {}) function reader() local d = done done = not d if d then return nil end return piece end",
                    n.max(1)
                ),
                "local _ = load(reader)".into(),
            );
        }
        let src = match r.below(6) {
            0 => format!("'return ' .. string.rep('(', {n}) .. '1' .. string.rep(')', {n})"),
            1 => format!("string.rep('function f() ', {n}) .. string.rep('end ', {n})"),
            2 => format!("'return {{' .. string.rep('1,', {n}) .. '}}'"),
            3 => format!("string.rep('local a = a + 1 ', {n})"),
            4 => format!("'return ' .. string.rep('not ', {n}) .. '1'"),
            _ => format!("'x = \"' .. string.rep('\\\\65', {n}) .. '\"'"),
        };
        (
            format!("src = {src} name = string.rep('n', {n})"),
            r.pick(&[
                "local _ = load(src)",
                "local _ = load(src, name)",
                "local _ = load(src .. ')')",
                "local _ = load('x = 1', nil, name)",
            ])
                .into(),
        )
    }),
    ("next", |n, r| {
        (
            format!("{} for i = 1, {n} // 2 do t[i] = nil t['k' .. i] = nil end t.z = 1", rand_tab(n, r)),
            "local _ = next(t)".into(),
        )
    }),
    ("pairs", |n, r| (rand_tab(n, r), "for _ in pairs(t) do end".into())),
    ("pcall", |_, r| {
        (String::new(), r.pick(&["pcall(pcall, error, 1)", "pcall(function() end)", "pcall(nil)"]).into())
    }),
    ("print", |n, r| (s_and(n, r, ""), "print(s, 1, 2.5, nil)".into())),
    ("rawequal", |n, r| (s_and(n, r, ""), "local _ = rawequal(s, s2)".into())),
    ("rawget", |n, r| (rand_tab(n, r), "local _ = rawget(t, 1), rawget(t, 'k1')".into())),
    ("rawlen", |n, r| {
        let t = rand_tab(n, r);
        (s_and(n, r, &t), r.pick(&["local _ = rawlen(t)", "local _ = rawlen(s)"]).into())
    }),
    ("rawset", |n, r| (rand_tab(n, r), format!("rawset(t, {}, 1)", r.below(n + 2)))),
    ("select", |n, r| {
        (
            format!("{} u = {{table.unpack(t, 1, {})}}", rand_tab(n, r), n.min(4000)),
            "local _ = select('#', table.unpack(u)), select(1, table.unpack(u))".into(),
        )
    }),
    ("setmetatable", |_, _| (String::new(), "setmetatable({}, {__index = {}})".into())),
    ("tonumber", |n, r| {
        (
            format!("s = string.rep('{}', {n})", r.pick(&["1", "1", " ", "z", "0x1"])),
            if r.below(2) == 0 {
                "local _ = tonumber(s)".into()
            } else {
                format!("local _ = tonumber(s, {})", 2 + r.below(35))
            },
        )
    }),
    ("tostring", |n, r| {
        (
            format!("{} x = 1.5e300 s = {}", rand_tab(n, r), rand_str(n, r)),
            r.pick(&["local _ = tostring(t)", "local _ = tostring(x)", "local _ = tostring(s)"]).into(),
        )
    }),
    ("type", |_, _| (String::new(), "local _ = type(1), type('')".into())),
    ("xpcall", |n, r| (s_and(n, r, ""), "xpcall(error, function(m) return m end, s)".into())),
    ("warn", |n, r| (s_and(n, r, ""), "warn(s, s2)".into())),
    // ---- string
    ("string.byte", |n, r| {
        if r.below(3) == 0 {
            // The whole string (S5SEC-5: no shape read more than 300 bytes).
            return (s_and(n.min(1 << 16), r, ""), "local _ = s:byte(1, -1)".into());
        }
        (s_and(n, r, ""), format!("local _ = s:byte({}, {})", r.below(n + 1) as i64 - 1, r.below(300)))
    }),
    ("string.char", |n, r| {
        if r.below(2) == 0 {
            // Many arguments, the last one bad: the error comes after the work (S5SEC-5).
            return (
                format!("t = {{}} for i = 1, {} do t[i] = 65 end t[#t + 1] = -1", n.min(4000)),
                "pcall(string.char, table.unpack(t))".into(),
            );
        }
        (String::new(), "local _ = string.char(65, 66, 67, 0, 255)".into())
    }),
    ("string.dump", |_, _| (String::new(), "pcall(string.dump, print)".into())),
    // ---- opt-in: `Libs::DEBUG_INFO` (`docs/reference.md`)
    ("debug.traceback", |n, r| {
        (
            format!(
                "for i = 1, {n} do _G['g' .. i] = i end for i = 1, {} do package.loaded['m' .. i] = {{i, i}} end \
                 MSG = string.rep('x', {n}) function D(d, f) if d == 0 then return f() end local r = D(d - 1, f) return r end",
                n.min(4096)
            ),
            r.pick(&[
                "local s = debug.traceback()",
                "local s = debug.traceback(MSG)",
                "local s = debug.traceback(MSG, 0)",
                "D(math.min(#MSG, 1500), function() return debug.traceback() end)",
                "local s = select(2, xpcall(error, debug.traceback, MSG))",
                "local co = coroutine.create(function() coroutine.yield() end) coroutine.resume(co) local s = debug.traceback(co, MSG)",
            ])
            .into(),
        )
    }),
    ("debug.getinfo", |n, r| {
        (
            format!("function D(d, f) if d == 0 then return f() end local r = D(d - 1, f) return r end N = {}", n.min(1500)),
            r.pick(&[
                "local t = debug.getinfo(1, 'Slnut')",
                "local t = debug.getinfo(print)",
                "D(N, function() for i = 1, 40 do local t = debug.getinfo(i, 'Sln') end end)",
                "pcall(debug.getinfo, 1, string.rep('S', N))",
                "pcall(debug.getinfo, 'x')",
            ])
            .into(),
        )
    }),
    // ---- opt-in: `Libs::REQUIRE`
    ("require", |n, r| {
        (
            format!(
                "for i = 1, {n} do package.loaded['k' .. i] = i end package.preload.pm = function(name, data) return {{name, data}} end \
                 M = string.rep('m', {n}) for i = 1, {} do package.searchers[#package.searchers + 1] = function() return M end end",
                (n / 64).clamp(1, 256)
            ),
            r.pick(&[
                "local m = require('string')",
                "package.loaded.pm = nil local m = require('pm')",
                "package.loaded.hostmod = nil local m = require('hostmod')",
                "pcall(require, 'nope')",
                "pcall(require, M)",
            ])
            .into(),
        )
    }),
    ("string.find", |n, r| {
        (
            s_and(n, r, ""),
            format!(
                "pcall(string.find, s, {}, {}, {})",
                rand_pat(n, r),
                r.below(n + 2),
                r.pick(&["true", "false"])
            ),
        )
    }),
    ("string.format", |n, r| {
        let f = format!("f = {}", rand_fmt(n, r));
        (s_and(n, r, &f), "pcall(string.format, f, 1, 2.5, s, 3, 4)".into())
    }),
    ("string.gmatch", |n, r| {
        (s_and(n, r, ""), format!("pcall(function() for _ in s:gmatch({}) do end end)", rand_pat(n, r)))
    }),
    ("string.gsub", |n, r| {
        (
            s_and(n, r, ""),
            format!(
                "pcall(string.gsub, s, {}, {})",
                rand_pat(n, r),
                r.pick(&["'%0%0'", "{}", "function(x) return x end", "string.rep('%0', 64)", "''"])
            ),
        )
    }),
    ("string.len", |n, r| (s_and(n, r, ""), "local _ = s:len()".into())),
    ("string.lower", |n, r| (s_and(n, r, ""), "local _ = s:lower()".into())),
    ("string.match", |n, r| (s_and(n, r, ""), format!("pcall(string.match, s, {})", rand_pat(n, r)))),
    ("string.pack", |n, r| {
        (
            format!("f = string.rep('{}', {})", r.pick(&["i4", "x", "s", "z", "d", "!8"]), n.min(1 << 16)),
            "pcall(string.pack, f, 1, 'a', 2, 'b')".into(),
        )
    }),
    ("string.packsize", |n, r| {
        (
            format!("f = string.rep('{}', {})", r.pick(&["i4", "x", "d", "c3"]), n.min(1 << 16)),
            "pcall(string.packsize, f)".into(),
        )
    }),
    ("string.rep", |n, r| {
        (s_and(n, r, ""), format!("pcall(string.rep, s, {}, {})", r.below(8), r.pick(&["nil", "s", "','"])))
    }),
    ("string.reverse", |n, r| (s_and(n, r, ""), "local _ = s:reverse()".into())),
    ("string.sub", |n, r| {
        (s_and(n, r, ""), format!("local _ = s:sub({}, -{})", r.below(n + 1), r.below(n + 1)))
    }),
    ("string.unpack", |n, _| {
        (
            format!("f = string.rep('i4', {}) d = string.rep('\\0', {})", n.min(1 << 14), n.min(1 << 14) * 4),
            "pcall(string.unpack, f, d)".into(),
        )
    }),
    ("string.upper", |n, r| (s_and(n, r, ""), "local _ = s:upper()".into())),
    ("string arithmetic", |n, _| {
        (
            format!("s = string.rep('1', {})", n.min(300)),
            "pcall(function() return s + 1, s * '2', -s, s - 1, s / 2, s % 3, s ^ 1, s // 2 end)".into(),
        )
    }),
    // ---- table
    ("table.concat", |n, r| {
        let t = rand_tab(n, r);
        match r.below(4) {
            // The range given explicitly (S5SEC-5: a planted cost on that path alone went
            // unseen).
            3 => (t, "pcall(table.concat, t, ',', 1, #t)".into()),
            // A bad value at the end: the error comes after the output is built (S5SEC-5:
            // no shape raised late, so a charge an error skips went unseen).
            0 => (
                format!("{t} for i = 1, {n} do t[i] = 'abcdefgh' .. i end t[{n} + 1] = {{}}"),
                "pcall(table.concat, t, ',')".into(),
            ),
            1 => (format!("{t} sep = string.rep(',', {})", n.clamp(1, 4096)), "pcall(table.concat, t, sep)".into()),
            _ => (t, "pcall(table.concat, t, ',')".into()),
        }
    }),
    ("table.insert", |n, r| {
        (rand_tab(n, r), "pcall(table.insert, t, 1, 0) pcall(table.remove, t, 1)".into())
    }),
    ("table.move", |n, r| (rand_tab(n, r), format!("table.move(t, 1, {n}, 2)"))),
    ("table.pack", |n, r| {
        (rand_tab(n, r), format!("local _ = table.pack(table.unpack(t, 1, {}))", n.min(4000)))
    }),
    ("table.remove", |n, r| (rand_tab(n, r), "pcall(table.remove, t) t[#t + 1] = 1".into())),
    ("table.sort", |n, r| {
        (
            rand_tab(n, r),
            r.pick(&[
                "pcall(table.sort, t)",
                "pcall(table.sort, t, function(a, b) return tostring(a) < tostring(b) end)",
            ])
            .into(),
        )
    }),
    ("table.unpack", |n, r| (rand_tab(n, r), format!("pcall(table.unpack, t, 1, {})", n.min(1 << 16)))),
    // ---- math
    ("math.abs", |_, _| (String::new(), "local _ = math.abs(-3), math.abs(math.mininteger)".into())),
    ("math.acos", |_, _| (String::new(), "local _ = math.acos(0.5)".into())),
    ("math.asin", |_, _| (String::new(), "local _ = math.asin(0.5)".into())),
    ("math.atan", |_, _| (String::new(), "local _ = math.atan(1, 2)".into())),
    ("math.ceil", |_, _| (String::new(), "local _ = math.ceil(2.5), math.ceil(1e300)".into())),
    ("math.cos", |_, _| (String::new(), "local _ = math.cos(1e300)".into())),
    ("math.deg", |_, _| (String::new(), "local _ = math.deg(1)".into())),
    ("math.exp", |_, _| (String::new(), "local _ = math.exp(700)".into())),
    ("math.floor", |_, _| (String::new(), "local _ = math.floor(-2.5), math.floor(-1e300)".into())),
    ("math.fmod", |_, _| (String::new(), "local _ = math.fmod(1e300, 3), pcall(math.fmod, 1, 0)".into())),
    ("math.log", |_, _| (String::new(), "local _ = math.log(1e300, 7)".into())),
    ("math.max", |n, r| (rand_tab(n.min(4000), r), "pcall(math.max, table.unpack(t))".into())),
    ("math.min", |n, r| (rand_tab(n.min(4000), r), "pcall(math.min, table.unpack(t))".into())),
    ("math.modf", |_, _| (String::new(), "local _ = math.modf(-2.5)".into())),
    ("math.rad", |_, _| (String::new(), "local _ = math.rad(180)".into())),
    ("math.random", |_, r| {
        (
            String::new(),
            r.pick(&[
                "local _ = math.random()",
                "local _ = math.random(1, math.maxinteger)",
                "local _ = math.random(0)",
            ])
            .into(),
        )
    }),
    ("math.randomseed", |_, _| (String::new(), "math.randomseed(42)".into())),
    ("math.sin", |_, _| (String::new(), "local _ = math.sin(1e300)".into())),
    ("math.sqrt", |_, _| (String::new(), "local _ = math.sqrt(2)".into())),
    ("math.tan", |_, _| (String::new(), "local _ = math.tan(1e300)".into())),
    ("math.tointeger", |n, _| {
        (format!("s = string.rep('1', {})", n.min(1000)), "local _ = math.tointeger(s)".into())
    }),
    ("math.type", |_, _| (String::new(), "local _ = math.type(1.5)".into())),
    ("math.ult", |_, _| (String::new(), "local _ = math.ult(-1, 1)".into())),
    // Lua 5.3 compatibility (on unless `strict_5_4`).
    ("math.atan2", |_, _| (String::new(), "local _ = math.atan2(1, 2)".into())),
    ("math.cosh", |_, _| (String::new(), "local _ = math.cosh(700)".into())),
    ("math.sinh", |_, _| (String::new(), "local _ = math.sinh(-700)".into())),
    ("math.tanh", |_, _| (String::new(), "local _ = math.tanh(0.5)".into())),
    ("math.pow", |_, _| (String::new(), "local _ = math.pow(2, 0.5), math.pow(-8, 1 / 3)".into())),
    ("math.frexp", |_, _| (String::new(), "local _ = math.frexp(5e-324), math.frexp(-1e300)".into())),
    ("math.ldexp", |_, r| {
        (
            String::new(),
            r.pick(&[
                "local _ = math.ldexp(1, 2^31 - 1)",
                "local _ = math.ldexp(5e-324, 2000)",
                "local _ = math.ldexp(1, -2^40)",
            ])
            .into(),
        )
    }),
    ("math.log10", |_, _| (String::new(), "local _ = math.log10(1e300)".into())),
    // ---- utf8
    ("utf8.char", |n, r| {
        // Sometimes the last code point is bad: the error comes after the work (S5SEC-5).
        let bad = if r.below(2) == 0 { " t[#t + 1] = -1" } else { "" };
        (
            format!("{} for i = 1, #t do t[i] = (i * 7919) % 0x10FFFF end{bad}", rand_tab(n.min(4000), r)),
            "pcall(utf8.char, table.unpack(t))".into(),
        )
    }),
    ("utf8.codepoint", |n, r| (s_and(n, r, ""), "pcall(utf8.codepoint, s, 1, -1)".into())),
    ("utf8.codes", |n, r| (s_and(n, r, ""), "pcall(function() for _ in utf8.codes(s) do end end)".into())),
    ("utf8.len", |n, r| {
        (s_and(n, r, ""), format!("local _ = utf8.len(s, {}, -1, true)", 1 + r.below(n.max(1))))
    }),
    ("utf8.offset", |n, r| {
        (
            s_and(n, r, ""),
            format!(
                "pcall(utf8.offset, s, {}, {})",
                r.below(2 * n + 2) as i64 - n as i64 - 1,
                r.pick(&["nil", "-1", "1"])
            ),
        )
    }),
    // ---- io (the families' states have files in memory and an endless stdin: `host_env`)
    ("io.close", |_, _| (String::new(), "io.close(io.open('small'))".into())),
    ("io.flush", |_, _| (String::new(), "io.flush()".into())),
    ("io.input", |_, r| (String::new(), r.pick(&["io.input(io.stdin)", "io.input('small')", "pcall(io.input, 'missing')"]).into())),
    ("io.lines", |n, r| {
        (
            format!("name = string.rep('n', {})", n.min(1 << 20)),
            r.pick(&["for l in io.lines('big') do end", "for a, b in io.lines('big', 'n', 'l') do end", "pcall(io.lines, name)", "for l in io.lines('small', 'L') do end"]).into(),
        )
    }),
    ("io.open", |n, r| {
        (
            format!("name = string.rep('n', {})", n.min(1 << 20)),
            r.pick(&["local f = io.open('big') f:close()", "local f = io.open(name)", "local f = io.open('big', 'rb') f:close()", "pcall(io.open, 'big', 'rw')"]).into(),
        )
    }),
    ("io.output", |_, r| (String::new(), r.pick(&["io.output(io.stdout)", "io.output('out') io.output():close() io.output(io.stdout)"]).into())),
    ("io.read", |n, r| {
        (
            format!("io.input('big') k = {}", n.min(1 << 20)),
            r.pick(&[
                "if not io.read('l') then io.input('big') end",
                "if not io.read('n') then io.read('l') end if not io.read('l') then io.input('big') end",
                "if io.read(k) == nil then io.input('big') end",
                "io.read('a') io.input('big')",
                "local _ = io.stdin:read(k)",
                "local _ = io.stdin:read('L')",
            ])
            .into(),
        )
    }),
    ("io.type", |_, r| (String::new(), format!("local _ = io.type({})", r.pick(&["io.stdout", "1", "{}", "nil"])))),
    ("io.write", |n, r| {
        (
            format!("s = string.rep('w', {}) f = io.open('out', 'w')", n.min(1 << 20)),
            r.pick(&["io.write(s)", "io.write(1.5, 2, s)", "f:write(s) f:seek('set')", "io.stderr:write(s, 3)"]).into(),
        )
    }),
    // ---- os
    ("os.clock", |_, _| (String::new(), "local _ = os.clock()".into())),
    ("os.date", |n, r| {
        (
            format!(
                "f = string.rep('{}', {})",
                r.pick(&["%c", "%Y-%m-%d", "x", "%%", "\\xff"]),
                n.min(1 << 18)
            ),
            "pcall(os.date, f, 0)".into(),
        )
    }),
    ("os.difftime", |_, _| (String::new(), "local _ = os.difftime(math.maxinteger, math.mininteger)".into())),
    ("os.getenv", |n, r| {
        (
            format!("n = string.rep('A', {})", n.min(1 << 22)),
            r.pick(&["local _ = os.getenv(n)", "local _ = os.getenv('BIG')", "local _ = os.getenv('A')", "local _ = os.getenv('none')"]).into(),
        )
    }),
    ("os.setlocale", |_, r| {
        (String::new(), format!("local _ = os.setlocale({}, {})", r.pick(&["nil", "'C'", "''", "'xx'"]), r.pick(&["nil", "'all'", "'numeric'", "'time'"])))
    }),
    ("os.time", |_, _| {
        (String::new(), "local _ = os.time(), os.time({year = 2026, month = 1, day = 1})".into())
    }),
    // ---- coroutine
    ("coroutine.close", |_, _| {
        (String::new(), "local co = coroutine.create(function() coroutine.yield() end) coroutine.resume(co) coroutine.close(co)".into())
    }),
    ("coroutine.create", |_, _| (String::new(), "local _ = coroutine.create(print)".into())),
    ("coroutine.isyieldable", |_, _| (String::new(), "local _ = coroutine.isyieldable()".into())),
    ("coroutine.resume", |n, r| {
        (
            rand_tab(n.min(4000), r),
            "local co = coroutine.create(function(...) return ... end) coroutine.resume(co, table.unpack(t))"
                .into(),
        )
    }),
    ("coroutine.running", |_, _| (String::new(), "local _ = coroutine.running()".into())),
    ("coroutine.status", |_, _| {
        ("co = coroutine.create(print)".into(), "local _ = coroutine.status(co)".into())
    }),
    ("coroutine.wrap", |_, _| {
        (
            String::new(),
            "local f = coroutine.wrap(function() for i = 1, 3 do coroutine.yield(i) end end) f() f()".into(),
        )
    }),
    ("coroutine.yield", |n, r| {
        (
            rand_tab(n.min(4000), r),
            "local f = coroutine.wrap(function() coroutine.yield(table.unpack(t)) end) f()".into(),
        )
    }),
    // ---- opcode families
    ("op: arith", |_, _| ("a, b, c = 7, 2.5, 3".into(), "local x = a + b * c - a // c % b ^ 2 / c".into())),
    ("op: bitwise", |_, _| ("a, b = 0xff, 3".into(), "local x = (a & b) | (a ~ b) << 2 >> 1 ~ ~a".into())),
    ("op: compare", |n, r| {
        (s_and(n, r, ""), r.pick(&["local x = s < s2", "local x = s == s2", "local x = 1 < 2.5"]).into())
    }),
    ("op: concat", |n, r| (s_and(n.min(1 << 16), r, ""), "local x = s .. s2 .. 1 .. 2.5".into())),
    ("op: len", |n, r| {
        let t = rand_tab(n, r);
        (s_and(n, r, &t), r.pick(&["local x = #s", "local x = #t"]).into())
    }),
    ("op: table get/set", |n, r| {
        (rand_tab(n, r), format!("t[{}] = 1 local x = t.k1, t[1], t[{}]", r.below(n + 2), r.below(n + 2)))
    }),
    ("op: new table", |n, _| (String::new(), format!("local x = {{1, 2, 3, a = 1, [{n}] = 2}}"))),
    ("op: closure", |_, _| ("local u = 1".into(), "local f = function() return u end".into())),
    ("op: call", |n, r| {
        (
            format!("{} function f(...) return select('#', ...) end", rand_tab(n.min(4000), r)),
            "f(table.unpack(t))".into(),
        )
    }),
    ("op: for loops", |n, _| {
        (String::new(), format!("for i = 1, {} do end for i = 1.0, 2.0, 0.5 do end", n.min(1 << 16)))
    }),
    ("op: __index", |n, _| {
        (
            format!(
                "a = {{}} for i = 1, {} do a = setmetatable({{}}, {{__index = a}}) end f = setmetatable({{}}, {{__index = function(t, k) return k end}})",
                n.min(1900)
            ),
            "local x = a.missing, f[1]".into(),
        )
    }),
    ("op: __newindex", |_, _| {
        (
            "t = setmetatable({}, {__newindex = function(t, k, v) rawset(t, k, v) end})".into(),
            "t[1] = 1 t[1] = nil".into(),
        )
    }),
    ("op: __call", |_, _| {
        ("c = setmetatable({}, {__call = function(self, x) return x end})".into(), "local x = c(1)".into())
    }),
    ("op: __eq __lt __le", |_, _| {
        ("local m = {__eq = function() return true end, __lt = function() return true end, __le = function() return false end} a, b = setmetatable({}, m), setmetatable({}, m)".into(), "local x = a == b, a < b, a <= b".into())
    }),
    ("op: __concat __len __unm", |_, _| {
        ("local m = {__concat = function() return 'c' end, __len = function() return 1 end, __unm = function() return 0 end} a = setmetatable({}, m)".into(), "local x = a .. 'x', #a, -a".into())
    }),
    ("op: __close", |_, _| {
        ("m = {__close = function() end}".into(), "do local x <close> = setmetatable({}, m) end".into())
    }),
    ("op: runtime errors", |n, r| {
        (
            s_and(n, r, ""),
            r.pick(&[
                "pcall(function() return s() end)",
                "pcall(function() return {} + s end)",
                "pcall(function() local x = nil; x[s] = 1 end)",
                "pcall(function() return 1 < s end)",
            ])
            .into(),
        )
    }),
];

/// The fuel-against-time bound: a call may cost at most this many times the median of
/// plain operations, per unit of fuel (calibrated maximum is about 3.5x; knob
/// `SWEEP3_PROP_BOUND`).
fn prop_bound() -> f64 {
    std::env::var("SWEEP3_PROP_BOUND").ok().and_then(|s| s.parse().ok()).unwrap_or(10.0)
}

/// Runs `body` in a loop until `fuel` is spent: the thread's CPU time per unit of fuel used
/// (`remedy_lua_oracle::thread_cpu`, in nanoseconds). Not wall time, which
/// counts other processes' load as ours. Only ratios to the median are compared.
/// The libraries of the families' states: every one, the opt-in ones included.
const FAMILY_LIBS: Libs = super::common::ALL_OPT_IN.union(Libs::ENV).union(Libs::IO);

/// What `os.getenv` finds in the families' states: a short value and a 1 MiB one.
fn host_env(lua: &mut Lua) {
    lua.set_env("A", Some(b"1"));
    lua.set_env("BIG", Some(&[b'x'; 1 << 20]));
    // `io`: files in memory (`big`, 1 MiB of lines and numbers; `small`), at most 64 MiB
    // written, and a stdin that never ends.
    let fs = super::common::MemoryFiles::limited(64 << 20);
    let mut big = Vec::new();
    while big.len() < 1 << 20 {
        big.extend_from_slice(b"12345 0x1F -3.5e2 a line of text\n");
    }
    fs.insert("big", big);
    fs.insert("small", "x\n");
    lua.set_file_opener(Some(move |n: &str, m: &str| fs.open(n, m)));
    struct Endless;
    impl remedy_lua::Stream for Endless {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, remedy_lua::IoError> {
            for (i, b) in buf.iter_mut().enumerate() {
                *b = if i % 64 == 63 { b'\n' } else { b'7' };
            }
            Ok(buf.len())
        }
    }
    lua.set_stdin(Some(Box::new(Endless)));
}

/// The module the host serves for `require('hostmod')` in the families' states.
const HOST_MODULE: &str = "local M = {} for i = 1, 50 do M['f' .. i] = function() return i end end return M";

fn ns_per_fuel(setup: &str, body: &str, fuel: u64) -> Result<f64, String> {
    let mut lua = Lua::new(
        Limits::default().memory(512 << 20).string_len(64 << 20).fuel(1 << 40).hash_seed(1),
        FAMILY_LIBS,
    );
    lua.add_module("hostmod", HOST_MODULE);
    host_env(&mut lua);
    lua.set_print(|_| {});
    lua.exec(setup, "=setup").map_err(|e| format!("setup: {e:?}"))?;
    lua.set_limits(lua.limits().fuel(fuel));
    let f = lua.load(format!("while true do {body} end"), "=body").map_err(|e| format!("body: {e:?}"))?;
    let start = remedy_lua_oracle::thread_cpu();
    let r = lua.call(f.value(), &[]);
    let t = remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64;
    match r {
        Err(Error::Limit(LimitKind::Fuel)) => Ok(t / fuel as f64),
        // Out of memory or string length: what was done is still bounded by fuel used.
        Err(Error::Limit(LimitKind::Memory)) => Ok(t / (fuel - lua.fuel_left()).max(1) as f64),
        other => Err(format!("ended with {other:?}")),
    }
}

/// The median of three measurements (Idle priority makes single runs noisy).
fn median3(setup: &str, body: &str, fuel: u64) -> Result<f64, String> {
    let mut v =
        [ns_per_fuel(setup, body, fuel)?, ns_per_fuel(setup, body, fuel)?, ns_per_fuel(setup, body, fuel)?];
    v.sort_by(f64::total_cmp);
    Ok(v[1])
}

/// The interrupt (or the deadline) stops `body` within `max` while it runs.
fn stop_latency(setup: &str, body: &str, use_clock: bool) -> Result<Duration, String> {
    let mut lua = Lua::new(Limits::unmetered().memory(512 << 20).string_len(64 << 20), FAMILY_LIBS);
    lua.add_module("hostmod", HOST_MODULE);
    host_env(&mut lua);
    lua.set_print(|_| {});
    lua.exec(setup, "=setup").map_err(|e| format!("setup: {e:?}"))?;
    let f = lua.load(format!("while true do {body} end"), "=body").map_err(|e| format!("body: {e:?}"))?;
    let after = Duration::from_millis(20);
    let start = Instant::now();
    let (r, late) = if use_clock {
        lua.set_time_limit(Some((std::sync::Arc::new(move || start.elapsed()), after)));
        let r = lua.call(f.value(), &[]);
        (r, start.elapsed().saturating_sub(after))
    } else {
        let h = lua.interrupt_handle();
        let t = std::thread::spawn(move || {
            std::thread::sleep(after);
            h.interrupt();
            Instant::now()
        });
        let r = lua.call(f.value(), &[]);
        let end = Instant::now();
        (r, end.saturating_duration_since(t.join().unwrap()))
    };
    let want = if use_clock { LimitKind::Time } else { LimitKind::Interrupted };
    match r {
        Err(Error::Limit(k)) if k == want || k == LimitKind::Memory => Ok(late),
        other => Err(format!("ended with {other:?}")),
    }
}

/// The property: every library function and opcode family, with random arguments from tiny
/// to large, costs at most `prop_bound()` times the median of plain operations per unit of
/// fuel (so cost cannot grow faster than fuel with input size), and the interrupt and the
/// deadline stop each one promptly. A new library function without a family here fails the
/// coverage check. Quick by default; long run:
/// `SWEEP3_PROP_LONG=1 SWEEP3_PROP_SEED=7 cargo test -p remedy-lua --test all sweep3_sec::fuel_tracks_time -- --nocapture`
#[test]
fn fuel_tracks_time_for_every_function() {
    let _heavy = super::common::memory_heavy();
    // On a 64 KiB stack: data-driven native recursion overflows here, not in production.
    super::common::small_stack(fuel_tracks_time_body);
}

fn fuel_tracks_time_body() {
    let long = std::env::var("SWEEP3_PROP_LONG").is_ok();
    let seed: u64 = std::env::var("SWEEP3_PROP_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x9e37);
    let only = std::env::var("SWEEP3_PROP_ONLY").ok();
    // Coverage: every function the libraries expose has a family: in the globals, in
    // every table among them (a new library, not only the known ones: S5SEC-5), and in the
    // string metatable.
    let mut lua = Lua::new(Limits::default(), FAMILY_LIBS);
    let names = lua
        .exec(
            "local out = {} local function scan(prefix, t) for k, v in pairs(t) do \
             if type(v) == 'function' then out[#out + 1] = prefix .. tostring(k) end end end \
             scan('', _G) for k, v in pairs(_G) do if type(v) == 'table' and v ~= _G then scan(k .. '.', v) end end \
             for k, v in pairs(getmetatable('')) do if type(v) == 'function' then out[#out + 1] = 'string-meta.' .. k \
             elseif type(v) == 'table' and v ~= string then scan('string-meta.' .. k .. '.', v) end end \
             return table.concat(out, ' ')",
            "=names",
        )
        .unwrap();
    let names = lua.to_string_lossy(names[0]).unwrap();
    // The string metatable's arithmetic metamethods are the "string arithmetic" family.
    let family_of = |n: &str| match n.strip_prefix("string-meta.") {
        Some("__add" | "__sub" | "__mul" | "__div" | "__mod" | "__pow" | "__unm" | "__idiv") => "string arithmetic".to_owned(),
        _ => n.to_owned(),
    };
    let missing: Vec<&str> =
        names.split(' ').filter(|n| !FAMILIES.iter().any(|(f, _)| *f == family_of(n))).collect();
    assert!(missing.is_empty(), "library functions with no fuel-against-time family: {missing:?}");

    let (fuel, sizes, rounds): (u64, &[usize], usize) = if long {
        (2_000_000, &[1, 16, 1 << 8, 1 << 12, 1 << 16, 1 << 20], 4)
    } else {
        // The largest size twice a round: the bugs show there, and each draw picks one of
        // a family's argument shapes.
        (80_000, &[1, 1 << 14, 1 << 7, 1 << 14], 6)
    };
    // The quick run draws from several seeds: the fixed one and, unless a seed is given,
    // two fresh ones per run, so each run covers new argument picks (one seed alone caught
    // a planted bug in only 1 of 3 tries). Every seed is in the failure message, to replay
    // one with `SWEEP3_PROP_SEED`.
    let fresh = || {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64)
    };
    let seeds: Vec<u64> = if long || std::env::var("SWEEP3_PROP_SEED").is_ok() {
        vec![seed]
    } else {
        vec![seed, fresh() ^ 0xa5a5, fresh().rotate_left(17)]
    };
    let mut rngs: Vec<Rng> = seeds.iter().map(|s| Rng(s | 1)).collect();
    let mut bad = Vec::new();
    // Every family at every size: one run each (the median is taken over all of them, so
    // the bound follows the machine and the build).
    let mut runs: Vec<(String, String, String, f64, bool)> = Vec::new();
    for (name, make) in FAMILIES {
        // `SWEEP3_PROP_ONLY` checks some families; all are still measured for the median.
        let check = only.as_ref().is_none_or(|o| name.contains(o.as_str()));
        for round in 0..rounds {
            let r = &mut rngs[round % seeds.len()];
            for (si, &n) in sizes.iter().enumerate() {
                let (setup, body) = make(n, r);
                let label = format!("{name} n={n}: {setup} || {body}");
                match ns_per_fuel(&setup, &body, fuel) {
                    Ok(ns) => runs.push((label.clone(), setup.clone(), body.clone(), ns, check)),
                    Err(e) => bad.push(format!("{label}: {e}")),
                }
                // Latency at the largest size, by interrupt and by deadline.
                if check && si + 1 == sizes.len() && (long || round == 0) {
                    let max_late = Duration::from_millis(if long { 50 } else { 100 });
                    for use_clock in [round % 2 == 0, round % 2 == 1] {
                        match stop_latency(&setup, &body, use_clock) {
                            Ok(late) if late <= max_late => {}
                            Ok(late) => {
                                bad.push(format!("{label}: stopped {late:?} late (clock {use_clock})"))
                            }
                            Err(e) => bad.push(format!("{label}: latency run {e}")),
                        }
                    }
                }
            }
        }
    }
    let mut all: Vec<f64> = runs.iter().map(|x| x.3).collect();
    all.sort_by(f64::total_cmp);
    let median = all.get(all.len() / 2).copied().unwrap_or(1.0);
    let bound = prop_bound() * median;
    for (label, setup, body, ns, _) in runs.iter().filter(|x| x.4) {
        // A slow one is measured again as the median of three (Idle priority is noisy).
        let ns = if *ns > bound { median3(setup, body, fuel).unwrap_or(*ns) } else { *ns };
        if long || only.is_some() {
            println!("{:7.1} cpu/unit ({:4.1}x) {}", ns, ns / median, &label[..label.len().min(160)]);
        }
        if ns > bound {
            bad.push(format!(
                "{label}: {ns:.0} cpu per fuel unit, {:.1}x the median {median:.1}",
                ns / median
            ));
        }
    }
    assert!(bad.is_empty(), "seeds {seeds:?}, median {median:.1} cpu per unit, bound {bound:.1}:\n{}", bad.join("\n"));
}

/// LR-29: one `gsub` match over 2 MiB whose replacement expands to 128 MiB (64 `%0`) was
/// copied whole before the string limit was checked, and with no look at the stop: the
/// deadline and the interrupt got through 62 to 68 ms late. The copies now go a piece at a
/// time between looks, and none past the limit. The same shape for `table.concat` (whose
/// 16 MiB elements were each copied in one go, up to 20 ms late: now a piece at a time,
/// each paid for as it is copied), `string.format('%s')` and `string.rep` of huge strings.
/// Fuel is the same on both paths.
/// LR-29, `table.concat`: each long element is copied a megabyte at a time with a look at
/// the stop between pieces (before: one look per element, 9 for this join).
#[test]
fn lr29_concat_looks_at_the_stop_between_pieces() {
    // A clock that counts its reads, and a deadline that never comes.
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let r2 = std::sync::Arc::clone(&reads);
    let mut lua = Lua::new(Limits::unmetered().memory(512 << 20).string_len(256 << 20), FAMILY_LIBS);
    lua.exec("big = string.rep('x', 16 << 20) T = {} for i = 1, 8 do T[i] = big end", "=setup").unwrap();
    lua.set_time_limit(Some((
        std::sync::Arc::new(move || {
            r2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Duration::ZERO
        }),
        Duration::from_secs(3600),
    )));
    reads.store(0, std::sync::atomic::Ordering::Relaxed);
    lua.exec("local s = table.concat(T) assert(#s == 128 << 20)", "=t").unwrap();
    let n = reads.load(std::sync::atomic::Ordering::Relaxed);
    assert!(n >= 8 * 15, "a join of 8 x 16 MiB read the clock {n} times (a look per MiB copied)");
}

/// LR-29, a library buffer growing: one of 1 to 32 MiB is copied into the new one a piece at
/// a time between looks (`format('%s%s')` of 16 MiB strings grew its 16 MiB buffer in one
/// `realloc`, which copies one served from the heap: 8 to 9.5 ms with no look, 25 ms late
/// once in a loaded full run).
#[test]
fn lr29_a_growing_buffer_looks_at_the_stop_between_pieces() {
    let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let r2 = std::sync::Arc::clone(&reads);
    let mut lua = Lua::new(Limits::unmetered().memory(512 << 20).string_len(256 << 20), FAMILY_LIBS);
    lua.exec("big = string.rep('x', 16 << 20)", "=setup").unwrap();
    lua.set_time_limit(Some((
        std::sync::Arc::new(move || {
            r2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Duration::ZERO
        }),
        Duration::from_secs(3600),
    )));
    reads.store(0, std::sync::atomic::Ordering::Relaxed);
    lua.exec("local s = string.format('%s%s', big, big) assert(#s == 32 << 20)", "=t").unwrap();
    let n = reads.load(std::sync::atomic::Ordering::Relaxed);
    // Two copies of 16 MiB and the buffer's growth past the first.
    assert!(n >= 3 * 15, "32 MiB formatted read the clock {n} times (a look per MiB copied)");
}

#[test]
fn lr29_huge_replacements_and_joins_stop_promptly() {
    // Wall-clock latency: measured alone (`measuring`), not beside the other heavy tests,
    // whose load made it 29 ms late once against its 25 ms bound in a full run (passing
    // 5 of 5 alone).
    let _alone = super::common::measuring();
    let setup = "s = string.rep('ab', 1048576) big = string.rep('x', 16 << 20) T = {} for i = 1, 8 do T[i] = big end";
    for body in [
        "pcall(string.gsub, s, '%f[%w]%w+', string.rep('%0', 64))",
        "pcall(table.concat, T)",
        "pcall(string.format, string.rep('%s', 8), big, big, big, big, big, big, big, big)",
        "pcall(string.rep, big, 8)",
    ] {
        for use_clock in [true, false] {
            let late = stop_latency(setup, body, use_clock).unwrap_or_else(|e| panic!("{body}: {e}"));
            assert!(late < Duration::from_millis(50), "{body} (deadline {use_clock}): {late:?} late");
            // That one call, stopped 1, 2, 4 ... 64 ms into it (so one stop lands in each of
            // its stretches: the match, then the copies): each within 25 ms, not a seed that
            // happens to land well.
            for ms in [1, 2, 4, 8, 16, 32, 64] {
                let mut lua = Lua::new(Limits::unmetered().memory(512 << 20).string_len(64 << 20), FAMILY_LIBS);
                lua.exec(setup, "=setup").unwrap();
                let f = lua.load(body, "=one").unwrap();
                let after = Duration::from_millis(ms);
                let start = Instant::now();
                let stopper = if use_clock {
                    lua.set_time_limit(Some((std::sync::Arc::new(move || start.elapsed()), after)));
                    None
                } else {
                    let h = lua.interrupt_handle();
                    Some(std::thread::spawn(move || {
                        std::thread::sleep(after);
                        h.interrupt();
                        Instant::now()
                    }))
                };
                let _ = lua.call(f.value(), &[]);
                let end = Instant::now();
                // From the stop itself: a stopper thread that wakes late is the scheduler's.
                let late = match stopper {
                    Some(t) => end.saturating_duration_since(t.join().unwrap()),
                    None => end.saturating_duration_since(start).saturating_sub(after),
                };
                assert!(late < Duration::from_millis(25), "{body} (deadline {use_clock}), one call stopped at {ms} ms: {late:?} late");
            }
        }
        let fuel = |fast: bool| {
            let mut lua = Lua::new(Limits::default().memory(512 << 20).string_len(64 << 20).fuel(1 << 40), FAMILY_LIBS);
            lua.set_fast_loop(fast);
            let r = lua.exec(format!("{setup} {body}"), "=t").map(|_| ()).map_err(|e| e.to_string());
            (r, lua.fuel_left())
        };
        assert_eq!(fuel(true), fuel(false), "{body}");
    }
}
