//! Sweep 1, stability: soak tests. Each scenario runs a normal-length version in the suite
//! and a long version behind `#[ignore]` (`cargo test --test sweep1_soak -- --ignored
//! --nocapture long_`), sized by `REMEDYLUA_SOAK_SECS` (seconds per long test, default 300).
//! Every sample prints a `SOAK,<test>,<secs>,<iter>,<memory_used>,<process bytes>` line so
//! the curves can be collected from the output.
//!
//! Memory is watched three ways: the VM's own account (`memory_used`, after a full
//! collection), `Arc` counts on host data held by functions and userdata (dropped exactly
//! when the VM frees them), and the process's private bytes (long versions only, sampled
//! through the OS; a counting `#[global_allocator]` is impossible here because the
//! workspace forbids `unsafe_code` in every target, test binaries included).
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use remedy_lua::{Data, Error, LimitKind, Libs, Limits, Lua, Value};

// ---- harness ----

/// How long a scenario runs: `iters` steps, or `secs` seconds when set (long versions).
#[derive(Clone, Copy)]
struct Budget {
    iters: u64,
    secs: Option<u64>,
}

fn normal(iters: u64) -> Budget {
    Budget { iters, secs: None }
}

fn long() -> Budget {
    let secs = std::env::var("REMEDYLUA_SOAK_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
    Budget { iters: u64::MAX, secs: Some(secs) }
}

impl Budget {
    fn is_long(self) -> bool {
        self.secs.is_some()
    }
    fn done(self, i: u64, start: Instant) -> bool {
        i >= self.iters || self.secs.is_some_and(|s| start.elapsed() >= Duration::from_secs(s))
    }
}

/// The process's private bytes, from the OS (slow on Windows: about a second a sample).
fn proc_mem() -> Option<u64> {
    if let Ok(s) = std::fs::read_to_string("/proc/self/statm") {
        let pages: u64 = s.split_whitespace().nth(5)?.parse().ok()?;
        return Some(pages * 4096);
    }
    let pid = std::process::id();
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("(Get-Process -Id {pid}).PrivateMemorySize64")])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// A sampled curve of one scenario.
struct Curve {
    name: &'static str,
    start: Instant,
    long: bool,
    last_proc: Instant,
    last_print: f64,
    /// (iteration, memory_used after a full collection, process bytes), at most
    /// `CURVE_CAP`: past it, neighbours are merged keeping their maxima (the plateau check
    /// compares peaks), and later samples are merged `stride` at a time. Every sample was
    /// kept before, so a long soak's own list grew without bound (32 bytes a sample, 8 MB
    /// in two minutes of `long_host`, in doubling steps) and would have tripped the process
    /// check itself.
    samples: Vec<(u64, usize, Option<u64>)>,
    stride: usize,
    pending: Option<((u64, usize, Option<u64>), usize)>,
}

/// The most samples a curve keeps.
const CURVE_CAP: usize = 4096;

/// Two samples as one: the later iteration, the larger of each memory.
fn merge(a: (u64, usize, Option<u64>), b: (u64, usize, Option<u64>)) -> (u64, usize, Option<u64>) {
    (b.0, a.1.max(b.1), a.2.max(b.2))
}

impl Curve {
    fn new(name: &'static str, b: Budget) -> Curve {
        let start = Instant::now();
        Curve {
            name,
            start,
            long: b.is_long(),
            last_proc: start - Duration::from_secs(3600),
            last_print: -1.0,
            samples: Vec::new(),
            stride: 1,
            pending: None,
        }
    }

    /// Records a sample; process memory only in long runs, at most every 20 s. Printed at
    /// most once a second, and with every process reading (printed output is kept in memory
    /// when captured).
    fn sample(&mut self, iter: u64, mem: usize) {
        let proc = if self.long && self.last_proc.elapsed() >= Duration::from_secs(20) {
            self.last_proc = Instant::now();
            proc_mem()
        } else {
            None
        };
        let t = self.start.elapsed().as_secs_f64();
        if t - self.last_print >= 1.0 || proc.is_some() {
            self.last_print = t;
            println!("SOAK,{},{t:.1},{iter},{mem},{}", self.name, proc.map_or(String::new(), |p| p.to_string()));
        }
        let x = (iter, mem, proc);
        let (acc, n) = match self.pending.take() {
            Some((p, n)) => (merge(p, x), n + 1),
            None => (x, 1),
        };
        if n < self.stride {
            self.pending = Some((acc, n));
            return;
        }
        self.samples.push(acc);
        if self.samples.len() >= CURVE_CAP {
            self.samples = self.samples.chunks(2).map(|c| c.iter().copied().reduce(merge).unwrap()).collect();
            self.stride *= 2;
        }
    }

    /// The samples, with one still being merged.
    fn all(&self) -> Vec<(u64, usize, Option<u64>)> {
        let mut v = self.samples.clone();
        v.extend(self.pending.map(|(p, _)| p));
        v
    }

