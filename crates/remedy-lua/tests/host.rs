//! The host API for engines: interrupts and time limits that stop any running call from
//! outside, tracked userdata the host hears about when collected, per-state app data, and
//! weak tables that behave as in C Lua.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

use super::common;

/// Programs that run for a very long time in different places: the interpreter loop, a
/// library function working (gsub, sort) and coroutines.
const LONG: &[(&str, &str)] = &[
    ("loop", "local n = 0 while true do n = n + 1 end"),
    ("gsub", "local s = string.rep('a', 200000) local r = s:gsub('a*b', '') return r"),
    (
        "sort",
        "local t = {} for i = 1, 200000 do t[i] = (i * 7919) % 200000 end \
         for round = 1, 1000 do table.sort(t, function(a, b) return a > b end) table.sort(t) end",
    ),
    (
        "coroutine",
        "local co = coroutine.wrap(function() while true do coroutine.yield(1) end end) while true do co() end",
    ),
];

fn big() -> Limits {
    Limits::default().fuel(u64::MAX >> 4).memory(256 << 20)
}

/// Runs `src` while another thread interrupts it after `after`: the error and how long the
/// stop took past the interrupt.
fn interrupted(src: &str, fast: bool, after: Duration) -> (Result<Vec<Value>, Error>, Duration, Lua) {
    let mut lua = Lua::new(big(), Libs::ALL);
    lua.set_fast_loop(fast);
    let handle = lua.interrupt_handle();
    let at = Arc::new(Mutex::new(None::<Instant>));
    let at2 = at.clone();
    let stopper = std::thread::spawn(move || {
        std::thread::sleep(after);
        if let Ok(mut a) = at2.lock() {
            *a = Some(Instant::now());
        }
        handle.interrupt();
    });
    let r = lua.exec(src, "=t");
    let done = Instant::now();
    let _ = stopper.join();
    let at = at.lock().ok().and_then(|a| *a).unwrap_or(done);
    (r, done.saturating_duration_since(at), lua)
}

#[test]
fn an_interrupt_stops_any_running_call_promptly() {
    for fast in [true, false] {
        for (name, src) in LONG {
            let (r, late, mut lua) = interrupted(src, fast, Duration::from_millis(30));
            assert_eq!(r.as_ref().err(), Some(&Error::Limit(LimitKind::Interrupted)), "{name} (fast {fast})");
            assert!(
                late < Duration::from_millis(500),
                "{name} (fast {fast}) stopped {late:?} after the interrupt"
            );
            // Interrupted calls stay stopped until the host resets the interrupt.
            assert_eq!(lua.exec("return 1", "=t").err(), Some(Error::Limit(LimitKind::Interrupted)));
            lua.interrupt_handle().reset();
            let v = lua.exec("local t = {} for i = 1, 100 do t[i] = i end return #t", "=t");
            assert!(matches!(v.as_deref(), Ok([Value::Int(100)])), "{name}: usable after: {v:?}");
        }
    }
}

#[test]
fn scripts_cannot_catch_an_interrupt() {
    let src = "local caught = 0 \
               while true do local ok = pcall(function() while true do end end) if not ok then caught = caught + 1 end end";
    let (r, _, _) = interrupted(src, true, Duration::from_millis(20));
    assert_eq!(r.err(), Some(Error::Limit(LimitKind::Interrupted)));
}

#[test]
fn a_time_limit_stops_calls_by_the_hosts_clock() {
    for fast in [true, false] {
        let mut lua = Lua::new(big(), Libs::ALL);
        lua.set_fast_loop(fast);
        let start = Instant::now();
        let clock: remedy_lua::Clock = Arc::new(move || start.elapsed());
        lua.set_time_limit(Some((clock, Duration::from_millis(40))));
        for (name, src) in LONG {
            let t = Instant::now();
            let r = lua.exec(src, "=t");
            assert_eq!(r.err(), Some(Error::Limit(LimitKind::Time)), "{name}");
            assert!(t.elapsed() < Duration::from_millis(600), "{name}: {:?}", t.elapsed());
        }
        // Each call gets the whole limit again, and a short call is untouched.
        let v = lua.exec("return 2 + 2", "=t");
        assert!(matches!(v.as_deref(), Ok([Value::Int(4)])), "{v:?}");
        lua.set_time_limit(None);
    }
}

