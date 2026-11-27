//! Sweep 4, area "host API and embedding": the public API as a careless embedder uses it.
//! Each test is a finding of this sweep. Every test runs with the fast loop
//! on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::panic::AssertUnwindSafe;

use remedy_lua::{Data, DataLimits, Error, Libs, Limits, Lua, Value};

fn states() -> impl Iterator<Item = Lua> {
    [true, false].into_iter().map(|fast| {
        let mut lua = Lua::new(Limits::default().fuel(10_000_000), Libs::ALL);
        lua.set_fast_loop(fast);
        lua
    })
}

/// Makes garbage (in a metamethod run by `get`, which is not a call from the host) and
/// caps memory just above what is used, so the host's next allocation runs a full
/// emergency collection.
fn squeeze(lua: &mut Lua, junk: Value) {
    lua.set_limits(Limits::default());
    lua.get(junk, Value::Int(1)).unwrap();
    let mut l = lua.limits();
    l.memory = lua.memory_used() + 16;
    lua.set_limits(l);
}

/// API-1: values the host makes outside a call must survive the host's next steps (the
/// natural set-up sequence), even when an allocation in between runs a full collection.
#[test]
fn api1_host_made_values_survive_until_the_next_call() {
    for mut lua in states() {
        lua.exec(
            "junk = setmetatable({}, {__index = function() for i = 1, 300 do local _ = {i} end end})",
            "=s",
        )
        .unwrap();
        let junk = lua.get_global("junk").unwrap();
        // set_global: the name's string is made after `t` was checked.
        let t = lua.create_table().unwrap();
        squeeze(&mut lua, junk);
        lua.set_global("api1_table_global_with_a_long_unique_name", Value::Table(t)).unwrap();
        // table_set with a fresh key and value, made apart.
        let k = lua.create_string("a key long enough not to be interned, made by the host").unwrap();
        squeeze(&mut lua, junk);
        let v = lua.create_string("a value long enough not to be interned, made by the host").unwrap();
        squeeze(&mut lua, junk);
        lua.table_set(t, k, v).unwrap();
        let ud = lua.create_userdata(7u32, None).unwrap();
        squeeze(&mut lua, junk);
        let f = lua.create_function("f", |_, _| Ok(vec![Value::Int(1)])).unwrap();
        squeeze(&mut lua, junk);
        let d = lua.from_data(&Data::Table(vec![(Data::Str("x".into()), Data::Int(1))])).unwrap();
        squeeze(&mut lua, junk);
        for (i, v) in [ud, f, d].into_iter().enumerate() {
            lua.table_set(t, Value::Int(i as i64 + 1), v).unwrap();
            squeeze(&mut lua, junk);
        }
        // An exec result kept as a global after making its name (a host's module loader).
        lua.set_limits(Limits::default());
        let out = lua.exec("return {n = 5}", "=m").unwrap();
        squeeze(&mut lua, junk);
        let k2 = lua.create_string("module_name_long_enough_to_need_its_own_allocation").unwrap();
        squeeze(&mut lua, junk);
        let g = lua.globals();
        lua.table_set(g, k2, out[0]).unwrap();
        lua.set_limits(Limits::default());
        let r = lua
            .exec(
                "local t = api1_table_global_with_a_long_unique_name
                 return t['a key long enough not to be interned, made by the host'], type(t[1]), t[2](), t[3].x,
                 module_name_long_enough_to_need_its_own_allocation.n",
                "=check",
            )
            .unwrap();
        assert_eq!(
            lua.to_string_lossy(r[0]).as_deref(),
            Some("a value long enough not to be interned, made by the host")
        );
        assert_eq!(lua.to_string_lossy(r[1]).as_deref(), Some("userdata"));
        assert!(matches!(r[2..], [Value::Int(1), Value::Int(1), Value::Int(5)]), "{r:?}");
        // Values held for the host go at the next call: memory returns to its baseline.
        let base = lua.memory_used();
        for _ in 0..1000 {
            lua.create_string(vec![b'x'; 1000]).unwrap();
        }
        assert!(lua.memory_used() > base + 900_000);
        lua.exec("return 1", "=t").unwrap();
        lua.collect_garbage().unwrap();
        assert!(lua.memory_used() < base + 10_000, "{} vs {base}", lua.memory_used());
    }
}