    /// The VM's memory after warm-up must not creep: the last third's peak stays within
    /// 2% (+32 KiB) of the middle third's; process bytes within 16 MiB.
    fn assert_plateau(&self) {
        let samples = self.all();
        let n = samples.len();
        assert!(n >= 6, "{}: too few samples ({n})", self.name);
        let (mid, last) = (&samples[n / 3..2 * n / 3], &samples[2 * n / 3..]);
        let peak = |s: &[(u64, usize, Option<u64>)]| s.iter().map(|x| x.1).max().unwrap_or(0);
        let (m, l) = (peak(mid), peak(last));
        assert!(l <= m + m / 50 + (32 << 10), "{}: VM memory creeps: middle peak {m}, last peak {l}", self.name);
        let procs: Vec<u64> = samples[n / 3..].iter().filter_map(|x| x.2).collect();
        if procs.len() >= 4 {
            let half = procs.len() / 2;
            let (a, b) = (procs[..half].iter().max().unwrap(), procs[half..].iter().max().unwrap());
            assert!(*b <= a + (16 << 20), "{}: process memory creeps: {a} -> {b}", self.name);
        }
    }
}

fn gc_mem(lua: &mut Lua) -> usize {
    lua.collect_garbage().unwrap();
    lua.memory_used()
}

fn fuel_used(lua: &Lua) -> u64 {
    lua.limits().fuel - lua.fuel_left()
}

fn int(v: &[Value]) -> i64 {
    match v.first() {
        Some(Value::Int(i)) => *i,
        other => panic!("expected an integer, got {other:?}"),
    }
}

// ---- 1. VMs created and dropped ----