#[test]
fn slices_keep_fuel_exact() {
    // The budget is handed out in slices, but a call stops at the very step it runs out.
    for fast in [true, false] {
        for n in [1u64, 2, 100, (1 << 14) - 1, 1 << 14, (1 << 14) + 1, 100_000] {
            let mut lua = Lua::new(Limits::default().fuel(n), Libs::ALL);
            lua.set_fast_loop(fast);
            let r = lua.exec("local i = 0 while true do i = i + 1 end", "=t");
            assert_eq!(r.err(), Some(Error::Limit(LimitKind::Fuel)));
            assert_eq!(lua.fuel_left(), 0);
            // A program that fits uses the same fuel whatever the slices.
            let mut lua = Lua::new(Limits::default().fuel(1_000_000), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec("local s = 0 for i = 1, 30000 do s = s + i end return s", "=t")
                .map(|_| ())
                .unwrap_or_default();
            let used = 1_000_000 - lua.fuel_left();
            let mut again = Lua::new(Limits::default().fuel(used), Libs::ALL);
            again.set_fast_loop(fast);
            assert!(
                again.exec("local s = 0 for i = 1, 30000 do s = s + i end return s", "=t").is_ok(),
                "{used}"
            );
            let mut short = Lua::new(Limits::default().fuel(used - 1), Libs::ALL);
            short.set_fast_loop(fast);
            assert_eq!(
                short.exec("local s = 0 for i = 1, 30000 do s = s + i end return s", "=t").err(),
                Some(Error::Limit(LimitKind::Fuel))
            );
        }
    }
}

#[test]
fn the_host_hears_which_tracked_userdata_were_collected() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    for key in 1..=10u64 {
        let v = lua.create_userdata_tracked(key, None, key).unwrap_or(Value::Nil);
        let k = lua.create_string(format!("h{key}")).unwrap_or(Value::Nil);
        lua.table_set(lua.globals(), k, v).unwrap_or_default();
    }
    // An untracked one is never reported.
    let plain = lua.create_userdata(0u64, None).unwrap_or(Value::Nil);
    lua.set_global("plain", plain).unwrap_or_default();
    // Scripts drop the odd ones (one held only weakly, and one kept inside a closure).
    lua.exec(
        "local weak = setmetatable({}, { __mode = 'v' }) weak[1] = h3 \
         local held = h5 keep = function() return held end \
         h1, h3, h5, h7, h9, plain = nil, nil, nil, nil, nil, nil",
        "=t",
    )
    .unwrap_or_default();
    lua.collect_garbage().unwrap_or_default();
    let mut got = lua.take_collected();
    got.sort_unstable();
    assert_eq!(got, [1, 3, 7, 9], "h5 lives on in a closure");
    assert!(lua.take_collected().is_empty(), "each is reported once");
    lua.exec("keep = nil", "=t").unwrap_or_default();
    lua.collect_garbage().unwrap_or_default();
    assert_eq!(lua.take_collected(), [5]);
}

#[test]
fn app_data_is_typed_and_reachable_from_host_functions() {
    struct Counter(u32);
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    lua.set_app_data(Counter(0));
    lua.set_app_data(String::from("label"));
    lua.register("bump", |lua, _| {
        let c = lua.app_data_mut::<Counter>().ok_or_else(|| Error::runtime("no counter"))?;
        c.0 += 1;
        Ok(vec![Value::Int(i64::from(c.0))])
    })
    .unwrap_or_default();
    let v = lua.exec("bump() bump() return bump()", "=t");
    assert!(matches!(v.as_deref(), Ok([Value::Int(3)])), "{v:?}");
    assert_eq!(lua.app_data::<String>().map(String::as_str), Some("label"));
    assert!(lua.app_data::<u8>().is_none());
    assert_eq!(lua.take_app_data::<Counter>().map(|c| c.0), Some(3));
    assert!(lua.app_data::<Counter>().is_none());
}

/// Weak tables as C Lua has them: weak keys, weak values, both, ephemerons (a value
/// reachable only through its weak key goes with it), and strings, numbers and booleans,
/// which are values and never removed.
#[test]
fn weak_tables_behave_as_in_c_lua() {
    let prog = r#"
        local function count(t) local n = 0 for _ in pairs(t) do n = n + 1 end return n end
        local wk = setmetatable({}, { __mode = "k" })
        local wv = setmetatable({}, { __mode = "v" })
        local wkv = setmetatable({}, { __mode = "kv" })
        local keep = {}
        for i = 1, 10 do
          local k, v = {}, {}
          if i % 2 == 0 then keep[#keep + 1] = k keep[#keep + 1] = v end
          wk[k] = i
          wv[i] = v
          wkv[k] = v
        end
        -- Ephemerons: the value refers to its own key.
        local eph = setmetatable({}, { __mode = "k" })
        for i = 1, 5 do local k = {} eph[k] = { k } end
        -- Values that are not objects stay.
        wv.s = "a string" wv.n = 42 wv.b = true wk["str"] = 1 wk[3.5] = 2
        local f = function() end
        wv.f = f
        collectgarbage() collectgarbage()
        return count(wk), count(wv), count(wkv), count(eph), wv.s, wv.n, wv.b, wk.str, wk[3.5], wv.f == f
    "#;
    let ours = common::remedy(prog, 100_000_000);
    if let Ok(c) = common::oracle(prog, 100_000_000) {
        assert_eq!(ours, Ok(c));
    }
    assert!(ours.as_ref().is_ok_and(|s| !s.starts_with("error")), "{ours:?}");
}
