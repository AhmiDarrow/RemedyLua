//! Sweep 2, area "security and sandbox": the new fast paths (`fast.rs`), long-string keys,
//! the pattern functions, wide frames and the iterative front end, attacked as a hostile
//! script and a careless host. Every test is a finding fixed in this sweep (its id in the
//! comment) or an attack that is stopped correctly.
//! Programs run with the fast loop on and off: outcome, fuel and memory must agree.
//!
//! `hostile_fuzz_fast_vs_full`: random hostile programs over the new paths, fast against
//! full, under small fuel and memory caps and with the watchdog. Long run:
//! `SWEEP2_SEC_ITERS=20000 SWEEP2_SEC_SEED=7 cargo test -p remedy-lua --test all sweep2_sec::hostile -- --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant};

use remedy_lua::{Error, Libs, Limits, Lua, Value};

// ---------------------------------------------------------------- helpers

/// Runs `f` on a worker thread; `Err` when it panicked or did not finish in `secs`.
fn bounded<T: Send + 'static>(
    secs: u64,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<(T, Duration), String> {
    // (Never detached: `common::with_deadline`.)
    let r = super::common::with_deadline(16 << 20, Duration::from_secs(secs), move || {
        let start = Instant::now();
        let r = std::panic::catch_unwind(AssertUnwindSafe(f));
        r.map(|v| (v, start.elapsed()))
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

fn describe(lua: &mut Lua, src: &str) -> String {
    match lua.exec(src, "=t") {
        Ok(v) => format!("ok {}", v.iter().map(|x| show(lua, *x)).collect::<Vec<_>>().join(",")),
        Err(Error::Limit(k)) => format!("limit {k:?}"),
        Err(e) => format!("error {}", e.message()),
    }
}

fn show(lua: &Lua, v: Value) -> String {
    match v {
        Value::Str(_) => lua.to_string_lossy(v).unwrap_or_default(),
        Value::Table(_) | Value::Func(_) | Value::Thread(_) | Value::UserData(_) => v.type_name().to_owned(),
        other => format!("{other:?}"),
    }
}

/// `limits` with the fixed test seed unless one is set: the fast and full runs must see
/// the same `pairs` order.
fn seeded(limits: Limits) -> Limits {
    if limits.hash_seed.is_some() { limits } else { limits.hash_seed(super::common::TEST_SEED) }
}

/// (outcome, fuel used, memory held) in a fresh state.
fn measured(src: &str, limits: Limits, fast: bool) -> (String, u64, usize) {
    let mut lua = Lua::new(seeded(limits), Libs::ALL);
    lua.set_fast_loop(fast);
    let r = describe(&mut lua, src);
    (r, limits.fuel.saturating_sub(lua.fuel_left()), lua.memory_used())
}

/// Runs `src` with the fast loop and without, under the watchdog: both must end within
/// `secs` and agree on outcome, fuel and memory. The outcome and fuel used.
fn both(src: &str, limits: Limits, secs: u64) -> (String, u64) {
    let s = src.to_owned();
    let (r, _) = bounded(secs, move || (measured(&s, limits, true), measured(&s, limits, false)))
        .unwrap_or_else(|e| panic!("{e}: {}", &src[..src.len().min(400)]));
    assert_eq!(r.0, r.1, "fast loop and full path differ (outcome, fuel, memory):\n{src}");
    (r.0.0, r.0.1)
}

fn fuel(n: u64) -> Limits {
    Limits::default().fuel(n)
}

// ---------------------------------------------------------------- probing

/// Exploration helper: `SWEEP2_PROBE=<file>` holds programs separated by lines `----`
/// (`SWEEP2_FUEL`, `SWEEP2_MEM`); each runs fast and full, timed.
#[test]
#[ignore = "helper: manual probing"]
fn probe() {
    let Ok(path) = std::env::var("SWEEP2_PROBE") else { return };
    let text = std::fs::read_to_string(path).unwrap();
    let f = std::env::var("SWEEP2_FUEL").ok().and_then(|s| s.parse().ok()).unwrap_or(10_000_000);
    let m = std::env::var("SWEEP2_MEM").ok().and_then(|s| s.parse().ok()).unwrap_or(64 << 20);
    for src in text.split("\n----\n") {
        let limits = Limits::default().fuel(f).memory(m);
        let s = src.to_owned();
        let r = bounded(60, move || {
            let t = Instant::now();
            let a = measured(&s, limits, true);
            let ta = t.elapsed();
            let b = measured(&s, limits, false);
            (a, ta, b, t.elapsed() - ta)
        });
        let head: String = src.trim().chars().take(70).collect();
        match r {
            Ok(((a, ta, b, tb), _)) => {
                let same = if a == b { "same" } else { "DIFFER" };
                println!("PROBE [{head}]\n   fast {ta:?} {a:?}\n   full {tb:?} {b:?} {same}");
            }
            Err(e) => println!("PROBE [{head}]\n   {e}"),
        }
    }
}

// ---------------------------------------------------------------- hostile fuzz

/// xorshift64*.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

const KEYS: &[&str] = &[
    "1",
    "2",
    "3",
    "'a'",
    "'b'",
    "'__index'",
    "ls",
    "ls2",
    "2.0",
    "-0.0",
    "x",
    "t",
    "true",
    "#t + 1",
    "k",
    "'k' .. x % 7",
    "mm",
];
const VALS: &[&str] = &[
    "1",
    "x",
    "nil",
    "'v'",
    "ls",
    "t",
    "u",
    "f",
    "2.5",
    "{}",
    "{1, 2}",
    "false",
    "s",
    "math.maxinteger",
    "mt",
];

fn stmt(r: &mut Rng, depth: usize) -> String {
    let k = r.pick(KEYS);
    let v = r.pick(VALS);
    let tab = r.pick(&["t", "u", "mt", "o"]);
    match r.below(44) {
        0 => format!("{tab}[{k}] = {v}"),
        1 => format!("{tab}.a = {v}"),
        2 => format!("x = {tab}[{k}]"),
        3 => format!("x = {tab}.a"),
        4 => format!("{tab}[#{tab} + 1] = {v}"),
        5 => format!("x = #{tab}"),
        6 => format!("for k2, v2 in pairs({tab}) do n = n + 1 if n % 3 == 0 then {tab}[k2] = nil end end"),
        7 => format!("for i2, v2 in ipairs({tab}) do n = n + i2 end"),
        8 => format!("for k2, v2 in next, {tab}, {k} do n = n + 1 if n > 50 then break end end"),
        9 => format!("setmetatable({tab}, mt)"),
        10 => format!("setmetatable({tab}, nil)"),
        11 => "mt.__index = u".into(),
        12 => "mt.__index = function(_, kk) n = n + 1 return kk end".into(),
        13 => "mt.__newindex = u".into(),
        14 => "mt.__newindex = function(tt, kk, vv) rawset(tt, kk, vv) end".into(),
        15 => "mt.__newindex = nil mt.__index = nil".into(),
        16 => "mt.__len = function() return 3 end".into(),
        17 => "mt.__eq = function() return true end".into(),
        18 => format!("s = s .. {v}"),
        19 => "x = (s == ls) or (ls == ls2) or (ls < ls2)".into(),
        20 => format!("ls = string.rep('ab', {}) ls2 = ls:sub(1)", 20 + r.below(60)),
        21 => "x = o:m(x)".into(),
        22 => format!("x = select('#', s:gsub('{}', {}))", r.pick(&["a", "%w+", "(.)", ".-", "b*"]), r.pick(&["'%0%0'", "u", "f", "'z'"])),
        23 => format!("for w in s:gmatch('{}') do n = n + 1 end", r.pick(&["a", "%a+", "()", "(.)(.)"])),
        24 => "x = string.format('%s %d %q', s, n, ls)".into(),
        25 => r.pick(&["collectgarbage()", "collectgarbage('step')", "collectgarbage('step', 100)"]).into(),
        26 => "co = coroutine.wrap(function() for k2, v2 in pairs(t) do coroutine.yield(k2) end for i2 in ipairs(u) do coroutine.yield(i2) end end) for i3 = 1, 4 do x = co() end".into(),
        27 => format!("{tab} = {}", r.pick(&["{}", "{1, 2, 3, a = 1}", "setmetatable({}, mt)", "{[ls] = 1, [ls2] = 2}"])),
        28 => format!("table.insert({tab}, {v})"),
        29 => format!("x = table.remove({tab})"),
        30 => format!("x = table.concat({tab}, ',')"),
        31 => "x = f(x, n)".into(),
        32 => format!("x = rawequal({k}, {v}) x = rawlen(t) x = rawget(t, {k})"),
        33 => format!("o.m = function(self, a) return a end o.{} = 1", r.pick(&["a", "b", "m"])),
        34 => "o = setmetatable({}, {__index = u}) u.m = function(self, a) return a end".into(),
        35 => format!("local t9 = {{}} for i9 = 1, {} do t9[i9] = i9 t9['k' .. i9] = i9 end t = t9", r.below(200)),
        36 => format!("for i9 = 1, {} do t[i9] = nil end", r.below(50)),
        37 => "x = coroutine.wrap(function(...) return select('#', ...) end)(table.unpack(t))".into(),
        38 => "error(x)".into(),
        39 => format!("k = {k} mm = {v}"),
        40 => format!("x = t[{k}] == u[{k}]"),
        41 if depth < 3 => format!("x = pcall(function() {} end)", block(r, depth + 1, 3)),
        42 if depth < 3 => format!("for i8 = 1, {} do {} end", r.below(20), block(r, depth + 1, 3)),
        43 if depth < 3 => format!("if x then {} else {} end", block(r, depth + 1, 2), block(r, depth + 1, 2)),
        _ => format!("n = n + {}", r.below(9)),
    }
}

fn block(r: &mut Rng, depth: usize, max: usize) -> String {
    (0..1 + r.below(max)).map(|_| stmt(r, depth)).collect::<Vec<_>>().join("\n")
}

fn program(r: &mut Rng) -> String {
    let wide = if r.below(4) == 0 {
        format!("local {}\n", (0..300).map(|i| format!("w{i}")).collect::<Vec<_>>().join(","))
    } else {
        String::new()
    };
    format!(
        "{wide}local t, u, mt, o = {{1, 2, a = 1}}, {{3, 4}}, {{}}, {{}}\n\
         local x, n, s, k, mm, co = 0, 0, 'abc', 1, 1, nil\n\
         local ls, ls2 = string.rep('ab', 30), string.rep('ab', 30)\n\
         local function f(a, ...) return a, ... end\n\
         for iter = 1, 3 do\n{}\nend\nreturn n, type(x), #s",
        (0..5 + r.below(25))
            .map(|_| {
                let st = stmt(r, 0);
                if r.below(3) == 0 { st } else { format!("pcall(function() {st} end)") }
            })
            .collect::<Vec<_>>()
            .join("\n")
    )
}

/// One program, fast against full, under the given caps; then the same state must still
/// run code after whatever happened.
fn fuzz_one(src: &str, limits: Limits, stress: bool) -> Result<(), String> {
    let s = src.to_owned();
    let run = move |fast: bool| {
        let mut lua = Lua::new(seeded(limits), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.set_gc_stress(stress);
        let r = describe(&mut lua, &s);
        let used = (limits.fuel.saturating_sub(lua.fuel_left()), lua.memory_used());
        lua.set_limits(Limits::default().fuel(1_000_000));
        let after = describe(
            &mut lua,
            "local t = {} for i = 1, 100 do t[i] = i .. '' end return #t, #table.concat(t)",
        );
        (r, used, after)
    };
    let ((a, b), _) = bounded(30, move || super::common::small_stack(move || (run(true), run(false))))?;
    if a != b {
        return Err(format!("fast {a:?}\nfull {b:?}"));
    }
    if std::env::var_os("SWEEP2_SEC_SHOW").is_some() {
        println!("OUTCOME {}", a.0.chars().take(90).collect::<String>());
    }
    if a.2 != "ok Int(100),Int(192)" {
        return Err(format!("state unusable after {:?}: {}", a.0, a.2));
    }
    Ok(())
}

#[test]
fn hostile_fuzz_fast_vs_full() {
    let iters = std::env::var("SWEEP2_SEC_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let seed = std::env::var("SWEEP2_SEC_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0x5eed_2025_u64);
    let mut r = Rng(seed | 1);
    let mut fails = Vec::new();
    for i in 0..iters {
        let src = program(&mut r);
        let limits = match r.below(3) {
            0 => Limits::default().fuel(200_000),
            1 => Limits::default().fuel(100 + r.below(20_000) as u64),
            _ => Limits::default().fuel(200_000).memory(48_000 + r.below(200_000)),
        };
        let stress = r.below(16) == 0;
        if let Err(e) = fuzz_one(&src, limits, stress) {
            println!(
                "FAIL #{i} (fuel {}, memory {}, stress {stress}): {e}\n{src}\n",
                limits.fuel, limits.memory
            );
            fails.push(i);
            if fails.len() >= 3 {
                break;
            }
        }
    }
    assert!(fails.is_empty(), "hostile fuzz failures: {fails:?} (seed {seed})");
}

// ---------------------------------------------------------------- the state after a limit

/// A limit hit deep inside the new paths (a coroutine in the fast loop, a gsub callback,
/// a sort comparator, a wide frame, an `__index` chain) leaves a state that runs again:
/// the stuck coroutine is dead or resumable without a panic, globals intact.
#[test]
fn state_usable_after_limits_in_new_paths() {
    let wide = format!("local {}\n", (0..300).map(|i| format!("w{i}")).collect::<Vec<_>>().join(","));
    let attacks = [
        "co = coroutine.create(function() local t = {} for i = 1, 1e9 do t[i % 64] = {i} end end) return coroutine.resume(co)".to_owned(),
        "co = coroutine.create(function() return (string.rep('a', 1e5):gsub('a', function(c) local x = 0 while true do x = x + 1 end end)) end) return coroutine.resume(co)".to_owned(),
        "co = coroutine.create(function() local t = {} for i = 1, 200 do t[i] = i end table.sort(t, function(a, b) local s = '' while true do s = s .. 'x' end end) end) return coroutine.resume(co)".to_owned(),
        format!("co = coroutine.create(function() {wide} local s = 0 for k, v in pairs(_G) do s = s + 1 end while true do s = {{s}} end end) return coroutine.resume(co)"),
        "local A = {} A.__index = A local o = setmetatable({}, A) co = coroutine.create(function() while true do local x = o.missing end end) return coroutine.resume(co)".to_owned(),
        "co = coroutine.create(function() local function r(n) return r(n + 1) + 1 end return r(1) end) return coroutine.resume(co)".to_owned(),
    ];
    for fast in [true, false] {
        for a in &attacks {
            let src = a.clone();
            let (r, _) = bounded(60, move || {
                let mut lua = Lua::new(Limits::default().fuel(2_000_000).memory(4 << 20), Libs::ALL);
                lua.set_fast_loop(fast);
                let first = describe(&mut lua, &src);
                let mut out = vec![first];
                for _ in 0..3 {
                    out.push(describe(&mut lua, "local ok, e = pcall(coroutine.resume, co) return coroutine.status(co) .. ' ' .. tostring(ok)"));
                }
                // A dead coroutine keeps its stack, as in C Lua, and the heap may be too
                // full to compile anything: the host drops it.
                lua.set_global("co", Value::Nil).unwrap();
                lua.collect_garbage().unwrap();
                out.push(describe(&mut lua, "local t = {} for i = 1, 1000 do t[#t + 1] = i .. '' end return #t"));
                out
            })
            .unwrap_or_else(|e| panic!("{e}: {a}"));
            assert!(
                r[0].starts_with("limit") || r[0].starts_with("ok Bool(false)"),
                "fast {fast}: {a}: {r:?}"
            );
            assert_eq!(r[4], "ok Int(1000)", "fast {fast}: state unusable after {a}: {r:?}");
            for x in &r[1..4] {
                assert!(
                    x.starts_with("ok dead") || x.starts_with("ok suspended") || x.starts_with("limit"),
                    "fast {fast}: {a}: {r:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------- findings

// SEC2-1 (critical, fuel bypass): a long string that is not interned, used as a key whose
// interned copy is in the table, compared all its bytes with that copy on every lookup
// (`Heap::find_canon` through `retry_key`/`get_in`/`canon`), for one unit of fuel: with a
// 400 KB key, 3M fuel ran 13 s (C Lua does the same memcmp, but has no fuel to honour).
// Fix: the string remembers the interned copy it found (`LStr::twin`, checked live), so
// its bytes are compared once, like its hash is computed once.
#[test]
fn sec2_1_long_key_compared_once() {
    for body in [
        "local n = 0 while true do n = n + (t[s2] or 0) end",
        "local n = 0 while true do t[s2] = n n = n + 1 end",
        "local n = 0 while true do for k, v in next, t, s2 do n = n + 1 end end",
        "local n = 0 while true do n = n + (rawget(t, s2) or 0) end",
    ] {
        let src = format!(
            "local s = string.rep('x', 1e6) local t = {{[s] = 1, a = 2}} local s2 = string.rep('x', 1e6) {body}"
        );
        let (r, _) = both(&src, fuel(1_000_000), 8);
        assert_eq!(r, "limit Fuel", "{body}");
    }
    // Still found after the interned copy is collected and made again.
    let (r, _) = both(
        "local s2 = string.rep('y', 100) local t = {[string.rep('y', 100)] = 1} local a = t[s2] \
         t = nil collectgarbage() collectgarbage() t = {[string.rep('y', 100)] = 2} return a, t[s2]",
        fuel(1_000_000),
        10,
    );
    assert_eq!(r, "ok Int(1),Int(2)");
}

// SEC2-2 (major, fuel undercharged ~25-100x): the pattern functions charged a step of
// the matcher 1/64 of a unit of fuel and nothing per replacement made through a table or
// function, so `gsub` with a table burned 30M fuel in 72 s (2.4 us per unit; plain code
// is 2-25 ns) and `gmatch('%w+')` in 13 s. Fix: 4 steps per unit (`STEPS_PER_FUEL`) and
// 4 units per replacement through a table or function.
#[test]
fn sec2_2_patterns_charge_their_work() {
    let used = |body: &str| {
        both(&format!("local s = string.rep('a', 10000) {body} return 1"), fuel(50_000_000), 20).1
    };
    assert!(used("local x = s:gsub('a', {a = 'bb'})") >= 10_000 * 4, "gsub with a table");
    assert!(used("local x = s:gsub('a', function(c) return c end)") >= 10_000 * 4, "gsub with a function");
    assert!(used("local x = s:gsub('a', 'b')") >= 10_000 / 2, "gsub with a string");
    assert!(used("for w in s:gmatch('%w+') do end") >= 10_000 / 4, "gmatch");
    assert!(used("local x = s:find('%d')") >= 10_000 / 4, "find");
}

/// Helper: `SWEEP2_MIN=<file>` (`SWEEP2_FUEL`, `SWEEP2_MEM`): drops lines while fast and
/// full still differ; prints what is left.
#[test]
#[ignore = "helper: minimizer"]
fn minimize() {
    let Ok(path) = std::env::var("SWEEP2_MIN") else { return };
    let f = std::env::var("SWEEP2_FUEL").ok().and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let m = std::env::var("SWEEP2_MEM").ok().and_then(|s| s.parse().ok()).unwrap_or(64 << 20);
    let limits = Limits::default().fuel(f).memory(m);
    let differs = |lines: &[String]| {
        let src = lines.join("\n");
        measured(&src, limits, true) != measured(&src, limits, false)
    };
    let mut lines: Vec<String> = std::fs::read_to_string(path).unwrap().lines().map(str::to_owned).collect();
    assert!(differs(&lines), "does not differ");
    let mut changed = true;
    while changed {
        changed = false;
        let mut i = 0;
        while i < lines.len() {
            let mut cand = lines.clone();
            cand.remove(i);
            if differs(&cand) {
                lines = cand;
                changed = true;
            } else {
                i += 1;
            }
        }
    }
    let src = lines.join("\n");
    println!("MIN\n{src}\nfast {:?}\nfull {:?}", measured(&src, limits, true), measured(&src, limits, false));
}

// SEC2-3 (minor, fast loop not indistinguishable): fuel differed by a unit between the
// fast loop and the full path when a collection ran (found by `hostile_fuzz_fast_vs_full`).
// The collector's work is charged as fuel and it scans every slot of a stack that is not
// running, so anything the two paths leave differently there shows: (a) a fast `return`
// copied only the results wanted, the full path all of them; (b) a fast field write to a
// string key skipped the table barrier the full path takes (`raw_set` barriers when the
// key is collectable); (c) resize work under the 16-slot charging threshold was dropped by
// a full-path write but kept across fast in-place writes. Fixes: copy all results; the
// same barrier rule; `take_resized` keeps amounts below the threshold.
#[test]
fn sec2_3_fast_loop_fuel_matches_through_collections() {
    let programs = [
        r#"
local t, u, mt, o = {1, 2, a = 1}, {3, 4}, {}, {}
local x, n, s, k, mm, co = 0, 0, 'abc', 1, 1, nil
local ls, ls2 = string.rep('ab', 30), string.rep('ab', 30)
for iter = 1, 3 do
pcall(function() mt.__newindex = function(tt, kk, vv) rawset(tt, kk, vv) end end)
co = coroutine.wrap(function() for k2, v2 in pairs(t) do coroutine.yield(k2) end for i2 in ipairs(u) do coroutine.yield(i2) end end) for i3 = 1, 4 do x = co() end
pcall(function() o.a = x end)
pcall(function() for i8 = 1, 8 do x = t[3] == u[3]
x = pcall(function() mt.__index = function(_, kk) n = n + 1 return kk end
x = #u end)
local t9 = {} for i9 = 1, 167 do t9[i9] = i9 t9['k' .. i9] = i9 end t = t9 end end)
x = pcall(function() co = coroutine.wrap(function() for k2, v2 in pairs(t) do coroutine.yield(k2) end for i2 in ipairs(u) do coroutine.yield(i2) end end) for i3 = 1, 4 do x = co() end end)
pcall(function() mt.__newindex = nil mt.__index = nil end)
for i8 = 1, 12 do co = coroutine.wrap(function() for k2, v2 in pairs(t) do coroutine.yield(k2) end for i2 in ipairs(u) do coroutine.yield(i2) end end) for i3 = 1, 4 do x = co() end
x = string.format('%s %d %q', s, n, ls) end
end
return n"#,
        r#"
local t, u, mt, o = {1, 2, a = 1}, {3, 4}, {}, {}
for iter = 1, 3 do
pcall(function() k = mm mm = {} end)
pcall(function() mt.__newindex = nil mt.__index = nil end)
pcall(function() x = pcall(function() t[#t + 1] = {1, 2}
x = table.concat(u, ',') end) end)
pcall(function() local t9 = {} for i9 = 1, 64 do t9[i9] = i9 t9['k' .. i9] = i9 end t = t9 end)
pcall(function() local t9 = {} for i9 = 1, 60 do t9[i9] = i9 t9['k' .. i9] = i9 end t = t9 end)
pcall(function() o = setmetatable({}, {__index = u}) u.m = function(self, a) return a end end)
pcall(function() collectgarbage('step') end)
collectgarbage('step', 100)
pcall(function() x = (s == ls) or (ls == ls2) or (ls < ls2) end)
pcall(function() co = coroutine.wrap(function() for k2, v2 in pairs(t) do coroutine.yield(k2) end for i2 in ipairs(u) do coroutine.yield(i2) end end) for i3 = 1, 4 do x = co() end end)
u.a = x
end"#,
    ];
    for p in programs {
        for mem in [200_665, 64 << 20] {
            both(p, Limits::default().fuel(200_000).memory(mem), 20);
        }
    }
}

// SEC2-4 (critical, fuel bypass): `string.format('%.3s', s)` copied and scanned all of
// `s` but was charged for the 3 bytes written: a 1 MB `s` made 3M fuel last 49 s.
// `%q` built a heap string per control character (`format!`), so `%q` of a string of
// NULs ran ~1 us per unit of fuel. Fixes: `%s` with a specification charges the input's
// length; `%q` writes `\ddd` without allocating (output checked against C Lua for all 256
// bytes).
#[test]
fn sec2_4_format_charges_what_it_reads() {
    let (r, _) = both(
        "local s = string.rep('a', 1e6) while true do local x = string.format('%.3s', s) end",
        fuel(3_000_000),
        10,
    );
    assert_eq!(r, "limit Fuel");
    let (r, _) = both(
        "local s = string.rep(string.char(0), 1e6) while true do local x = string.format('%q', s) end",
        fuel(3_000_000),
        10,
    );
    assert_eq!(r, "limit Fuel");
    let (r, _) = both(
        "local t = {} for i = 0, 255 do t[#t + 1] = string.char(i) .. '1' .. string.char(i) end \
         local q = string.format('%q', table.concat(t)) return #q, load('return ' .. q)() == table.concat(t)",
        fuel(1_000_000),
        10,
    );
    assert_eq!(r, "ok Int(927),Bool(true)");
}

// SEC2-5 (critical, fuel bypass): plain `find` (`find_plain`, C Lua's `lmemfind`) is
// O(#s * #p) at worst but was charged #s/64: `('a'):rep(1e6):find(('a'):rep(1e4)..'b', 1,
// true)` made 3M fuel last 81 s, and one call with longer strings runs for hours.
// Fix: it counts the bytes it compares and stops once past the fuel left.
#[test]
fn sec2_5_plain_find_charges_compares() {
    let (r, _) = both(
        "local s = string.rep('a', 1e6) local p = string.rep('a', 1e4) .. 'b' while true do local x = s:find(p, 1, true) end",
        fuel(3_000_000),
        10,
    );
    assert_eq!(r, "limit Fuel");
    // One call over the largest strings stops at the fuel, not at the end of its work.
    let (r, _) = both(
        "local s = string.rep('a', 8e6) local p = string.rep('a', 4e6) .. 'b' return s:find(p, 1, true)",
        fuel(1_000_000),
        10,
    );
    assert_eq!(r, "limit Fuel");
    let (r, _) = both(
        "return ('hello world'):find('o w', 1, true), ('abc'):find('', 4, true), ('abc'):find('c', -1, true)",
        fuel(10_000),
        10,
    );
    assert_eq!(r, "ok Int(5),Int(4),Int(3),Int(3)");
}

// SEC2-6 (critical, fuel bypass): a match that failed with "pattern too complex" returned
// the error before its steps were charged, so `pcall(string.find, s, 'a*a*a*a*b')` did up
// to 4M+ steps per call for ~3 units of fuel: an endless loop of them never ran out of
// fuel. Also one match could take `max_steps` (4M + 64 per subject byte) whatever fuel was
// left. Fixes: steps are charged before a pattern error is raised (find, match, gmatch,
// gsub) and a match's step budget is capped by the fuel left.
#[test]
fn sec2_6_pattern_errors_are_paid_for() {
    for call in [
        "pcall(string.find, s, 'a*a*a*a*b')",
        "pcall(string.gsub, s, 'a*a*a*a*b', '')",
        "pcall(function() for x in s:gmatch('a*a*a*a*b') do end end)",
    ] {
        let (r, _) =
            both(&format!("local s = string.rep('a', 1e5) while true do {call} end"), fuel(3_000_000), 10);
        assert_eq!(r, "limit Fuel", "{call}");
    }
    let (r, used) = both("return pcall(string.find, string.rep('a', 3e4), 'a*a*a*b')", fuel(10_000_000), 10);
    assert_eq!(r, "ok Bool(false),pattern too complex");
    assert!(used > 1_000_000, "{used}");
    let (r, _) = both("return pcall(string.find, string.rep('a', 1e6), 'a*a*a*a*b')", fuel(100_000), 10);
    assert_eq!(r, "limit Fuel");
}

// SEC2-7 (critical, fuel bypass): the interner's string hash (`heap::hash_bytes`,
// FxHash-style `h = (rotl(h, 5) ^ w) * K`) had collisions independent of the seed: flipping
// the top bit of one 8-byte word and bit 4 of the next leaves the hash unchanged (the
// multiply carries a top-bit difference through unchanged), so with k such pairs a script
// makes 2^k strings with one hash under every seed. Interning them as table keys walked one
// chain, uncharged: quadratic work for linear fuel (checked against the old function for
// random seeds). Fix: a keyed folded multiply per 8 bytes (factor derived from the seed).
#[test]
fn sec2_7_no_seed_free_string_collisions() {
    // 2^15 strings of 15 flip pairs (240 bytes) each, used as keys.
    let src = r#"
        local A, B = {}, {}
        for j = 1, 15 do
          local w = {} for i = 1, 16 do w[i] = 65 + (j + i) % 26 end
          A[j] = string.char(table.unpack(w))
          w[8] = w[8] ~ 0x80 w[9] = w[9] ~ 0x10
          B[j] = string.char(table.unpack(w))
        end
        local t, parts = {}, {}
        for v = 0, (1 << 15) - 1 do
          for j = 1, 15 do parts[j] = (v >> (j - 1)) & 1 == 1 and B[j] or A[j] end
          t[table.concat(parts)] = v
        end
        local n = 0 for k in pairs(t) do n = n + 1 end
        return n"#;
    let (r, _) = both(src, Limits::default().fuel(50_000_000).memory(128 << 20), 20);
    assert_eq!(r, "ok Int(32768)");
}

/// The murmur3 finalizer `value::hash_key` mixes with, and its inverse.
fn fmix(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

fn unfmix(mut h: u64) -> u64 {
    // Modular inverses of the two odd factors.
    let inv = |a: u64| {
        let mut x = a;
        for _ in 0..6 {
            x = x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)));
        }
        x
    };
    h ^= h >> 33;
    h = h.wrapping_mul(inv(0xc4ce_b9fe_1a85_ec53));
    h ^= h >> 33;
    h = h.wrapping_mul(inv(0xff51_afd7_ed55_8ccd));
    h ^ (h >> 33)
}

/// `n` float keys that all hash to slot 0 of every hash part up to 2^20 slots in a state
/// with `hash_seed(base)` (an adversary who knows the seed: tables start with
/// `mix_seed(base, 2)`), packed as doubles.
fn colliding_floats(base: u64, n: usize) -> Vec<u8> {
    let table_seed = fmix(base ^ 2u64.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut out = Vec::new();
    let mut i = 1u64;
    while out.len() < n * 8 {
        let raw = unfmix(i << 20) ^ table_seed;
        assert_eq!(fmix(raw ^ table_seed) & 0xfffff, 0);
        let f = f64::from_bits(raw ^ 0x5555);
        if f.is_finite() && f.fract() != 0.0 {
            out.extend_from_slice(&f.to_le_bytes());
        }
        i += 1;
    }
    out
}

// SEC2-8 (critical when the seed is known): non-integer keys hash with the state's seed, so
// an adversary who knows it (a host that fixes `hash_seed` to replay runs, a leak) computes
// keys that all land in one slot (the mixer is invertible): every insert and lookup walked
// the whole run, uncharged. 40k float keys: 8e8 probes to insert, 40k per lookup.
// Fix (`table.rs`): an insert that would make a run too long gives the table a new seed
// derived from its own and rehashes under it, charged as a resize. (Since sweep 5b, S5GC-5:
// the hash part is chained, a lookup walks only its home's keys, and a chain past
// `CHAIN_FREE` reseeds; steps past it are fuel.) Runs with one seed replay exactly, fast
// loop on or off.
#[test]
fn sec2_8_known_seed_collisions_stay_bounded() {
    let seed = 0x5eed_7e57;
    let keys = colliding_floats(seed, 40_000);
    let src = r#"
        local t, ks = {}, {}
        for i = 1, #KEYS // 8 do ks[i] = string.unpack('<d', KEYS, i * 8 - 7) end
        for i = 1, #ks do t[ks[i]] = i end
        local hits, misses = 0, 0
        for r = 1, 5 do
          for i = 1, #ks do
            if t[ks[i]] == i then hits = hits + 1 end
            if t[ks[i] * 3] == nil then misses = misses + 1 end
          end
        end
        return hits, misses"#;
    let run = move |fast: bool| {
        let limits = Limits::default().fuel(200_000_000).memory(64 << 20).hash_seed(seed);
        let mut lua = Lua::new(limits, Libs::ALL);
        lua.set_fast_loop(fast);
        let k = lua.create_string(&keys).unwrap();
        lua.set_global("KEYS", k).unwrap();
        let start = Instant::now();
        let r = describe(&mut lua, src);
        (r, limits.fuel - lua.fuel_left(), lua.memory_used(), start.elapsed())
    };
    let ((a, b), _) = bounded(60, move || (run(true), run(false))).unwrap();
    assert_eq!(a.0, "ok Int(200000),Int(200000)");
    assert_eq!((&a.0, a.1, a.2), (&b.0, b.1, b.2), "fast and full differ");
    // Bounded: a few hundred ns per operation, not the 20k-probe walks of before.
    assert!(a.3 < Duration::from_secs(3), "{:?}", a.3);
}
