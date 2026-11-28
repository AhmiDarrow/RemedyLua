//! Host-compiled chunks (`Lua::compile` / `Lua::load_compiled`): the instantiated chunk
//! behaves exactly as `load` of the same source in the same state would.

use remedy_lua::{CompiledChunk, DataLimits, Error, Libs, LimitKind, Limits, Lua, Value};

// A long string constant (past the compiler's piecewise-interning size) goes through too.
const PRELUDE_HEAD: &str = r#"local long = "LONG"
"#;

fn prelude() -> String {
    PRELUDE_HEAD.replace("LONG", &"x".repeat(9000)) + PRELUDE
}

const PRELUDE: &str = r#"
local M = {}
local count = 0
function M.inc(n) count = count + (n or 1); return count end
function M.sum(...)
  local s = 0
  for _, v in ipairs({...}) do s = s + v end
  return s
end
function M.fmt(t)
  local keys = {}
  for k in pairs(t) do keys[#keys + 1] = tostring(k) end
  table.sort(keys)
  return table.concat(keys, ",")
end
M.cls = setmetatable({}, { __index = function(_, k) return "dyn:" .. k end })
local i = 0
::top:: i = i + 1
if i < 5 then goto top end
M.i = i
M.longlen = #long .. "!"
M.fact = function(n) if n <= 1 then return 1 end return n * M.fact(n - 1) end
M.boom = function() local t = nil; return t.field end
lookup = { alpha = 1, beta = 2 }
return M
"#;

const PROBE: &str = r#"
local M = ...
return { M.inc(), M.inc(4), M.sum(1, 2, 3.5), M.fmt({ z = 1, a = 2, [3] = 3 }), M.cls.hello,
         M.i, M.longlen, M.fact(10), lookup.alpha + lookup["be" .. "ta"] }
"#;

fn with(f: impl FnOnce(&mut Limits)) -> Limits {
    let mut l = Limits::default();
    f(&mut l);
    l
}

fn new(limits: Limits) -> Lua {
    Lua::new(limits, Libs::STANDARD | Libs::DEBUG_INFO)
}

/// Runs the prelude (given as a function) then the probe on its result: the probe's
/// result as text, and the fuel each of the two calls used.
fn run(lua: &mut Lua, prelude: Value, budget: u64) -> (String, [u64; 2]) {
    let m = lua.call(prelude, &[]).unwrap();
    let f0 = budget - lua.fuel_left();
    let probe = lua.load(PROBE, "=probe").unwrap();
    let r = lua.call(probe.value(), &m).unwrap();
    let f1 = budget - lua.fuel_left();
    let d = lua.to_data(r[0], DataLimits::default()).unwrap();
    (format!("{d:?}"), [f0, f1])
}

#[test]
fn same_results_and_fuel_as_load() {
    // A fixed hash seed (the default is random per state, and `pairs` order and probe
    // counts follow it), and a full collection first in both, so the compile's garbage in
    // the loading state does not shift the collector's pacing: then every unit is equal.
    let budget = 10_000_000;
    let chunk = new(Limits::default()).compile(prelude(), "=prelude").unwrap();
    for seed in 0..64 {
        let limits = with(|l| l.fuel = budget).hash_seed(seed);
        let mut a = new(limits);
        let f = a.load(prelude(), "=prelude").unwrap();
        a.collect_garbage().unwrap();
        let (want, want_fuel) = run(&mut a, f.value(), budget);

        let mut b = new(limits);
        let f = b.load_compiled(&chunk).unwrap();
        b.collect_garbage().unwrap();
        let (got, got_fuel) = run(&mut b, f.value(), budget);
        assert_eq!(got, want, "seed {seed}");
        assert_eq!(got_fuel, want_fuel, "seed {seed}: a compiled chunk runs on the fuel the loaded one does");
        assert!(got.contains("dyn:hello") && got.contains("3628800") && got.contains("9000!"), "{got}");
    }
}

#[test]
fn same_errors_and_debug_info_as_load() {
    let src = "local function f() error('nope') end\nlocal function g() local t; return t.x end\nreturn f, g";
    let mut a = new(Limits::default());
    let fa = a.load(src, "@lib/thing.lua").unwrap();
    let ra = a.call(fa.value(), &[]).unwrap();
    let chunk = new(Limits::default()).compile(src, "@lib/thing.lua").unwrap();
    let mut b = new(Limits::default());
    let fb = b.load_compiled(&chunk).unwrap();
    let rb = b.call(fb.value(), &[]).unwrap();
    for i in 0..2 {
        let ea = a.call(ra[i], &[]).unwrap_err().to_string();
        let eb = b.call(rb[i], &[]).unwrap_err().to_string();
        assert_eq!(eb, ea);
        assert!(eb.contains("lib/thing.lua:"), "{eb}");
    }
    let info = "local f = ...; local i = debug.getinfo(f, 'S'); return i.source .. '|' .. i.linedefined .. '|' .. i.lastlinedefined";
    let qa = a.load(info, "=q").unwrap();
    let qb = b.load(info, "=q").unwrap();
    let ia = a.call(qa.value(), &[ra[1]]).unwrap();
    let ib = b.call(qb.value(), &[rb[1]]).unwrap();
    assert_eq!(
        b.to_data(ib[0], DataLimits::default()).unwrap(),
        a.to_data(ia[0], DataLimits::default()).unwrap()
    );
}

#[test]
fn compile_errors_are_loads() {
    let mut lua = new(Limits::default());
    let ea = lua.load("x = = 1", "=bad").unwrap_err().to_string();
    let ec = lua.compile("x = = 1", "=bad").unwrap_err().to_string();
    assert_eq!(ec, ea);
    assert!(matches!(lua.compile("\x1bLua", "=bin"), Err(Error::Syntax(_))), "bytecode is still refused");
}

#[test]
fn instances_are_independent_and_strings_are_interned_per_state() {
    let chunk = new(Limits::default()).compile(prelude(), "=prelude").unwrap();
    let mut states: Vec<Lua> = (0..3).map(|_| new(Limits::default())).collect();
    let mut mods = Vec::new();
    for s in &mut states {
        let f = s.load_compiled(&chunk).unwrap();
        mods.push(s.call(f.value(), &[]).unwrap());
    }
    // Each state's counter is its own.
    let bump = "local M = ...; return M.inc(10)";
    let s0 = &mut states[0];
    let b = s0.load(bump, "=b").unwrap();
    s0.call(b.value(), &mods[0]).unwrap();
    let r = s0.call(b.value(), &mods[0]).unwrap();
    assert_eq!(r, vec![Value::Int(20)]);
    let s1 = &mut states[1];
    let b = s1.load(bump, "=b").unwrap();
    assert_eq!(s1.call(b.value(), &mods[1]).unwrap(), vec![Value::Int(10)]);
    // A constant key from the chunk finds a key the state made itself (same interned string).
    let s2 = &mut states[2];
    let r = s2.exec("lookup[('al'):rep(1) .. 'pha2'] = 7; return lookup.alpha, lookup.alpha2", "=k").unwrap();
    assert_eq!(r, vec![Value::Int(1), Value::Int(7)]);
}

#[test]
fn a_chunk_crosses_threads() {
    let chunk = new(Limits::default()).compile("return ... * 2", "=dbl").unwrap();
    let handles: Vec<_> = (0..4i64)
        .map(|i| {
            let c: CompiledChunk = chunk.clone();
            std::thread::spawn(move || {
                let mut lua = new(Limits::default());
                let f = lua.load_compiled(&c).unwrap();
                lua.call(f.value(), &[Value::Int(i)]).unwrap()
            })
        })
        .collect();
    for (i, h) in handles.into_iter().enumerate() {
        assert_eq!(h.join().unwrap(), vec![Value::Int(i as i64 * 2)]);
    }
}

#[test]
fn instantiation_is_charged_memory_and_the_state_survives() {
    let chunk = new(Limits::default()).compile(prelude(), "=prelude").unwrap();
    assert!(chunk.instance_bytes() > 9000, "the long string constant counts: {}", chunk.instance_bytes());
    let base = new(Limits::default()).memory_used();
    let tight = with(|l| l.memory = base + chunk.instance_bytes() / 2);
    let mut lua = new(tight);
    let before = lua.memory_used();
    let e = lua.load_compiled(&chunk).unwrap_err();
    assert!(matches!(e, Error::Limit(LimitKind::Memory)), "{e:?}");
    // What the refused instantiation made is garbage: nothing leaks.
    lua.collect_garbage().unwrap();
    assert_eq!(lua.memory_used(), before);
    // And the state is fine once it has room.
    lua.set_limits(Limits::default());
    let f = lua.load_compiled(&chunk).unwrap();
    assert!(lua.call(f.value(), &[]).is_ok());
    let mut roomy = new(Limits::default());
    let before = roomy.memory_used();
    let _f = roomy.load_compiled(&chunk).unwrap();
    assert!(roomy.memory_used() >= before + chunk.instance_bytes() / 2);
}

#[test]
fn instantiating_inside_a_call_is_charged_fuel() {
    let chunk = new(Limits::default()).compile(prelude(), "=prelude").unwrap();
    let cost = |fuel: u64| {
        let mut lua = new(with(|l| l.fuel = fuel));
        let c = chunk.clone();
        lua.register("inst", move |lua, _| {
            let f = lua.load_compiled(&c)?;
            Ok(vec![f.value()])
        })
        .unwrap();
        let before = lua.fuel_left();
        let r = lua.exec("inst()", "=c");
        (r, before.saturating_sub(lua.fuel_left()))
    };
    let (r, used) = cost(10_000_000);
    r.unwrap();
    assert!(used as usize >= chunk.instance_bytes() / 64, "copying the chunk is fuel: {used}");
    let (r, _) = cost(50);
    assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{r:?}");
}
