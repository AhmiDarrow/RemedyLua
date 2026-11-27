//! Sweep 3, stability (goal 4: runs for days without degrading). Soak tests with a small
//! default; `REMEDYLUA_S3_SECS=<n>` turns each into an `n`-second run (the bounded long run:
//! `cargo test --release -p remedy-lua --test all sweep3_stab -- --nocapture`).
//!
//! Every soak runs two states in lockstep, one with the fast loop and one without, and
//! requires the same results, fuel and `memory_used` from both at every step. Besides the
//! account, `Lua::internal_sizes` shows the bookkeeping it does not count (interner, roots,
//! scratch, live objects per slab, the tracked-userdata queue, app data), which must come
//! back to its baseline too. Samples print as `S3,<test>,<secs>,<iter>,<memory_used>`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

use super::common;

// ---- harness ----

/// Steps a soak runs: `iters`, or `REMEDYLUA_S3_SECS` seconds when that is set.
struct Budget {
    iters: u64,
    secs: Option<u64>,
    start: Instant,
}

fn budget(iters: u64) -> Budget {
    let secs = std::env::var("REMEDYLUA_S3_SECS").ok().and_then(|s| s.parse().ok());
    Budget { iters, secs, start: Instant::now() }
}

impl Budget {
    fn more(&self, i: u64) -> bool {
        match self.secs {
            Some(s) => self.start.elapsed() < Duration::from_secs(s),
            None => i < self.iters,
        }
    }
}

fn limits() -> Limits {
    Limits::default().fuel(50_000_000).memory(32 << 20).hash_seed(common::TEST_SEED)
}

/// A pair of states, fast loop on and off.
fn pair(l: Limits, libs: Libs) -> [Lua; 2] {
    let mut a = Lua::new(l, libs);
    let mut b = Lua::new(l, libs);
    a.set_fast_loop(true);
    b.set_fast_loop(false);
    [a, b]
}

fn gc_mem(lua: &mut Lua) -> usize {
    lua.collect_garbage().unwrap();
    lua.memory_used()
}

fn fuel_used(lua: &Lua) -> u64 {
    lua.limits().fuel - lua.fuel_left()
}

fn sizes(lua: &Lua) -> Vec<(&'static str, usize)> {
    lua.internal_sizes()
}

/// The sizes that must return exactly to their baseline (the stack, slabs and interner table may
/// stay at their peak size; the stack is bounded by the collector's shrink).
fn counts(lua: &Lua) -> Vec<(&'static str, usize)> {
    // The collector's scratch capacity (S6GC-M1) varies with the heap's shape below its
    // keep threshold (64 KiB for each of its seven buffers): only whether it is past that
    // is compared.
    sizes(lua)
        .into_iter()
        .filter(|(k, _)| {
            !matches!(
                *k,
                "stack" | "interner_slots" | "interner_moving" | "slab_slots" | "free_slots" | "slack_bytes"
            )
        })
        .map(|(k, v)| if k == "gc_scratch_bytes" { (k, usize::from(v > 7 * 64 * 1024)) } else { (k, v) })
        .collect()
}

fn size(lua: &Lua, key: &str) -> usize {
    sizes(lua).into_iter().find(|(k, _)| *k == key).unwrap().1
}

/// A call's outcome as text.
fn outcome(lua: &Lua, r: &Result<Vec<Value>, Error>) -> String {
    match r {
        Ok(v) => v.iter().map(|x| common::show(lua, *x)).collect::<Vec<_>>().join(","),
        Err(e) => format!("err {e}"),
    }
}

/// Runs `src` in both states; both must agree on outcome, fuel and memory.
fn both_exec(p: &mut [Lua; 2], src: &str) -> String {
    let r: Vec<(String, u64, usize)> = p
        .iter_mut()
        .map(|l| {
            let r = l.exec(src, "=s");
            (outcome(l, &r), fuel_used(l), l.memory_used())
        })
        .collect();
    assert_eq!(r[0], r[1], "fast loop and full path differ for:\n{src}");
    r[0].0.clone()
}

/// Calls global `name` in both states; same checks.
fn both_call(p: &mut [Lua; 2], name: &str, args: &[i64]) -> String {
    let r: Vec<(String, u64, usize)> = p
        .iter_mut()
        .map(|l| {
            let a: Vec<Value> = args.iter().map(|&i| Value::Int(i)).collect();
            let r = l.call_global(name, &a).map(|o| o.unwrap_or_default());
            (outcome(l, &r), fuel_used(l), l.memory_used())
        })
        .collect();
    assert_eq!(r[0], r[1], "fast loop and full path differ calling {name}{args:?}");
    r[0].0.clone()
}