/// API-2: a host function that catches the panic of a nested call and returns normally
/// must not let the outer script run on in a poisoned state.
#[test]
fn api2_poison_inside_a_host_function_stops_the_outer_call() {
    for mut lua in states() {
        lua.register("boom", |_, _| panic!("host bug")).unwrap();
        lua.register("guard", |lua, a| {
            let f = a[0];
            let caught = std::panic::catch_unwind(AssertUnwindSafe(|| lua.call(f, &[])));
            Ok(vec![Value::Bool(caught.is_err())])
        })
        .unwrap();
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
            lua.exec("local t = {} for i = 1, 10 do t[i] = guard(function() local x = {1, 2, 3} boom() end) end return #t", "=t")
        }));
        match r {
            Ok(Err(Error::Host(_))) | Err(_) => {}
            other => panic!("outer call went on after the poison: {other:?}"),
        }
        assert!(matches!(lua.exec("return 1", "=after"), Err(Error::Host(_))));
    }
}

/// API-3: `Lua` moves between threads; handles and errors are plain data.
#[test]
fn api3_send_and_sync_bounds() {
    fn send<T: Send>() {}
    fn send_sync<T: Send + Sync>() {}
    send::<Lua>();
    send_sync::<remedy_lua::Interrupt>();
    send_sync::<Error>();
    send_sync::<Value>();
    send_sync::<remedy_lua::Root>();
    send_sync::<Data>();
    send_sync::<remedy_lua::Diagnostic>();
}

/// Probe: the data limits are honoured on the way out.
#[test]
fn api_data_limits() {
    for mut lua in states() {
        let v = lua.exec("return {1, 2, 3}", "=t").unwrap()[0];
        assert!(lua.to_data(v, DataLimits { depth: 32, nodes: 2, bytes: 100 }).is_err());
        assert!(lua.to_data(v, DataLimits::default()).is_ok());
    }
}

/// API-4: a host function that waits (spending no fuel) stops at the interrupt or the
/// deadline when it returns, without the host checking for itself.
#[test]
fn api4_host_function_return_checks_interrupt_and_deadline() {
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    for mut lua in states() {
        lua.set_limits(Limits::default().fuel(u64::MAX / 4));
        lua.register("wait", |_, _| {
            std::thread::sleep(Duration::from_millis(5));
            Ok(vec![])
        })
        .unwrap();
        let start = Instant::now();
        let clock: remedy_lua::Clock = Arc::new(move || start.elapsed());
        lua.set_time_limit(Some((clock, Duration::from_millis(40))));
        let t = Instant::now();
        let r = lua.exec("while true do pcall(wait) end", "=t");
        assert!(matches!(r, Err(Error::Limit(remedy_lua::LimitKind::Time))), "{r:?}");
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
        lua.set_time_limit(None);
        // The interrupt, set from the host function itself (as a watchdog would meanwhile).
        let stop = lua.interrupt_handle();
        lua.register("stop", move |_, _| {
            stop.interrupt();
            Ok(vec![])
        })
        .unwrap();
        let r = lua.exec("local ok = pcall(stop) return 'went on'", "=t");
        assert!(matches!(r, Err(Error::Limit(remedy_lua::LimitKind::Interrupted))), "{r:?}");
        lua.interrupt_handle().reset();
        assert!(lua.exec("return 1", "=t").is_ok());
    }
}

/// API-5: `set_limits` cannot change what is read only when the state is made, and
/// `limits()` keeps telling the truth about it.
#[test]
fn api5_set_limits_keeps_seed_and_strictness() {
    let mut lua = Lua::new(Limits::default().hash_seed(42), Libs::ALL);
    lua.set_limits(Limits::default().fuel(1000));
    assert_eq!(lua.limits().hash_seed, Some(42));
    assert_eq!(lua.limits().fuel, 1000);
    assert!(!lua.limits().strict_5_4);
    let l = Limits::default().stack_bytes(1 << 20).error_len(10).stack_slots(1 << 16);
    assert_eq!((l.stack_bytes, l.error_len, l.stack_slots), (1 << 20, 10, 1 << 16));
}