/// Host data a VM holds; its `Arc` count shows whether the VM let go of it.
struct Payload(#[allow(dead_code)] Arc<()>);

fn vm_churn(b: Budget, name: &'static str) {
    let token = Arc::new(());
    let mut curve = Curve::new(name, b);
    let start = Instant::now();
    let mut baseline = None;
    let mut i = 0;
    while !b.done(i, start) {
        let mut lua = Lua::new(Limits::default().fuel(2_000_000).memory(8 << 20), Libs::STANDARD);
        // Every fresh VM starts at the same size.
        let m0 = lua.memory_used();
        assert_eq!(*baseline.get_or_insert(m0), m0, "fresh VM {i} has a different size");
        let t = Arc::clone(&token);
        lua.register("host.echo", move |_, a| {
            let _held = &t;
            Ok(a.to_vec())
        })
        .unwrap();
        let ud = lua.create_userdata(Payload(Arc::clone(&token)), None).unwrap();
        lua.set_global("u", ud).unwrap();
        let r = lua
            .exec(
                "local t = {} for i = 1, 200 do t[i] = tostring(i) .. 'x' end \
                 local co = coroutine.wrap(function() coroutine.yield(#t) end) \
                 return host.echo(co() + #table.concat(t)) + (host.echo(u) == u and 1 or 0)",
                "=churn",
            )
            .unwrap();
        assert_eq!(int(&r), 200 + 692 + 1);
        drop(lua);
        // The VM is gone: so are its function and userdata, and their host data.
        assert_eq!(Arc::strong_count(&token), 1, "VM {i} leaked host data");
        if i % 250 == 0 {
            curve.sample(i, m0);
        }
        i += 1;
    }
    curve.sample(i, 0);
    if b.is_long() {
        // Process bytes must not follow the number of VMs made.
        let p: Vec<u64> = curve.all().iter().filter_map(|s| s.2).collect();
        if p.len() >= 4 {
            let (a, z) = (p[p.len() / 4], *p.last().unwrap());
            assert!(z <= a + (16 << 20), "process memory grows with VMs made: {a} -> {z}");
        }
    }
}

#[test]
fn vms_created_and_dropped() {
    vm_churn(normal(2000), "vm_churn");
}

#[test]
#[ignore = "long soak: REMEDYLUA_SOAK_SECS"]
fn long_vms_created_and_dropped() {
    vm_churn(long(), "long_vm_churn");
}

// ---- 2. scripts loaded and unloaded in one VM ----

fn load_unload(b: Budget, name: &'static str) {
    let mut lua = Lua::new(Limits::default().memory(8 << 20), Libs::ALL - Libs::PRINT);
    lua.exec("function compile(src) return load(src)() end", "=setup").unwrap();
    let mut curve = Curve::new(name, b);
    let start = Instant::now();
    let base = gc_mem(&mut lua);
    let mut i: u64 = 0;
    while !b.done(i, start) {
        // A new chunk each time: new prototypes, new constants, new strings.
        let src = format!(
            "local t = {{}} for k = 1, 8 do t[k] = 'chunk{i}_' .. k end \
             local function f(x) return x + {i} end \
             return f(#t) + #t[8]"
        );
        let f = lua.load(&src, "=chunk").unwrap();
        let r = lua.call(f.value(), &[]).unwrap();
        assert_eq!(int(&r), 8 + i as i64 + format!("chunk{i}_8").len() as i64);
        drop(f);
        // And one compiled by the script itself with `load`.
        let s = lua.create_string(format!("return 'inner{i}', {i} * 2")).unwrap();
        let r = lua.call_global("compile", &[s]).unwrap().unwrap();
        assert_eq!(r.get(1), Some(&Value::Int(i as i64 * 2)));
        if i % 200 == 0 {
            let m = gc_mem(&mut lua);
            curve.sample(i, m);
        }
        i += 1;
    }
    let end = gc_mem(&mut lua);
    curve.sample(i, end);
    // Everything loaded is unreachable again: memory is back where it started.
    assert!(end <= base + 1024, "loaded chunks are not given back: {base} -> {end}");
    curve.assert_plateau();
}

#[test]
fn scripts_loaded_and_unloaded() {
    load_unload(normal(3000), "load_unload");
}

#[test]
#[ignore = "long soak: REMEDYLUA_SOAK_SECS"]
fn long_scripts_loaded_and_unloaded() {
    load_unload(long(), "long_load_unload");
}

// ---- 3. a long-lived script allocating and freeing ----

/// One step of churn: tables grown and shrunk, unique strings, closures, weak tables,
/// coroutines left suspended, a bounded ring of live objects. Returns a checksum.
///
/// Its closures share an upvalue (`i`) and its coroutine bodies inherit `_ENV`, which
/// trips stab-1; [`CHURN_SAFE`] is the same work written so that every upvalue a closure
/// captures is new, to show everything else plateaus.
const CHURN: &str = r#"
local weakk = setmetatable({}, {__mode = "k"})
local weakv = setmetatable({}, {__mode = "v"})
local keep, RING = {}, 128
local function churn(i)
  local t, n = {}, 50 + i % 400
  for j = 1, n do t[j] = j * 2 end
  for j = n, 1, -1 do t[j] = nil end
  local h = {}
  for j = 1, 40 do h["k" .. j .. "_" .. i] = j end
  local hs = 0
  for k, v in pairs(h) do hs = hs + v; h[k] = nil end
  local parts = {}
  for j = 1, 20 do parts[j] = string.format("%d:%s", i * j, string.rep("x", j % 7)) end
  local s = table.concat(parts, ",")
  local fs, acc = {}, 0
  for j = 1, 30 do local c = j; fs[j] = function(x) return x + c + i end end
  for j = 1, 30 do acc = acc + fs[j](j) end
  local obj = {i}
  weakk[obj] = s
  weakv[i % 1000] = {i}
  local co = coroutine.create(function(a)
    local big <close> = setmetatable({}, {__close = function() end})
    coroutine.yield(a)
    coroutine.yield(s)
    return 3
  end)
  local _, got = coroutine.resume(co, i)
  local w = coroutine.wrap(function() for k = 1, 10 do coroutine.yield(k) end end)
  acc = acc + w() + w() + got
  if i % 3 == 0 then coroutine.close(co) end
  keep[i % RING + 1] = {s, fs[1], co, w}
  return acc + #s + hs
end
return churn
"#;

/// [`CHURN`] without shared or inherited upvalues (library functions passed as
/// arguments instead), so stab-1 does not apply.
const CHURN_SAFE: &str = r#"
local weakk = setmetatable({}, {__mode = "k"})
local weakv = setmetatable({}, {__mode = "v"})
local keep, RING = {}, 128
local function churn(i)
  local t, n = {}, 50 + i % 400
  for j = 1, n do t[j] = j * 2 end
  for j = n, 1, -1 do t[j] = nil end
  local h = {}
  for j = 1, 40 do h["k" .. j .. "_" .. i] = j end
  local hs = 0
  for k, v in pairs(h) do hs = hs + v; h[k] = nil end
  local parts = {}
  for j = 1, 20 do parts[j] = string.format("%d:%s", i * j, string.rep("x", j % 7)) end
  local s = table.concat(parts, ",")
  local fs, acc = {}, 0
  for j = 1, 30 do local c = j + i; fs[j] = function(x) return x + c end end
  for j = 1, 30 do acc = acc + fs[j](j) end
  local obj = {i}
  weakk[obj] = s
  weakv[i % 1000] = {i}
  local co = coroutine.create(function(y, sm, a, s)
    local big <close> = sm({}, {__close = function() end})
    y(a)
    y(s)
    return 3
  end)
  local _, got = coroutine.resume(co, coroutine.yield, setmetatable, i, s)
  local w = coroutine.wrap(function(y) for k = 1, 10 do y(k) end end)
  acc = acc + w(coroutine.yield) + w() + got
  if i % 3 == 0 then coroutine.close(co) end
  keep[i % RING + 1] = {s, fs[1], co, w}
  return acc + #s + hs
end
return churn
"#;

fn churn_soak(b: Budget, name: &'static str, script: &str) {
    let mut lua = Lua::new(Limits::default().memory(16 << 20), Libs::STANDARD);
    let churn = lua.exec(script, "=churn").unwrap()[0];
    let churn = lua.root(churn);
    let mut curve = Curve::new(name, b);
    let start = Instant::now();
    let mut total: i64 = 0;
    let mut i: u64 = 1;
    while !b.done(i - 1, start) {
        let r = lua.call(churn.value(), &[Value::Int(i as i64)]).unwrap();
        total = total.wrapping_add(int(&r));
        if i % 100 == 0 {
            let m = gc_mem(&mut lua);
            curve.sample(i, m);
        }
        i += 1;
    }
    curve.assert_plateau();
    // The same steps in C Lua give the same checksum.
    if !b.is_long() {
        let src = format!("local churn = (function() {script} end)() local t = 0 for i = 1, {} do t = t + churn(i) end return t", i - 1);
        assert_eq!(remedy_lua_oracle::run(&src, u64::MAX >> 2), Ok(total.to_string()), "checksum differs from C Lua");
    }
}

#[test]
fn long_lived_script_churn_plateaus() {
    churn_soak(normal(1500), "churn_safe", CHURN_SAFE);
}

#[test]
#[ignore = "long soak: REMEDYLUA_SOAK_SECS"]
fn long_long_lived_script_churn_plateaus() {
    churn_soak(long(), "long_churn_safe", CHURN_SAFE);
}

/// The same churn written the ordinary way creeps because of stab-1.
#[test]
fn long_lived_script_churn_with_shared_upvalues_plateaus() {
    churn_soak(normal(1500), "churn", CHURN);
}

/// sweep1-stab-1 (critical, memory-account leak / creep): `make_closure` (`vm.rs:2158`)
/// charges `FUNC_BASE + n * (8 + UPVAL_SIZE)` for every upvalue of the new closure, but
/// a new upvalue object (the only thing the sweep later releases `UPVAL_SIZE` for,
/// `heap.rs:1010`) is made only when `find_upval` finds no open upvalue for that slot
/// (`vm.rs:742-755`). Closures that share an open upvalue (several closures over the same
/// local) or inherit one from the enclosing function (anything that uses a global inherits
/// `_ENV`) are charged 40 bytes that are never given back. `memory_used` climbs forever
/// in a script whose live data is constant, until every allocation fails with
/// `Error::Limit(Memory)`: a theme making a few hundred callbacks a frame dies within
/// minutes. Expected: 2000 calls in 1 MiB succeed and memory returns to the start;
/// actual: `Error::Limit(Memory)` at call 430 (59 phantom upvalue charges per call). Fix: charge
/// `FUNC_BASE + n * 8` in `make_closure` and `UPVAL_SIZE` in `find_upval`'s `Err` branch
/// only (the chunk upvalue in `lib.rs:453` is made, so it is right).
#[test]
fn closures_sharing_upvalues_do_not_creep() {
    let mut lua = Lua::new(Limits::default().memory(1 << 20), Libs::STANDARD);
    let f = lua
        .exec(
            "return function(i) local fs = {} for j = 1, 30 do fs[j] = function() return i + #string end end return #fs end",
            "=t",
        )
        .unwrap()[0];
    let f = lua.root(f);
    let base = gc_mem(&mut lua);
    for i in 0..2000 {
        let r = lua.call(f.value(), &[Value::Int(i)]);
        assert_eq!(r, Ok(vec![Value::Int(30)]), "call {i}, memory {}", lua.memory_used());
    }
    let end = gc_mem(&mut lua);
    assert!(end <= base + 256, "memory crept {base} -> {end}");
}

/// The same churn in C Lua: its `collectgarbage("count")` after a full collection at
/// thirds of the run, for comparison with RemedyLua's curve.
#[test]
fn churn_plateaus_in_c_lua_too() {
    let src = format!(
        "local churn = (function() {CHURN} end)() local out = {{}} \
         for i = 1, 1500 do churn(i) if i % 500 == 0 then collectgarbage() collectgarbage() \
         out[#out + 1] = string.format('%d', collectgarbage('count') * 1024) end end \
         return table.concat(out, ',')"
    );
    let c = remedy_lua_oracle::run(&src, u64::MAX >> 2).unwrap();
    println!("SOAK_C,churn,{c}");
    let v: Vec<i64> = c.split(',').map(|x| x.parse().unwrap()).collect();
    assert!(v[2] <= v[1] + v[1] / 50 + (32 << 10), "C Lua creeps too: {v:?}");
}

// ---- 4. limits hit thousands of times ----

const ATTACKS: [(&str, LimitKind); 9] = [
    ("while true do end", LimitKind::Fuel),
    ("local s = 'x' while true do s = s .. s end", LimitKind::Memory),
    ("local t = {} while true do t[#t + 1] = {} end", LimitKind::Memory),
    ("pcall(function() while true do end end) return 'escaped'", LimitKind::Fuel),
    ("local ok = pcall(function() local t = {} while true do t[#t + 1] = 'v' .. #t end end) return ok", LimitKind::Memory),
    ("local co = coroutine.wrap(function() while true do end end) co()", LimitKind::Fuel),
    ("local t = {3, 2, 1} table.sort(t, function(a, b) while true do end end)", LimitKind::Fuel),
    (
        "do local x <close> = setmetatable({}, {__close = function() local s = 'y' while true do s = s .. s end end}) end",
        LimitKind::Memory,
    ),
    // (No globals inside the closures: a closure inheriting `_ENV` trips stab-1.)
    ("xpcall(function() local e = 1 + {} end, function() while true do end end)", LimitKind::Fuel),
];

/// A fixed workload whose result and fuel use must never change.
const PROBE: &str = "local s = 0 for i = 1, 1000 do s = s + i * i end \
     local t = {} for i = 1, 100 do t[i] = tostring(i * 7) end table.sort(t) \
     local m = setmetatable({}, {__index = function(_, k) return k * 2 end}) \
     return s + #table.concat(t) + m[21] + #string.format('%5.2f|%q', 3.14159, 'a\\nb')";

fn limits_repeated(b: Budget, name: &'static str) {
    let mut lua = Lua::new(Limits::default().fuel(300_000).memory(2 << 20), Libs::STANDARD);
    let host_cb = lua
        .create_function("spin", |lua, a| {
            // A host function that calls back into a runaway script.
            lua.call(a[0], &[])
        })
        .unwrap();
    lua.set_global("spin", host_cb).unwrap();
    let attacks: Vec<_> = ATTACKS.iter().map(|(src, k)| (lua.load(src, "=attack").unwrap(), *k)).collect();
    let host_attack = (lua.load("spin(function() while true do end end)", "=hostattack").unwrap(), LimitKind::Fuel);
    let probe = lua.load(PROBE, "=probe").unwrap();
    let mut base = usize::MAX;
    gc_mem(&mut lua);
    let want = lua.call(probe.value(), &[]).unwrap();
    let want = int(&want);
    let probe_fuel = fuel_used(&lua);
    let mut curve = Curve::new(name, b);
    let start = Instant::now();
    let mut i: u64 = 0;
    while !b.done(i, start) {
        let (f, kind) = if i % 10 == 9 { &host_attack } else { &attacks[(i % 10) as usize % attacks.len()] };
        let r = lua.call(f.value(), &[]);
        assert_eq!(r, Err(Error::Limit(*kind)), "attack {} (step {i})", i % 10);
        // Afterwards: memory back to the start, the probe exact, its fuel identical.
        let m = gc_mem(&mut lua);
        // The first round grows stacks once; from then on memory is back to that each time.
        if i == 9 {
            base = m;
        }
        assert!(m <= base, "step {i}: memory after a limit {m} > base {base}");
        let r = lua.call(probe.value(), &[]).unwrap();
        assert_eq!(int(&r), want, "step {i}: wrong result after a limit");
        assert_eq!(fuel_used(&lua), probe_fuel, "step {i}: probe fuel changed after a limit");
        // Fresh compiles still work too.
        assert_eq!(int(&lua.exec("return 6 * 7", "=t").unwrap()), 42);
        if i % 50 == 0 {
            curve.sample(i, m);
        }
        i += 1;
    }
    curve.sample(i, gc_mem(&mut lua));
    curve.assert_plateau();
}

#[test]
fn limits_hit_repeatedly_vm_stays_exact() {
    limits_repeated(normal(300), "limits");
}

#[test]
#[ignore = "long soak: REMEDYLUA_SOAK_SECS"]
fn long_limits_hit_repeatedly_vm_stays_exact() {
    limits_repeated(long(), "long_limits");
}

// ---- 5. host functions, userdata and roots in cycles ----

fn host_cycles(b: Budget, name: &'static str) {
    let token = Arc::new(());
    let mut lua = Lua::new(Limits::default().memory(8 << 20), Libs::STANDARD);
    let meta = lua.create_table().unwrap();
    let meta = lua.root(Value::Table(meta));
    let Value::Table(mt) = meta.value() else { unreachable!() };
    lua.exec("function check(f, u) return f(u) == u end", "=setup").unwrap();
    let base = gc_mem(&mut lua);
    let mut curve = Curve::new(name, b);
    let start = Instant::now();
    let mut i: u64 = 0;
    while !b.done(i, start) {
        let t = Arc::clone(&token);
        let f = lua
            .create_function("echo", move |_, a| {
                let _held = &t;
                Ok(a.to_vec())
            })
            .unwrap();
        let rf = lua.root(f);
        let ud = lua.create_userdata(Payload(Arc::clone(&token)), Some(mt)).unwrap();
        let ru = lua.root(ud);
        let r = lua.call_global("check", &[rf.value(), ru.value()]).unwrap().unwrap();
        assert_eq!(r, vec![Value::Bool(true)]);
        // Registered under a rotating name: the replaced one becomes garbage.
        let t2 = Arc::clone(&token);
        lua.register(&format!("lib.f{}", i % 8), move |_, _| {
            let _held = &t2;
            Ok(vec![Value::Int(1)])
        })
        .unwrap();
        // Values crossing as data, kept only by a global that is overwritten.
        let d = lua.from_data(&Data::Table(vec![(Data::Str(format!("k{i}")), Data::Int(i as i64))])).unwrap();
        lua.set_global("last", d).unwrap();
        drop((rf, ru));
        if i % 200 == 0 {
            let m = gc_mem(&mut lua);
            curve.sample(i, m);
            // Live: the 8 registered functions only.
            assert_eq!(Arc::strong_count(&token), 1 + 8.min(i as usize + 1), "step {i}: host data not freed");
        }
        i += 1;
    }
    lua.exec("lib = nil last = nil", "=clear").unwrap();
    let end = gc_mem(&mut lua);
    curve.sample(i, end);
    assert_eq!(Arc::strong_count(&token), 1, "host data still held after everything was dropped");
    assert!(end <= base + 1024, "host objects not given back: {base} -> {end}");
    curve.assert_plateau();
}

#[test]
fn host_objects_created_and_released() {
    host_cycles(normal(2000), "host");
}

#[test]
#[ignore = "long soak: REMEDYLUA_SOAK_SECS"]
fn long_host_objects_created_and_released() {
    host_cycles(long(), "long_host");
}

// ---- 6. determinism ----

/// Order-independent work (sums over `pairs`, gc churn, formatting): the same result and
/// fuel in every run and every VM.
const DETERMINISTIC: &str = r#"
local t, sum = {}, 0
for i = 1, 300 do t["key" .. i] = i; t[i] = {i, tostring(i)} end
for k, v in pairs(t) do if type(v) == "number" then sum = sum + v else sum = sum + v[1] end end
local s = {}
for i = 1, 200 do s[#s + 1] = string.format("%x-%g", i * 31, i / 7) end
table.sort(s)
local co = coroutine.wrap(function() for i = 1, 50 do coroutine.yield(i) end end)
for _ = 1, 50 do sum = sum + co() end
collectgarbage()
return sum .. ":" .. s[1] .. s[#s] .. ":" .. #table.concat(s)
"#;

#[test]
fn deterministic_output_and_fuel_across_runs_and_vms() {
    let mut seen: Option<(String, u64)> = None;
    let mut lua = Lua::new(Limits::default(), Libs::STANDARD);
    for vm in 0..20 {
        if vm % 2 == 0 {
            lua = Lua::new(Limits::default(), Libs::STANDARD);
        }
        let f = lua.load(DETERMINISTIC, "=det").unwrap();
        lua.collect_garbage().unwrap();
        let r = lua.call(f.value(), &[]).unwrap();
        let got = (lua.to_string_lossy(r[0]).unwrap(), fuel_used(&lua));
        assert_eq!(seen.get_or_insert_with(|| got.clone()), &got, "run {vm} differs");
    }
    assert_eq!(remedy_lua_oracle::run(DETERMINISTIC, u64::MAX >> 2), Ok(seen.unwrap().0));
}

/// The order `pairs` visits keys is the same in every VM given the same `hash_seed`; by
/// default each VM seeds its table hashing at random (so scripts cannot aim keys at one
/// slot), and the order differs between VMs.
#[test]
fn pairs_order_same_in_every_vm_with_a_fixed_seed() {
    let src = "local t = {} for i = 1, 20 do t['k' .. i] = true end \
               local o = {} for k in pairs(t) do o[#o + 1] = k end return table.concat(o, ',')";
    let orders = |limits: Limits| {
        let mut orders = std::collections::HashSet::new();
        for _ in 0..10 {
            let mut lua = Lua::new(limits, Libs::STANDARD);
            let r = lua.exec(src, "=t").unwrap();
            orders.insert(lua.to_string_lossy(r[0]).unwrap());
        }
        orders.len()
    };
    assert_eq!(orders(Limits::default().hash_seed(7)), 1);
    assert!(orders(Limits::default()) > 1, "random seeds gave one pairs order over 10 VMs");
}

/// Fuel and results do not depend on the VM's random hash seed even when the script
/// iterates string keys.
#[test]
fn fuel_independent_of_hash_seed() {
    let src = "local t = {} for i = 1, 500 do t['k' .. i] = i end \
               local n = 0 for k, v in pairs(t) do n = n + v end \
               for i = 1, 500, 2 do t['k' .. i] = nil end \
               for i = 1, 500 do t['z' .. i] = i end return n";
    let mut fuels = std::collections::HashSet::new();
    for _ in 0..10 {
        let mut lua = Lua::new(Limits::default(), Libs::STANDARD);
        let f = lua.load(src, "=t").unwrap();
        let r = lua.call(f.value(), &[]).unwrap();
        assert_eq!(int(&r), 125_250);
        fuels.insert(fuel_used(&lua));
    }
    assert_eq!(fuels.len(), 1, "fuel use varies between VMs: {fuels:?}");
}

// ---- 7. emergency collections at the cap ----

fn emergency(b: Budget, name: &'static str) {
    let mut lua = Lua::new(Limits::default().fuel(5_000_000).memory(1 << 20), Libs::STANDARD);
    // Each iteration makes a ~300 KB string and drops the last: only collection at the cap
    // can make room fast enough.
    let fill = lua
        .load(
            "local n = ... local keep = 0 \
             for i = 1, 12 do local s = string.rep('x', 300000 + n * 13 + i) keep = keep + #s end \
             return keep",
            "=fill",
        )
        .unwrap();
    // A live set over the cap: must fail with Memory, not succeed or panic.
    let over = lua
        .load("local a = string.rep('a', 400000) local b = string.rep('b', 400000) local c = string.rep('c', 400000) return #a + #b + #c", "=over")
        .unwrap();
    let mut base = usize::MAX;
    let mut curve = Curve::new(name, b);
    let start = Instant::now();
    let mut i: u64 = 0;
    while !b.done(i, start) {
        let n = (i % 1000) as i64;
        let r = lua.call(fill.value(), &[Value::Int(n)]).unwrap();
        assert_eq!(int(&r), (0..12).map(|k| 300_000 + n * 13 + k + 1).sum::<i64>(), "step {i}");
        if i % 5 == 4 {
            assert_eq!(lua.call(over.value(), &[]), Err(Error::Limit(LimitKind::Memory)), "step {i}");
        }
        if i % 10 == 0 {
            let m = gc_mem(&mut lua);
            if i == 10 {
                base = m;
            }
            assert!(m <= base, "step {i}: {m} > {base}");
            curve.sample(i, m);
        }
        i += 1;
    }
    curve.sample(i, gc_mem(&mut lua));
    curve.assert_plateau();
}

#[test]
fn emergency_collections_at_the_cap() {
    emergency(normal(80), "emergency");
}

#[test]
#[ignore = "long soak: REMEDYLUA_SOAK_SECS"]
fn long_emergency_collections_at_the_cap() {
    emergency(long(), "long_emergency");
}

// ---- findings: host API patterns that keep garbage alive ----

/// sweep1-stab-2 (major, degraded-after-use): every `from_data` result is pushed onto
/// `rets` (`lib.rs:736`, `self.rets.push(v)`), which is a GC root (`vm.rs:478`) and is
/// cleared only by the next `Lua::call`. A host that feeds events in with `from_data`
/// and stores them (`set_global`/`table_set`) without calling a function in between keeps
/// every one alive until the cap. Expected: only the latest value is reachable, the loop
/// runs forever in 1 MiB; actual: `Error::Limit(Memory)` after a few thousand events.
/// Fix: clear `rets` at the start of every host entry (`enter` with `entry == 0`), or
/// keep only the most recent `from_data` result.
#[test]
fn from_data_without_calls_does_not_pile_up() {
    let mut lua = Lua::new(Limits::default().memory(1 << 20), Libs::STANDARD);
    for i in 0..50_000 {
        let d = Data::Table(vec![(Data::Str("payload".into()), Data::Str(format!("event number {i} with some text")))]);
        let v = lua.from_data(&d).unwrap_or_else(|e| panic!("event {i}: {e:?}"));
        lua.set_global("event", v).unwrap();
    }
}

/// sweep1-stab-3 (major, degraded-after-use): a host function that calls back into Lua
/// keeps every collectable result of every callback alive until it returns
/// (`vm.rs:1124-1127`: `self.scratch.extend(out...)` in `call_nested` when
/// `native_depth > 0`). A host "for each item, call the script's handler" loop over many
/// items whose handler returns a fresh table fails with a memory error although each
/// result is garbage the moment the next call starts. Expected: 100 000 callbacks in
/// 2 MiB succeed; actual: `Error::Limit(Memory)`. Fix: give the host function a way to
/// drop results (e.g. truncate scratch back to the caller's mark when the next nested
/// call starts, since the host got owned `Vec<Value>`s and roots what it keeps).
#[test]
fn host_callback_loop_does_not_pile_up_results() {
    let mut lua = Lua::new(Limits::default().fuel(50_000_000).memory(2 << 20), Libs::STANDARD);
    lua.register("each", |lua, a| {
        let mut n = 0;
        for i in 0..100_000 {
            let r = lua.call(a[0], &[Value::Int(i)])?;
            if matches!(r.first(), Some(Value::Table(_))) {
                n += 1;
            }
        }
        Ok(vec![Value::Int(n)])
    })
    .unwrap();
    let r = lua.exec("return each(function(i) return {i, 'item'} end)", "=t");
    assert_eq!(r, Ok(vec![Value::Int(100_000)]));
}

/// The same pattern for strings a host function makes itself (`hold` in `vm.rs:407`):
/// temporary strings made in a loop stay alive until the host function returns.
#[test]
#[ignore = "sweep1: stab-3 by design: what a host function makes stays alive until it returns (see sweep1-report.md)"]
fn host_temporary_strings_do_not_pile_up() {
    let mut lua = Lua::new(Limits::default().memory(2 << 20), Libs::STANDARD);
    lua.register("build", |lua, _| {
        let mut last = Value::Nil;
        for i in 0..100_000 {
            last = lua.create_string(format!("temporary string number {i}"))?;
        }
        Ok(vec![last])
    })
    .unwrap();
    let r = lua.exec("return #build()", "=t");
    assert_eq!(r, Ok(vec![Value::Int("temporary string number 99999".len() as i64)]));
}

/// Roots dropped by the host were forgotten only during a collection (fixed: `root()`
/// prunes dropped handles whenever the list doubles); a
/// host that roots and drops values without the script allocating grows `roots` (24 bytes
/// each, not counted in `memory_used`). Checked through process memory, long-only.
#[test]
#[ignore = "long soak: process memory"]
fn long_roots_without_allocation_do_not_grow() {
    let mut lua = Lua::new(Limits::default(), Libs::STANDARD);
    let t = lua.create_table().unwrap();
    let keep = lua.root(Value::Table(t));
    let before = proc_mem().unwrap();
    for _ in 0..4_000_000 {
        drop(lua.root(keep.value()));
    }
    let after = proc_mem().unwrap();
    println!("SOAK,roots,0,4000000,{},{after}", lua.memory_used());
    assert!(after <= before + (16 << 20), "dropped roots pile up: {before} -> {after}");
}

/// sweep1-stab-5 (critical, hang the fuel limit does not stop): copying varargs is not
/// charged. `local function f(...) return f(1, ...) end` grows its argument list by one
/// per tail call and copies all of it each time (`Instr::VarArg` with `MULTI`,
/// `vm.rs:2096-2106`; `tail_call`'s `copy_within`, `vm.rs:2141`; the vararg move in
/// `precall`, `vm.rs:877-884`), while each call costs a handful of fuel. Work is
/// quadratic in fuel: measured (debug) 10k fuel 0.1 s, 20k 0.37 s, 40k 1.5 s, 80k 5.7 s,
/// so the default 10M fuel (capped by the 1M stack slots) runs for hours. Expected:
/// 160 000 fuel stops in well under a second; actual: about 23 s in a debug build. Fix:
/// charge fuel per value copied in `VarArg`, `tail_call`, `precall`'s vararg move (and
/// any other O(n) copy: `table.unpack`, `select`, multiple returns).
#[test]
fn vararg_growth_is_paid_for_in_fuel() {
    let mut lua = Lua::new(Limits::default().fuel(160_000), Libs::STANDARD);
    let start = Instant::now();
    let r = lua.exec("local function f(...) return f(1, ...) end return f()", "=wide");
    assert_eq!(r, Err(Error::Limit(LimitKind::Fuel)));
    assert!(start.elapsed() < Duration::from_secs(5), "160k fuel took {:?}", start.elapsed());
}

/// sweep1-stab-6 (major, degraded-after-limit): a thread's stack only grows
/// (`ensure_stack`, `vm.rs:355-369`) and is never shrunk; its capacity stays charged to
/// `memory_used`. One deep recursion (or the vararg growth of stab-5, which reaches the
/// 1M `stack_slots`, 16 MiB) leaves the VM with that much less memory for the rest of its
/// life, after the call is over and a full collection has run. C Lua shrinks stacks in
/// the collector (`luaD_shrinkstack`). Expected: after a 1900-deep recursion and a
/// collection, memory is back near the start; actual: 27 745 -> 158 817 bytes for good.
/// Fix: in `full_gc`/`end_cycle`, shrink an idle thread's stack to what it uses (plus
/// headroom) and release the difference.
#[test]
fn stack_shrinks_after_deep_recursion() {
    let mut lua = Lua::new(Limits::default(), Libs::STANDARD);
    lua.exec("return 1", "=warm").unwrap();
    let base = gc_mem(&mut lua);
    let r = lua.exec("local function f(n) if n == 0 then return 0 end return 1 + f(n - 1) end return f(1900)", "=deep");
    assert_eq!(r, Ok(vec![Value::Int(1900)]));
    let after = gc_mem(&mut lua);
    assert!(after <= base + (8 << 10), "stack kept after deep recursion: {base} -> {after}");
}

/// A curve keeps at most `CURVE_CAP` samples however long the run (it kept every one:
/// a two-minute `long_host` grew the process 8 MB in doubling steps with the VM's memory
/// flat, and a longer one would have failed its own process check), and merging keeps each
/// stretch's peak, so the plateau check sees every spike it saw before.
#[test]
fn soak_curve_is_bounded_and_keeps_peaks() {
    let mut c = Curve::new("bounded", normal(1));
    let n = 1_000_000u64;
    for i in 0..n {
        // Flat at 1,000 with one spike per third: 5,000, 7,000, 9,000.
        let m = match i {
            300_001 => 5_000,
            600_001 => 7_000,
            999_998 => 9_000,
            _ => 1_000,
        };
        c.sample(i, m);
    }
    let all = c.all();
    assert!(all.len() <= CURVE_CAP, "{} samples", all.len());
    assert!(all.len() >= CURVE_CAP / 4, "{} samples", all.len());
    let peak = |from: u64, to: u64| all.iter().filter(|s| s.0 >= from && s.0 < to).map(|s| s.1).max().unwrap_or(0);
    assert_eq!(peak(300_001, 300_001 + 2 * n / CURVE_CAP as u64), 5_000);
    assert_eq!(peak(600_001, 600_001 + 2 * n / CURVE_CAP as u64), 7_000);
    assert_eq!(all.iter().map(|s| s.1).max(), Some(9_000));
    assert_eq!(all.last().map(|s| s.0), Some(n - 1));
}