fn sample(name: &str, b: &Budget, i: u64, mem: usize) {
    println!("S3,{name},{:.1},{i},{mem}", b.start.elapsed().as_secs_f64());
}

/// The peak of the last third of `xs` stays within 2% + 32 KiB of the middle third's.
fn assert_plateau(name: &str, xs: &[usize]) {
    let n = xs.len();
    if n < 6 {
        return;
    }
    let peak = |s: &[usize]| s.iter().copied().max().unwrap_or(0);
    let (m, l) = (peak(&xs[n / 3..2 * n / 3]), peak(&xs[2 * n / 3..]));
    assert!(l <= m + m / 50 + (32 << 10), "{name}: memory creeps: middle peak {m}, last peak {l}");
}

// ---- 1. VMs created and dropped ----

struct Payload(#[allow(dead_code)] Arc<()>);

#[test]
fn vms_created_and_dropped_free_everything() {
    let token = Arc::new(());
    let b = budget(1500);
    let mut first: Option<(usize, Vec<(&'static str, usize)>)> = None;
    let mut i = 0;
    while b.more(i) {
        let mut lua = Lua::new(limits().fuel(5_000_000), Libs::ALL);
        lua.set_fast_loop(i % 2 == 0);
        let fresh = (lua.memory_used(), counts(&lua));
        assert_eq!(first.get_or_insert_with(|| fresh.clone()), &fresh, "fresh VM {i} differs");
        // Everything that holds host data: functions, userdata (tracked too), app data, the
        // time limit's clock, the print sink, an interrupt handle kept past the drop.
        let t = Arc::clone(&token);
        lua.register("host.echo", move |_, a| {
            let _held = &t;
            Ok(a.to_vec())
        })
        .unwrap();
        let ud = lua.create_userdata(Payload(Arc::clone(&token)), None).unwrap();
        lua.set_global("u", ud).unwrap();
        let tud = lua.create_userdata_tracked(Payload(Arc::clone(&token)), None, i).unwrap();
        lua.set_global("tu", tud).unwrap();
        lua.set_app_data(Payload(Arc::clone(&token)));
        let t = Arc::clone(&token);
        lua.set_time_limit(Some((
            Arc::new(move || {
                let _held = &t;
                Duration::ZERO
            }),
            Duration::from_secs(60),
        )));
        let t = Arc::clone(&token);
        lua.set_print(move |_| {
            let _held = &t;
        });
        let stop = lua.interrupt_handle();
        let r = lua
            .exec(
                "local t = {} for i = 1, 100 do t[i] = tostring(i) .. string.rep('y', 41) end \
                 local co = coroutine.wrap(function() coroutine.yield(#t) end) \
                 local parked = coroutine.create(function() coroutine.yield() end) coroutine.resume(parked) \
                 print(#t) \
                 return host.echo(co() + #table.concat(t)) + (host.echo(u) == u and 1 or 0)",
                "=churn",
            )
            .unwrap();
        assert_eq!(common::show(&lua, r[0]), (100 + 100 * 41 + 192 + 1).to_string());
        drop(lua);
        assert_eq!(Arc::strong_count(&token), 1, "VM {i} kept host data after drop");
        assert!(!stop.is_set());
        if i % 250 == 0 {
            sample("vm_churn", &b, i, fresh.0);
        }
        i += 1;
    }
}

// ---- 2. scripts loaded and unloaded ----

#[test]
fn scripts_loaded_and_unloaded_return_to_baseline() {
    let mut p = pair(limits(), Libs::ALL - Libs::PRINT);
    both_exec(&mut p, "function compile(src) return load(src)() end");
    for l in &mut p {
        gc_mem(l);
    }
    let base: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
    let b = budget(1500);
    let mut curve = Vec::new();
    let mut i: u64 = 0;
    while b.more(i) {
        // New chunks each time: prototypes, constants, short and long strings, a wide frame.
        let wide: String = (0..300).map(|k| format!("local v{k} = {k} + {i}\n")).collect();
        let src = format!(
            "local t = {{}} for k = 1, 8 do t[k] = 'chunk{i}_' .. k .. string.rep('L', 40) end \
             local h = {{}} h['key{i}' .. string.rep('K', 50)] = 1 \
             local function f(x) return x + {i} end \
             {wide} return f(#t) + #t[8] + v299"
        );
        for l in &mut p {
            let f = l.load(&src, &format!("=chunk{i}")).unwrap();
            l.call(f.value(), &[]).unwrap();
        }
        let r =
            both_exec(&mut p, &format!("return compile([[ return {i} * 2, 'u{i}' .. ('{i}'):rep(30) ]])"));
        assert!(r.starts_with(&(i * 2).to_string()));
        if i % 100 == 99 {
            let m: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
            assert_eq!(m[0], m[1], "fast and full differ in what they hold");
            assert_eq!(m[0], base[0], "chunk {i}: memory or bookkeeping did not return to the baseline");
            curve.push(m[0].0);
            sample("load_unload", &b, i, m[0].0);
            // The interner's table shrinks back when its strings go (it may keep a peak).
            assert!(size(&p[0], "interner_slots") <= 8192, "interner table grows: {:?}", sizes(&p[0]));
        }
        i += 1;
    }
}

// ---- 3. a long-lived script allocating and freeing ----

/// Exercises what changed since cycle 1: long strings as keys, concat, gsub/find/format,
/// pairs/ipairs/next, `__index` chains, new tables, wide frames, varargs, shared upvalues,
/// coroutines, weak tables, `<close>`, table resizes.
pub const CHURN: &str = r#"
local cache = setmetatable({}, {__mode = 'v'})
local wk = setmetatable({}, {__mode = 'k'})
local ring, pos = {}, 0
local Base = {get = function(self) return self.v end}
Base.__index = Base
local Mid = setmetatable({}, Base) Mid.__index = Mid
local wide = load((function()
  local b = {} for k = 1, 280 do b[#b+1] = 'local a'..k..' = ...' end
  b[#b+1] = 'return a1, a280' return table.concat(b, '\n') end)()) or function(x) return x, x end
local function sum(...) local s = 0 for _, v in ipairs({...}) do s = s + v end return s end
function step(n)
  local acc = 0
  for i = 1, 40 do
    local k = 'long key number ' .. n .. ':' .. i .. string.rep('#', 30)
    local t = {k = k, n = i, [k] = i}
    for kk, v in pairs(t) do if type(v) == 'number' then acc = acc + v end end
    local o = setmetatable({v = i}, Mid)
    acc = acc + o:get()
    local s = ('x%dy'):format(i):rep(3)
    s = s:gsub('y', function(c) return c:upper() end)
    acc = acc + (s:find('Y', 1, true) or 0)
    local cnt = 0
    local function inc() cnt = cnt + 1 end
    local function get() return cnt end
    inc() inc()
    acc = acc + get() + sum(i, i, i)
    local co = coroutine.wrap(function(a) local b = coroutine.yield(a + 1) return b * 2 end)
    acc = acc + co(i) + co(i)
    local parked = coroutine.create(function() coroutine.yield(t) end)
    coroutine.resume(parked)
    cache[i] = {i} wk[{}] = t
    do local c <close> = setmetatable({}, {__close = function() acc = acc + 1 end}) end
    local big = {} for j = 1, 100 do big[j] = j end
    for j = 100, 1, -1 do big[j] = nil end
    local a1, a280 = wide(i)
    acc = acc + a1 + a280
    pos = pos % 64 + 1
    ring[pos] = {k, t, o, parked, string.rep(k, 3)}
    acc = acc + (next(t) and 1 or 0)
  end
  return acc
end
"#;

#[test]
fn long_lived_script_plateaus_and_paths_agree() {
    let mut p = pair(limits(), Libs::ALL);
    both_exec(&mut p, CHURN);
    let b = budget(400);
    let (mut curve, mut slots) = (Vec::new(), Vec::new());
    let mut i = 0;
    let mut warm = None;
    while b.more(i) {
        let r = both_call(&mut p, "step", &[i as i64]);
        assert!(!r.starts_with("err"), "step {i}: {r}");
        if i % 20 == 19 {
            let m: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
            assert_eq!(m[0], m[1], "step {i}: fast and full hold different things");
            curve.push(m[0].0);
            // What the account does not see: slab, interner and stack sizes.
            slots.push(
                ["slab_slots", "interner_slots", "stack"].iter().map(|k| size(&p[0], k)).sum::<usize>(),
            );
            sample("churn", &b, i, m[0].0);
            // After warm-up the live set is fixed (a 64-slot ring): so are the counts.
            if i >= 99 {
                let w = warm.get_or_insert_with(|| m[0].1.clone());
                for ((k, a), (_, z)) in w.iter().zip(&m[0].1) {
                    assert!(*z <= a + a / 10 + 64, "step {i}: {k} grows: {a} -> {z}");
                }
            }
        }
        i += 1;
    }
    assert_plateau("churn", &curve);
    assert_plateau("churn slots", &slots);
}

/// The same churn in C Lua: its `collectgarbage('count')` curve plateaus too (printed next
/// to ours for the report).
#[test]
fn churn_in_c_lua_for_comparison() {
    let mut c = remedy_lua_oracle::State::new();
    c.exec(CHURN).unwrap();
    let mut l = Lua::new(limits(), Libs::ALL);
    l.exec(CHURN, "=churn").unwrap();
    let b = budget(200);
    let (mut ours, mut theirs) = (Vec::new(), Vec::new());
    let mut i = 0;
    while b.more(i) {
        let a = c.exec(&format!("return step({i})")).unwrap();
        let o = l.call_global("step", &[Value::Int(i as i64)]).unwrap().unwrap();
        assert_eq!(a, common::show(&l, o[0]), "step {i}: RemedyLua and C Lua differ");
        if i % 20 == 19 {
            let kb: f64 = c.exec("collectgarbage() return collectgarbage('count')").unwrap().parse().unwrap();
            theirs.push((kb * 1024.0) as usize);
            ours.push(gc_mem(&mut l));
            println!("S3,churn_vs_c,{i},ours,{},c,{}", ours.last().unwrap(), theirs.last().unwrap());
        }
        i += 1;
    }
    assert_plateau("churn in C Lua", &theirs);
    assert_plateau("churn in RemedyLua", &ours);
}

// ---- 4. coroutines created and abandoned ----

#[test]
fn coroutines_created_and_abandoned_return_to_baseline() {
    let mut p = pair(limits(), Libs::ALL);
    both_exec(
        &mut p,
        r#"
        local function deep(n) if n == 0 then coroutine.yield(1) return 0 end return 1 + deep(n - 1) end
        function burst(n)
          local s = 0
          for i = 1, 50 do
            -- Abandoned while suspended deep in recursion, with open upvalues and <close>.
            local co = coroutine.create(function(x)
              local up = x
              local c <close> = setmetatable({}, {__close = function() s = s + 1 end})
              local f = function() up = up + 1 return up end
              deep(30 + i % 7) f()
              coroutine.yield(f())
            end)
            coroutine.resume(co, i)
            -- Dead by error, closed, wrapped and left, finished.
            pcall(coroutine.wrap(function() error({i}) end))
            local cl = coroutine.create(function() local c <close> = setmetatable({}, {__close = function() s = s + 2 end}) coroutine.yield() end)
            coroutine.resume(cl) coroutine.close(cl)
            local w = coroutine.wrap(function(...) coroutine.yield(...) end) w(i, i)
            local done = coroutine.create(function() return i end) coroutine.resume(done)
            -- Nested: a coroutine abandoned while it has resumed another.
            local outer = coroutine.wrap(function()
              local inner = coroutine.create(function() coroutine.yield(1) end)
              coroutine.resume(inner) coroutine.yield(inner)
            end)
            outer()
            s = s + i
          end
          return s
        end
        "#,
    );
    let base: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
    assert_eq!(base[0], base[1]);
    let b = budget(150);
    let mut i = 0;
    while b.more(i) {
        let r = both_call(&mut p, "burst", &[]);
        assert!(!r.starts_with("err"), "{r}");
        if i % 10 == 9 {
            let m: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
            assert_eq!(m[0], m[1]);
            assert_eq!(m[0], base[0], "burst {i}: coroutines left something behind");
            sample("coroutines", &b, i, m[0].0);
        }
        i += 1;
    }
}

// ---- 5. limits hit repeatedly ----

/// An attack, the limits it runs under, and the limit it must hit (`None`: an error the
/// script sees, such as a stack overflow).
struct Attack {
    src: &'static str,
    limits: fn(Limits) -> Limits,
    kind: Option<LimitKind>,
}

const ATTACKS: &[Attack] = &[
    Attack { src: "while true do end", limits: |l| l.fuel(200_000), kind: Some(LimitKind::Fuel) },
    Attack {
        src: "local x = 0 for i = 1, math.huge do x = x + i end",
        limits: |l| l.fuel(150_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack {
        src: "pcall(function() while true do end end) while true do end",
        limits: |l| l.fuel(100_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack {
        src: "local co = coroutine.wrap(function() while true do coroutine.yield() end end) while true do co() end",
        limits: |l| l.fuel(100_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack {
        src: "local t = {} for i = 1, 300 do t[i] = 301 - i end table.sort(t, function(a, b) while true do end end)",
        limits: |l| l.fuel(100_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack {
        src: "string.rep('a', 1000):gsub('a', function() while true do end end)",
        limits: |l| l.fuel(100_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack {
        src: "local t = setmetatable({}, {__index = function(t, k) return t[k + 1] end}) return t[1]",
        limits: |l| l.fuel(100_000).call_depth(2000),
        kind: None,
    },
    // The default call depth is bounded by memory: the same overflow, from the frames
    // filling the budget.
    Attack {
        src: "local t = setmetatable({}, {__index = function(t, k) return t[k + 1] end}) return t[1]",
        limits: |l| l.memory(4 << 20),
        kind: None,
    },
    Attack {
        src: "local function f(n) return 1 + f(n) end return pcall(f, 1), f(2)",
        limits: |l| l.memory(4 << 20),
        kind: None,
    },
    Attack {
        src: "local t = {} while true do t[#t + 1] = {} end",
        limits: |l| l.memory(2 << 20),
        kind: Some(LimitKind::Memory),
    },
    Attack {
        src: "local s = 'x' while true do s = s .. s .. 'y' end",
        limits: |l| l.memory(2 << 20),
        kind: Some(LimitKind::Memory),
    },
    Attack {
        src: "local t = {} for i = 1, 1e7 do t[string.rep('k', 45) .. i] = i end",
        limits: |l| l.memory(2 << 20),
        kind: Some(LimitKind::Memory),
    },
    Attack {
        src: "local co = coroutine.wrap(function() local t = {} while true do t[#t+1] = tostring(#t) .. string.rep('z', 50) end end) co()",
        limits: |l| l.memory(2 << 20),
        kind: Some(LimitKind::Memory),
    },
    Attack { src: "return string.rep('x', 1 << 30)", limits: |l| l.string_len(1 << 20), kind: None },
    Attack {
        src: "local function f(...) return f(1, ...) end return f()",
        limits: |l| l.memory(2 << 20).fuel(2_000_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack { src: "local function f() return 1 + f() end return f()", limits: |l| l, kind: None },
    Attack {
        src: "local t = setmetatable({}, {__lt = function(a, b) table.sort({t, t, t}) return true end}) table.sort({t, t, t})",
        limits: |l| l,
        kind: None,
    },
    Attack {
        src: "while true do print(string.rep('p', 100)) end",
        limits: |l| l.output(10_000).fuel(200_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack {
        src: "local c <close> = setmetatable({}, {__close = function() while true do end end}) error('x')",
        limits: |l| l.fuel(100_000),
        kind: Some(LimitKind::Fuel),
    },
    Attack {
        src: "xpcall(function() error('x') end, function() local t = {} while true do t[#t+1] = {} end end)",
        limits: |l| l.memory(2 << 20),
        kind: Some(LimitKind::Memory),
    },
];

const PROBE: &str = "local t = {} for i = 1, 200 do t[i] = ('p%d'):format(i) end \
    local s = 0 for k, v in pairs(t) do s = s + #v + k end \
    local co = coroutine.wrap(function() coroutine.yield(s) end) return co(), #table.concat(t)";

#[test]
fn limits_hit_repeatedly_leave_the_vm_exact() {
    let base_limits = limits().memory(8 << 20).fuel(5_000_000);
    let mut p = pair(base_limits, Libs::ALL);
    for l in &mut p {
        l.set_print(|_| {});
    }
    // Run the probe once so one-time growth (stack, interner) is behind us.
    both_exec(&mut p, PROBE);
    // Measured from a clean collector, as every probe below is (the fuel a probe uses
    // depends on where collector steps fall, which depends on what ran before).
    for l in &mut p {
        gc_mem(l);
    }
    let probe = both_exec(&mut p, PROBE);
    let probe_fuel = fuel_used(&p[0]);
    let base: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
    assert_eq!(base[0], base[1]);
    let b = budget(6);
    let mut i = 0;
    while b.more(i) {
        for (n, a) in ATTACKS.iter().enumerate() {
            let mut kinds = Vec::new();
            for l in &mut p {
                l.set_limits((a.limits)(base_limits));
                let r = l.exec(a.src, "=attack");
                l.set_limits(base_limits);
                kinds.push(match r {
                    Err(Error::Limit(k)) => Some(k),
                    _ => None,
                });
            }
            assert_eq!(kinds[0], kinds[1], "attack {n}: fast and full stop differently");
            assert_eq!(kinds[0], a.kind, "attack {n} ({}) stopped by {:?}", a.src, kinds[0]);
            // The state is exactly as it was: same probe result, same fuel, same memory.
            for l in &mut p {
                gc_mem(l);
            }
            let r = both_exec(&mut p, PROBE);
            assert_eq!(r, probe, "attack {n}: the probe's result changed");
            assert_eq!(fuel_used(&p[0]), probe_fuel, "attack {n}: the probe's fuel changed");
            let m: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
            assert_eq!(m[0], base[0], "attack {n} ({}): memory or bookkeeping did not return", a.src);
            assert_eq!(m[1], base[1], "attack {n}: (full path) memory or bookkeeping did not return");
        }
        sample("limits", &b, i, base[0].0);
        i += 1;
    }
}

// ---- 6. interrupts and time limits fired repeatedly ----

/// A clock that moves one millisecond each time it is read: time limits fire
/// deterministically.
fn tick_clock() -> remedy_lua::Clock {
    let t = Arc::new(AtomicU64::new(0));
    Arc::new(move || Duration::from_millis(t.fetch_add(1, Ordering::Relaxed)))
}

#[test]
fn interrupts_and_time_limits_fired_repeatedly() {
    let mut p = pair(limits().fuel(u64::MAX), Libs::ALL);
    for l in &mut p {
        // A host function that interrupts the call it runs in, deterministically.
        l.register("host.stop", |lua, _| {
            lua.interrupt_handle().interrupt();
            Ok(vec![])
        })
        .unwrap();
    }
    both_exec(&mut p, PROBE);
    // Measured from a clean collector, as every probe below is (the fuel a probe uses
    // depends on where collector steps fall, which depends on what ran before).
    for l in &mut p {
        gc_mem(l);
    }
    let probe = both_exec(&mut p, PROBE);
    let probe_fuel = fuel_used(&p[0]);
    let base: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
    let b = budget(40);
    let mut i = 0;
    let mut worst = Duration::ZERO;
    while b.more(i) {
        // 1. From inside, in a loop, a coroutine, a sort comparator and a gsub callback.
        for src in [
            "host.stop() while true do end",
            "coroutine.wrap(function() host.stop() while true do end end)()",
            "local t = {3, 2, 1} table.sort(t, function(a, b) host.stop() while true do end end)",
            "local ok = pcall(function() host.stop() local t = {} while true do t[#t+1] = {} end end) return ok",
        ] {
            for l in &mut p {
                let r = l.exec(src, "=int");
                assert_eq!(r, Err(Error::Limit(LimitKind::Interrupted)), "{src}");
                // Still set: every entry stops at once until the host resets it.
                assert_eq!(l.exec("return 1", "=x"), Err(Error::Limit(LimitKind::Interrupted)));
                l.interrupt_handle().reset();
            }
        }
        // 2. The time limit by a deterministic clock.
        for l in &mut p {
            l.set_time_limit(Some((tick_clock(), Duration::from_millis(5))));
            let r = l.exec("local t = {} for i = 1, math.huge do t[i % 100 + 1] = {i} end", "=time");
            assert_eq!(r, Err(Error::Limit(LimitKind::Time)));
            l.set_time_limit(None);
        }
        // 3. From another thread, by the wall clock, while a loop runs: how late is the stop?
        for l in &mut p {
            let stop = l.interrupt_handle();
            let t0 = Instant::now();
            let th = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(3));
                stop.interrupt();
                Instant::now()
            });
            let r = l.exec("local s = 'a' while true do s = (s .. 'b'):sub(-10) end", "=wall");
            let ended = Instant::now();
            let fired = th.join().unwrap();
            assert_eq!(r, Err(Error::Limit(LimitKind::Interrupted)));
            assert!(ended > t0);
            worst = worst.max(ended.saturating_duration_since(fired));
            l.interrupt_handle().reset();
            // A real-clock time limit too.
            l.set_time_limit(Some((
                Arc::new({
                    let s = Instant::now();
                    move || s.elapsed()
                }),
                Duration::from_millis(3),
            )));
            assert_eq!(l.exec("while true do end", "=t"), Err(Error::Limit(LimitKind::Time)));
            l.set_time_limit(None);
            // How much garbage the wall-clock runs left depends on timing: start clean.
            gc_mem(l);
        }
        // Afterwards: the same probe, the same fuel, the same memory, both paths.
        let r = both_exec(&mut p, PROBE);
        assert_eq!(r, probe);
        assert_eq!(fuel_used(&p[0]), probe_fuel);
        let m: Vec<_> = p.iter_mut().map(|l| (gc_mem(l), counts(l))).collect();
        assert_eq!(m, base, "round {i}: memory or bookkeeping did not return");
        i += 1;
    }
    println!("S3,interrupt_latency_worst_ms,{:.3}", worst.as_secs_f64() * 1000.0);
    assert!(worst < Duration::from_millis(250), "an interrupt took {worst:?} to stop the loop");
}

// ---- 7. host bookkeeping: tracked userdata, app data, roots ----

#[test]
fn tracked_userdata_app_data_and_roots_do_not_pile_up() {
    let token = Arc::new(());
    let mut lua = Lua::new(limits(), Libs::ALL);
    lua.exec("keep = setmetatable({}, {__mode = 'v'})", "=s").unwrap();
    let base = (gc_mem(&mut lua), counts(&lua));
    let (base_free, base_slack) = (size(&lua, "free_slots"), size(&lua, "slack_bytes"));
    let b = budget(300);
    let mut next_key = 0u64;
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while b.more(i) {
        // Tracked userdata held by scripts briefly, then dropped; each key reported once.
        let mut made = Vec::new();
        for _ in 0..20 {
            let u = lua.create_userdata_tracked(Payload(Arc::clone(&token)), None, next_key).unwrap();
            made.push(next_key);
            next_key += 1;
            lua.set_global("tmp", u).unwrap();
            lua.exec("keep[#keep + 1] = tmp tmp = nil", "=s").unwrap();
        }
        // App data replaced and taken.
        lua.set_app_data(Payload(Arc::clone(&token)));
        lua.set_app_data(Payload(Arc::clone(&token)));
        lua.set_app_data(7u32);
        assert_eq!(lua.take_app_data::<u32>(), Some(7));
        // Roots made and dropped without anything being allocated.
        for _ in 0..200 {
            let r = lua.root(Value::Table(lua.globals()));
            drop(r);
        }
        lua.collect_garbage().unwrap();
        for k in lua.take_collected() {
            assert!(seen.insert(k), "key {k} reported twice");
        }
        for k in made {
            assert!(seen.contains(&k), "key {k} never reported");
        }
        assert!(size(&lua, "roots") <= 256, "dropped roots pile up: {}", size(&lua, "roots"));
        // Only the one app data payload holds the token.
        assert_eq!(Arc::strong_count(&token), 2);
        i += 1;
    }
    drop(lua.take_app_data::<Payload>());
    lua.exec("keep = setmetatable({}, {__mode = 'v'})", "=s").unwrap();
    let end = (gc_mem(&mut lua), counts(&lua));
    // The live bytes return exactly. The `keep` table and its metatable just replaced
    // leave their two slots free below the new ones (SEC-M5: free slots are counted in
    // `memory_used`); nothing else may be left free.
    assert_eq!(
        end.0 - size(&lua, "slack_bytes"),
        base.0 - base_slack,
        "live bytes did not return to their baseline"
    );
    assert_eq!(end.1, base.1, "bookkeeping did not return to its baseline");
    assert!(
        size(&lua, "free_slots") <= base_free + 2,
        "free slots pile up: {} from {base_free}",
        size(&lua, "free_slots")
    );
    assert_eq!(Arc::strong_count(&token), 1);
    sample("host_bookkeeping", &b, i, lua.memory_used());
}

// ---- 8. collector stress over the whole corpus ----

fn corpus() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua");
    let long = std::env::var("REMEDYLUA_S3_SECS").is_ok();
    let dirs: &[&str] = if long { &["conformance", "sweep1/spec", "sweep1/fuzz"] } else { &["conformance"] };
    let mut cases = Vec::new();
    for d in dirs {
        let mut files: Vec<_> =
            std::fs::read_dir(root.join(d)).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for f in files.iter().filter(|p| p.extension().is_some_and(|x| x == "lua")) {
            let src = std::fs::read_to_string(f).unwrap().replace("\r\n", "\n");
            for chunk in src.split("\n-- case:") {
                let (name, body) = chunk.split_once('\n').unwrap_or((chunk, ""));
                if !body.trim().is_empty() {
                    cases.push((format!("{}: {}", f.display(), name.trim()), body.to_owned()));
                }
            }
        }
    }
    cases
}

#[test]
fn gc_stress_over_the_corpus_keeps_results() {
    // On a 64 KiB stack: data-driven native recursion overflows here, not in production.
    super::common::small_stack(gc_stress_over_the_corpus_keeps_results_body);
}

fn gc_stress_over_the_corpus_keeps_results_body() {
    let mut failures = Vec::new();
    for (name, body) in corpus() {
        let run = |fast: bool, stress: bool| {
            let mut lua = Lua::new(limits(), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.set_gc_stress(stress);
            let r = match lua.exec(common::wrap(&body), "=wrapper") {
                Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
                Err(e) => Err(e.to_string()),
            };
            lua.set_gc_stress(false);
            (r, gc_mem(&mut lua))
        };
        let normal = run(true, false);
        for fast in [true, false] {
            let s = run(fast, true);
            // Same result; after a full collection the same memory as without stress.
            if s != normal {
                failures.push(format!("{name} (fast {fast}): {:?} vs {:?}", s, normal));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// ---- 9. determinism ----

#[test]
fn fixed_seed_runs_replay_exactly() {
    let src = format!(
        "{CHURN}\nlocal r = {{}} for i = 1, 5 do r[#r+1] = step(i) end \
         local keys = {{}} local t = {{}} for i = 1, 50 do t['k' .. i .. string.rep('z', i % 50)] = i t[i * 1.5] = i end \
         for k in pairs(t) do keys[#keys+1] = tostring(k):sub(1, 6) end \
         r[#r+1] = table.concat(keys, ',') r[#r+1] = math.random(1, 1 << 40) r[#r+1] = tostring(collectgarbage('count') > 0) \
         return table.concat(r, ';')"
    );
    let mut first = None;
    let b = budget(6);
    let mut i = 0;
    while b.more(i) {
        let mut p = pair(limits(), Libs::ALL);
        // Twice in the same states too (string keys hash by object, so the second run's
        // `pairs` order and `math.random` differ from the first, but replay as well).
        let a = (both_exec(&mut p, &src), fuel_used(&p[0]));
        let a2 = (both_exec(&mut p, &src), fuel_used(&p[0]));
        let got = (a, a2, gc_mem(&mut p[0]));
        assert_eq!(first.get_or_insert_with(|| got.clone()), &got, "run {i} differs");
        i += 1;
    }
}

// ---- 10. findings ----

/// stab3-1: after a spike of objects dies, the slabs give their slots back (they stayed
/// at the peak for the state's life), and a handle into the cut-off part stays stale when
/// new objects reuse those slots.
#[test]
fn a_spike_of_objects_gives_its_slots_back() {
    for fast in [true, false] {
        let mut lua = Lua::new(limits().memory(64 << 20), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec("function spike(n) local t = {} for i = 1, n do t[i] = {function() return i end, 'v' .. i} end return t[n][1] end", "=s")
            .unwrap();
        gc_mem(&mut lua);
        let base = size(&lua, "slab_slots");
        let old = lua.call_global("spike", &[Value::Int(100_000)]).unwrap().unwrap()[0];
        assert!(size(&lua, "slab_slots") > base + 200_000);
        gc_mem(&mut lua);
        gc_mem(&mut lua);
        let after = size(&lua, "slab_slots");
        assert!(after < base * 2 + 8192, "slots stay at the peak: {base} -> {after}");
        // The slots are made again: the old handle must not reach the new function there.
        lua.call_global("spike", &[Value::Int(100_000)]).unwrap();
        assert!(matches!(lua.call(old, &[]), Err(Error::Host(_))), "a stale handle reached a new object");
    }
}

/// stab3-2: `print` output the host never takes stays within the output limit (it grew by
/// up to the limit on every call, outside the memory account).
#[test]
fn untaken_print_output_stays_bounded() {
    let mut lua = Lua::new(limits().output(10_000), Libs::ALL);
    for _ in 0..50 {
        lua.exec("for i = 1, 400 do print(('x'):rep(49)) end", "=p").unwrap();
    }
    let out = lua.take_output();
    assert_eq!(out.len(), 10_000, "the buffer holds more than the output limit");
    // Taking it makes room again.
    lua.exec("print('again')", "=p").unwrap();
    assert_eq!(lua.take_output(), "again\n");
}

/// stab3-4: a parked coroutine gives back its 256-slot register window at the next
/// collection (4830 bytes each before, against 1117 in C Lua), and resumes correctly
/// afterwards: many values in, a deep frame, open upvalues and `<close>` intact.
#[test]
fn parked_coroutines_cost_about_what_c_luas_do() {
    let src = "collectgarbage() local a = collectgarbage('count') \
        local t = {} for i = 1, 200 do local c = coroutine.create(function() coroutine.yield() end) coroutine.resume(c) t[i] = c end \
        collectgarbage() local per = (collectgarbage('count') - a) * 1024 / 200 \
        local function deep(n, ...) if n == 0 then return select('#', coroutine.yield(...)) end return deep(n - 1, ...) end \
        local closed = 0 \
        local co = coroutine.create(function(...) \
          local up = 0 local c <close> = setmetatable({}, {__close = function() closed = closed + 1 end}) \
          local function bump() up = up + 1 return up end \
          local n = deep(40, ...) bump() coroutine.yield(n, bump()) return 'done', up end) \
        coroutine.resume(co, 1, 2, 3) collectgarbage() collectgarbage() \
        local _, n, u = coroutine.resume(co, table.unpack({}, 1, 500)) collectgarbage() \
        local _, d, up = coroutine.resume(co) \
        for i = 1, 200 do assert(coroutine.resume(t[i])) end \
        return math.floor(per), n, u, d, up, closed, coroutine.status(co)";
    let mut p = pair(limits(), Libs::ALL);
    let r = both_exec(&mut p, src);
    let parts: Vec<&str> = r.split(',').collect();
    let per: usize = parts[0].parse().unwrap();
    assert!(per < 1800, "a parked coroutine still holds {per} bytes");
    assert_eq!(&parts[1..], ["500", "2", "done", "2", "1", "dead"]);
}