/// API-6: a poisoned state refuses `load` too (it compiles into the broken heap).
#[test]
fn api6_poisoned_state_refuses_load() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    lua.register("boom", |_, _| panic!("host bug")).unwrap();
    let _ = std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec("boom()", "=t")));
    assert!(matches!(lua.load("return 1", "=t"), Err(Error::Host(_))));
}

/// Probes of reentrancy: a host function that collects, reloads, or is crossed by a
/// yield; none may panic.
#[test]
fn api_reentrancy_probes() {
    for mut lua in states() {
        lua.register("gc", |lua, _| {
            lua.collect_garbage()?;
            Ok(vec![])
        })
        .unwrap();
        lua.register("run", |lua, a| {
            let src = lua.to_string_lossy(a[0]).unwrap_or_default();
            lua.exec(src, "=inner")
        })
        .unwrap();
        lua.register("callf", |lua, a| lua.call(a[0], &[])).unwrap();
        let r = lua.exec(
            "local t = {} for i = 1, 100 do t[i] = {i} end gc() local s = 0 for i = 1, 100 do s = s + t[i][1] end
             local a, b = run('return 1, 2')
             local co = coroutine.wrap(function() return callf(function() coroutine.yield(1) end) end)
             local ok, e = pcall(co)
             return s, a, b, ok, e",
            "=t",
        );
        let r = r.unwrap();
        assert!(
            matches!(r[..4], [Value::Int(5050), Value::Int(1), Value::Int(2), Value::Bool(false)]),
            "{r:?}"
        );
        assert!(lua.to_string_lossy(r[4]).unwrap().contains("yield"), "{:?}", lua.to_string_lossy(r[4]));
        // A yield from the host's own call, outside any coroutine.
        let r = lua.exec("coroutine.yield(1)", "=t");
        assert!(
            matches!(&r, Err(Error::Runtime { message, .. }) if message.contains("outside a coroutine")),
            "{r:?}"
        );
        assert!(lua.exec("return 1", "=t").is_ok());
    }
}

/// API-1 (inside a host function): a value from `from_data` must survive the host
/// function's later calls into Lua (a UI host's dispatcher: settings and host data made,
/// effects run, then render called with them).
#[test]
fn api1_from_data_survives_nested_calls_in_a_host_function() {
    for mut lua in states() {
        lua.exec(
            "function churn() local t = {} for i = 1, 2000 do t[i] = {i} end collectgarbage() end",
            "=s",
        )
        .unwrap();
        lua.register("dispatch", |lua, _| {
            let s = lua.from_data(&Data::Table(vec![(Data::Str("k".into()), Data::Int(7))]))?;
            let churn = lua.get_global("churn")?;
            lua.call(churn, &[])?;
            lua.call(churn, &[])?;
            Ok(vec![s])
        })
        .unwrap();
        let r = lua.exec("return dispatch().k", "=t");
        assert!(matches!(r.as_deref(), Ok([Value::Int(7)])), "{r:?}");
    }
}

/// API-9: a host function with mutable state that is `Send` but not `Sync` (a `Cell`),
/// without a `Mutex` of the host's own; reaching it again while it runs is a script error.
#[test]
fn api9_create_function_mut_needs_no_sync() {
    for mut lua in states() {
        let count = std::cell::Cell::new(0i64);
        let mut log = Vec::new();
        lua.register_mut("bump", move |lua, a| {
            count.set(count.get() + 1);
            log.push(count.get());
            if let Some(f) = a.first().copied().filter(|v| matches!(v, Value::Func(_))) {
                lua.call(f, &[])?;
            }
            Ok(vec![Value::Int(count.get()), Value::Int(log.len() as i64)])
        })
        .unwrap();
        let r = lua.exec("bump() local a, b = bump() local ok, e = pcall(bump, function() bump() end) return a, b, ok, e", "=t").unwrap();
        assert!(matches!(r[..3], [Value::Int(2), Value::Int(2), Value::Bool(false)]), "{r:?}");
        assert!(lua.to_string_lossy(r[3]).unwrap().contains("called again while it ran"));
        assert!(lua.exec("return bump()", "=t").is_ok());
    }
}

/// API-10: memory a userdata owns on the heap is charged once the host declares it, and
/// given back when it is collected; a declaration past the limit stops the call.
#[test]
fn api10_userdata_owned_size_is_charged() {
    for mut lua in states() {
        lua.set_limits(Limits::default().memory(8 << 20));
        lua.register("buffer", |lua, a| {
            let n = a.first().and_then(|v| v.as_int()).unwrap_or(0) as usize;
            let v = lua.create_userdata(Vec::<u8>::new(), None)?;
            lua.set_userdata_size(v, std::mem::size_of::<Vec<u8>>() + n)?;
            // Only now, with the budget charged, does the host allocate.
            if let Some(b) = lua.userdata_mut::<Vec<u8>>(v) {
                b.resize(n, 0);
            }
            Ok(vec![v])
        })
        .unwrap();
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        lua.exec("keep = buffer(1000000)", "=t").unwrap();
        assert!(lua.memory_used() >= base + 1_000_000, "{} vs {base}", lua.memory_used());
        lua.exec("keep = nil", "=t").unwrap();
        lua.collect_garbage().unwrap();
        assert!(lua.memory_used() < base + 10_000, "{} vs {base}", lua.memory_used());
        // Asked for in a loop under pcall: stopped by the memory limit, uncatchably.
        let r = lua
            .exec("local t = {} while true do pcall(function() t[#t + 1] = buffer(1000000) end) end", "=t");
        assert!(matches!(r, Err(Error::Limit(remedy_lua::LimitKind::Memory))), "{r:?}");
        assert!(lua.memory_used() <= 8 << 20);
    }
}

/// API-11: the print sink can be removed; `from_data_with` bounds what it makes.
#[test]
fn api11_clear_print_and_from_data_limits() {
    use std::sync::{Arc, Mutex};
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let seen = Arc::new(Mutex::new(String::new()));
    let s2 = Arc::clone(&seen);
    lua.set_print(move |s| s2.lock().unwrap().push_str(s));
    lua.exec("print('a')", "=t").unwrap();
    lua.clear_print();
    lua.exec("print('b')", "=t").unwrap();
    assert!(seen.lock().unwrap().contains('a') && !seen.lock().unwrap().contains('b'));
    assert!(lua.take_output().contains('b'));
    let big = Data::Table((0..100).map(|i| (Data::Int(i), Data::Str("x".repeat(10)))).collect());
    let lim = |depth, nodes, bytes| DataLimits { depth, nodes, bytes };
    assert!(matches!(lua.from_data_with(&big, lim(32, 50, 1 << 20)), Err(Error::Host(_))));
    assert!(matches!(lua.from_data_with(&big, lim(32, 1000, 500)), Err(Error::Host(_))));
    assert!(matches!(lua.from_data_with(&big, lim(0, 1000, 1 << 20)), Err(Error::Host(_))));
    assert!(lua.from_data_with(&big, lim(1, 1000, 1 << 20)).is_ok());
}

/// API-12: the `os` clocks come from the host when it says so (a browser has none).
#[test]
fn api12_os_clocks_are_the_hosts() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let r = lua.exec("return os.time()", "=t").unwrap();
    assert!(r[0].as_int().unwrap() > 1_600_000_000);
    lua.set_os_clocks(Some(std::sync::Arc::new(|| std::time::Duration::from_secs(86_400))), None);
    let r = lua.exec("return os.time(), os.clock(), os.date('!%Y-%m-%d')", "=t").unwrap();
    assert!(matches!(r[..2], [Value::Int(86_400), Value::Num(0.0)]), "{r:?}");
    assert_eq!(lua.to_string_lossy(r[2]).as_deref(), Some("1970-01-02"));
}
