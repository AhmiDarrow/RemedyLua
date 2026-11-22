//! Sweep 5, area "host API and embedding": the public API attacked as a careless or hostile
//! host would, and by scripts through what a normal host exposes. Each `s5api_<n>` test is
//! finding S5API-n; the `s5api_probe_*` tests are attacks that held, kept as checks. Every
//! program runs with the fast loop on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use remedy_lua::{Data, DataLimits, Error, LimitKind, Libs, Limits, Lua, Value};

fn states_with(limits: Limits) -> impl Iterator<Item = Lua> {
    [true, false].into_iter().map(move |fast| {
        let mut lua = Lua::new(limits, Libs::ALL);
        lua.set_fast_loop(fast);
        lua
    })
}

fn states() -> impl Iterator<Item = Lua> {
    states_with(Limits::default())
}

fn size(lua: &Lua, key: &str) -> usize {
    lua.internal_sizes().into_iter().find(|(k, _)| *k == key).unwrap().1
}

/// Registers `peek()`, which records the largest `scratch` seen in `max`.
fn peek_scratch(lua: &mut Lua) -> Arc<Mutex<usize>> {
    let max = Arc::new(Mutex::new(0usize));
    let m = Arc::clone(&max);
    lua.register("peek", move |lua, _| {
        let n = size(lua, "scratch");
        let mut m = m.lock().unwrap();
        *m = (*m).max(n);
        Ok(vec![])
    })
    .unwrap();
    max
}

/// S5API-1: a library function that calls back many times (`gsub` with a function, `sort`
/// with a comparator or `__lt`, `load` with a reader) kept every result and capture in its
/// scratch, one uncharged slot each: 2 million slots (32 MB) under a 4 MB memory cap from
/// one `gsub`, bounded only by fuel. Repeats are now dropped as it grows.
#[test]
fn s5api_1_library_callbacks_do_not_pile_up_scratch() {
    for mut lua in states_with(Limits::default().memory(4 << 20).fuel(50_000_000)) {
        let max = peek_scratch(&mut lua);
        let r = lua.exec(
            "local n = 0
             local r = string.gsub(string.rep('a', 1000000), 'a', function() n = n + 1 if n % 50000 == 0 then peek() end return 'bb' end)
             return n, #r",
            "=t",
        );
        assert!(matches!(r.as_deref(), Ok([Value::Int(1_000_000), Value::Int(2_000_000)])), "{r:?}");
        let r = lua.exec(
            "local n, x = 0, {}
             local o = setmetatable({}, {__lt = function() n = n + 1 if n % 50000 == 0 then peek() end return x end})
             local t = {} for i = 1, 100000 do t[i] = o end
             pcall(table.sort, t)
             local i = 0
             load(function() i = i + 1 if i % 50000 == 0 then peek() end if i < 500000 then return ' ' end end)
             return n, i",
            "=t",
        );
        assert!(matches!(r.as_deref(), Ok([Value::Int(n), Value::Int(500_000)]) if *n >= 100_000), "{r:?}");
        let most = *max.lock().unwrap();
        assert!(most < 10_000, "scratch grew to {most}");
    }
}

/// S5API-1: compaction never moves `sort`'s root slots or drops what a native still holds:
/// a sort of fresh tables (each kept only by the table and the sort) with collections in the
/// comparator still sorts them all.
#[test]
fn s5api_1_sort_stays_correct_while_scratch_is_compacted() {
    // This checks correctness, not a budget: it used 92% of the default fuel, and sweep 6's
    // fuller charges for strings and marking (S6-RES-3, S6GC-2) took it 9% further.
    for mut lua in states_with(Limits::default().fuel(100_000_000)) {
        let r = lua
            .exec(
                "local t, x, n = {}, {}, 0
                 for i = 1, 20000 do t[i] = {v = (i * 7919) % 20000} end
                 table.sort(t, function(a, b)
                   n = n + 1
                   if n % 4000 == 0 then collectgarbage() end
                   return a.v < b.v and x or false
                 end)
                 for i = 1, 20000 do assert(t[i].v == i - 1, i) end
                 local s = string.gsub(string.rep('ab', 3000), '(a)(b)', function(a, b)
                   n = n + 1 if n % 1000 == 0 then collectgarbage() end return b .. a end)
                 return s == string.rep('ba', 3000)",
                "=t",
            )
            .unwrap();
        assert_eq!(r, [Value::Bool(true)]);
    }
}

/// S5API-1: a host function that reads a script's table in a loop through `get` (whose
/// `__index` returns the same few values) keeps each read until it returns; that list no
/// longer grows past twice the distinct values.
#[test]
fn s5api_1_host_function_reading_in_a_loop() {
    for mut lua in states() {
        lua.register("drain", |lua, a| {
            let mut i = 0;
            loop {
                i += 1;
                let v = lua.get(a[0], Value::Int(i))?;
                if v.is_nil() || i > 200_000 {
                    break;
                }
            }
            Ok(vec![Value::Int(size(lua, "scratch") as i64), Value::Int(i)])
        })
        .unwrap();
        let r = lua.exec("local s = 'same' return drain(setmetatable({}, {__index = function() return s end}))", "=t").unwrap();
        assert!(matches!(r[..], [Value::Int(n), Value::Int(200_001)] if n < 5_000), "{r:?}");
    }
}

/// S5API-2: values the host reads between calls (`get_global`, `create_string` of the same
/// name) were each kept in a list that only a call cleared: a host that polls a state without
/// calling grew it without bound (200,000 entries here, uncharged). Kept values stay valid.
#[test]
fn s5api_2_host_reads_between_calls_do_not_grow_held() {
    for mut lua in states() {
        lua.exec("x = {}", "=t").unwrap();
        let distinct: Vec<Value> = (0..3000).map(|i| lua.create_string(format!("{i:060}")).unwrap()).collect();
        for _ in 0..100_000 {
            lua.get_global("x").unwrap();
            lua.create_string("k").unwrap();
        }
        assert!(size(&lua, "held") < 10_000, "{}", size(&lua, "held"));
        lua.collect_garbage().unwrap();
        // collect_garbage lets them go, as documented; before it they were all kept.
        let mut lua2 = Lua::new(Limits::default(), Libs::ALL);
        let kept: Vec<Value> = (0..3000).map(|i| lua2.create_string(format!("{i:060}")).unwrap()).collect();
        for _ in 0..10_000 {
            lua2.get_global("print").unwrap();
        }
        let t = lua2.create_table().unwrap();
        for (i, v) in kept.iter().enumerate() {
            lua2.table_set(t, Value::Int(i as i64 + 1), *v).unwrap();
        }
        assert_eq!(lua2.table_len(t), 3000);
        assert!(distinct.iter().all(|v| lua.str_bytes(*v).is_none()));
    }
}

/// S5API-3: the results of a `call` made inside a host function are valid until its next
/// call (the guide). They were kept in the natives' return list, which any native reached in
/// between cleared: a `get` whose `__index` called `tostring` let them be collected, and the
/// host got "a value whose object was collected". Also across a nested host function.
#[test]
fn s5api_3_call_results_survive_get_inside_a_host_function() {
    for mut lua in states() {
        lua.exec(
            "function make() return {tag = 'kept'} end
             obj = setmetatable({}, {__index = function(_, k)
               local s = tostring(k) for i = 1, 2000 do local _ = {i} end collectgarbage() return s end})
             nested = setmetatable({}, {__index = function(_, k) return inner(k) end})",
            "=t",
        )
        .unwrap();
        lua.register("inner", |lua, a| {
            let make = lua.get_global("make")?;
            lua.call(make, &[])?;
            lua.exec("for i = 1, 2000 do local _ = {i} end collectgarbage()", "=c")?;
            Ok(vec![a[0]])
        })
        .unwrap();
        lua.register("host", |lua, _| {
            let make = lua.get_global("make")?;
            let r = lua.call(make, &[])?;
            for name in ["obj", "nested"] {
                let o = lua.get_global(name)?;
                let k = lua.create_string("a")?;
                lua.get(o, k)?;
                lua.collect_garbage()?;
            }
            let tag = lua.create_string("tag")?;
            Ok(vec![lua.get(r[0], tag)?])
        })
        .unwrap();
        let r = lua.exec("return host()", "=t").unwrap();
        assert_eq!(lua.to_string_lossy(r[0]).as_deref(), Some("kept"));
        // Still let go at the next call (a callback loop does not pile them up).
        lua.register("loop", |lua, _| {
            let make = lua.get_global("make")?;
            for _ in 0..20_000 {
                lua.call(make, &[])?;
            }
            Ok(vec![])
        })
        .unwrap();
        lua.set_limits(Limits::default().memory(2 << 20));
        lua.exec("loop()", "=t").unwrap();
    }
}

/// S5API-4: the keys of collected tracked userdata were queued uncharged: a script making
/// and dropping them through a host factory queued 3 million keys (24 MB) in one call under
/// a 1 MB cap. Each key is now charged from its userdata's making until `take_collected`.
/// Reports stay exactly once, for unreachable objects only, weak tables included.
#[test]
fn s5api_4_tracked_userdata_queue_is_charged() {
    for mut lua in states() {
        let meta = lua.create_table().unwrap();
        let m = lua.root(Value::Table(meta));
        let next = Mutex::new(0u64);
        lua.register("mk", move |lua, _| {
            let mut n = next.lock().unwrap();
            *n += 1;
            let v = lua.create_userdata_tracked(*n, Some(m.value().as_table().unwrap()), *n)?;
            Ok(vec![v])
        })
        .unwrap();
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        let r = lua.exec(
            "weakk = setmetatable({}, {__mode = 'k'}) weakv = setmetatable({}, {__mode = 'v'})
             eph = setmetatable({}, {__mode = 'k'})
             keep = mk()
             for i = 1, 1000 do local u = mk() weakk[u] = i weakv[i] = u eph[u] = {u} end
             collectgarbage() collectgarbage()
             local n = 0
             for _ in pairs(weakk) do n = n + 1 end for _ in pairs(weakv) do n = n + 1 end
             for _ in pairs(eph) do n = n + 1 end
             return n",
            "=t",
        );
        assert_eq!(r.unwrap(), [Value::Int(0)]);
        let mut c = lua.take_collected();
        c.sort_unstable();
        assert_eq!(c, (2..=1001).collect::<Vec<u64>>());
        lua.exec("keep = nil weakk = nil weakv = nil eph = nil", "=t").unwrap();
        lua.collect_garbage().unwrap();
        assert_eq!(lua.take_collected(), [1]);
        lua.collect_garbage().unwrap();
        assert!(lua.take_collected().is_empty());
        assert!(lua.memory_used() <= base + 2_000, "{} vs {base}", lua.memory_used());
        // Made and dropped in a loop in one call: stopped by the memory limit.
        lua.set_limits(Limits::default().memory(1 << 20).fuel(50_000_000));
        let r = lua.exec("for i = 1, 3000000 do mk() end", "=t");
        assert_eq!(r, Err(Error::Limit(LimitKind::Memory)));
        assert!(size(&lua, "collected") <= (1 << 20) / 8, "{}", size(&lua, "collected"));
        let n = lua.take_collected().len();
        assert!(n > 10_000, "{n}");
        lua.set_limits(Limits::default());
        lua.collect_garbage().unwrap();
        lua.take_collected();
        assert!(lua.memory_used() <= base + 2_000, "{} vs {base}", lua.memory_used());
    }
}

/// S5API-5: with an unlimited memory cap, `set_userdata_size(v, usize::MAX)` was accepted and
/// the collector's `header + size` overflowed (a panic with overflow checks, a wrong release
/// without). Sizes past `isize::MAX` are refused; the largest accepted is given back exactly.
#[test]
fn s5api_5_userdata_size_past_isize_max_is_refused() {
    for mut lua in states_with(Limits::unmetered()) {
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        let v = lua.create_userdata(0u8, None).unwrap();
        assert_eq!(lua.set_userdata_size(v, usize::MAX), Err(Error::Limit(LimitKind::Memory)));
        assert_eq!(lua.set_userdata_size(v, isize::MAX as usize), Err(Error::Limit(LimitKind::Memory)));
        // The largest accepted is `isize::MAX` less the record's own charge (LR-17: what the
        // record takes, no longer a flat 48): one byte more is refused.
        let largest = (48..=512usize)
            .map(|h| isize::MAX as usize - h)
            .find(|&n| lua.set_userdata_size(v, n).is_ok())
            .expect("a size just under isize::MAX less the record is accepted");
        assert_eq!(lua.set_userdata_size(v, largest + 1), Err(Error::Limit(LimitKind::Memory)));
        assert!(lua.memory_used() > isize::MAX as usize);
        lua.set_userdata_size(v, 1 << 40).unwrap();
        lua.exec("return 1", "=t").unwrap();
        lua.collect_garbage().unwrap();
        assert!(lua.memory_used() <= base + 2_000, "{} vs {base}", lua.memory_used());
    }
}

/// S5API-6: `from_data_with` with a depth limit past 64 let 65..limit deep data through its
/// check, then failed half-made with `Error::Runtime`; its check also recursed as deep as the
/// limit asked. The depth is capped at 64 before anything is made.
#[test]
fn s5api_6_from_data_with_depth_is_capped_before_making() {
    for mut lua in states() {
        let mut d = Data::Int(1);
        for _ in 0..100 {
            d = Data::Table(vec![(Data::Int(1), d)]);
        }
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        let r = lua.from_data_with(&d, DataLimits { depth: usize::MAX, nodes: usize::MAX, bytes: usize::MAX });
        assert!(matches!(r, Err(Error::Host(_))), "{r:?}");
        assert_eq!(lua.memory_used(), base);
        let mut d = Data::Int(1);
        for _ in 0..64 {
            d = Data::Table(vec![(Data::Int(1), d)]);
        }
        assert!(lua.from_data_with(&d, DataLimits { depth: 1000, nodes: usize::MAX, bytes: usize::MAX }).is_ok());
        assert!(lua.from_data(&d).is_ok());
    }
}

/// S5API-7: `math.random`'s state was set by hand from the seed, and the generator's second
/// value depended only on constants: `math.random(); math.random()` gave 0.5679936547286644
/// in every state, whatever its seed (random or fixed). It is now seeded as `randomseed`
/// seeds it.
#[test]
fn s5api_7_math_random_depends_on_the_seed() {
    let second = |limits: Limits| {
        let mut lua = Lua::new(limits, Libs::ALL);
        let r = lua.exec("math.random() return math.random(), math.random(1 << 40)", "=t").unwrap();
        format!("{r:?}")
    };
    let fixed: Vec<String> = [0, 1, 2, u64::MAX].into_iter().map(|s| second(Limits::default().hash_seed(s))).collect();
    for (i, a) in fixed.iter().enumerate() {
        for b in &fixed[i + 1..] {
            assert_ne!(a, b);
        }
    }
    assert_ne!(second(Limits::default()), second(Limits::default()));
    // A fixed seed still replays, with the fast loop on or off.
    let run = |fast: bool| {
        let mut lua = Lua::new(Limits::default().hash_seed(7), Libs::ALL);
        lua.set_fast_loop(fast);
        let r = lua.exec("local s = 0 for i = 1, 100 do s = s + math.random(1000) end return s, math.random()", "=t").unwrap();
        (format!("{r:?}"), lua.fuel_left())
    };
    assert_eq!(run(true), run(false));
    assert_eq!(run(true), run(true));
}

/// S5API-8: `root` of a stale handle (or another state's) marked whatever object had since
/// taken its slot, keeping an unrelated object alive as long as the root lived.
#[test]
fn s5api_8_root_of_a_stale_handle_keeps_nothing() {
    for mut lua in states() {
        let stale = lua.create_userdata_tracked(0u8, None, 0).unwrap();
        lua.exec("return 1", "=t").unwrap();
        lua.collect_garbage().unwrap();
        assert_eq!(lua.take_collected(), [0]);
        let t = lua.create_table().unwrap();
        lua.set_global("objs", Value::Table(t)).unwrap();
        for k in 1..=100u64 {
            let u = lua.create_userdata_tracked(k, None, k).unwrap();
            lua.table_set(t, Value::Int(k as i64), u).unwrap();
        }
        let other = Lua::new(Limits::default(), Libs::ALL).create_userdata(1u8, None).unwrap();
        let roots = [lua.root(stale), lua.root(other)];
        assert!(lua.userdata::<u8>(roots[0].value()).is_none());
        lua.exec("objs = nil", "=t").unwrap();
        lua.collect_garbage().unwrap();
        let mut c = lua.take_collected();
        c.sort_unstable();
        assert_eq!(c, (1..=100).collect::<Vec<u64>>());
    }
}

/// S5API-9: after a panic poisoned the state, `set_metatable` still wrote into it and
/// `to_data` still read it as if nothing happened; both are refused like every other entry.
#[test]
fn s5api_9_poisoned_state_refuses_set_metatable_and_to_data() {
    for mut lua in states() {
        lua.register("boom", |_, _| panic!("host bug")).unwrap();
        let t = lua.create_table().unwrap();
        let root = lua.root(Value::Table(t));
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec("boom()", "=t")));
        assert!(r.is_err());
        let t = root.value().as_table().unwrap();
        assert!(matches!(lua.set_metatable(t, None), Err(Error::Host(_))));
        assert!(matches!(lua.to_data(Value::Table(t), DataLimits::default()), Err(Error::Host(_))));
        assert!(matches!(lua.to_data(Value::Int(1), DataLimits::default()), Err(Error::Host(_))));
    }
}

/// Probe: after a panic (in a coroutine, caught by a host function further out), every
/// entry point refuses or reads, and none panics.
#[test]
fn s5api_probe_every_entry_after_poison() {
    for mut lua in states() {
        lua.register("boom", |_, _| panic!("host bug")).unwrap();
        lua.register("guard", |lua, a| {
            let f = a[0];
            let caught = std::panic::catch_unwind(AssertUnwindSafe(|| lua.call(f, &[])));
            // Everything the host function tries now is refused.
            assert!(lua.create_string("x").is_err());
            assert!(lua.collect_garbage().is_err());
            assert!(lua.exec("return 1", "=i").is_err());
            Ok(vec![Value::Bool(caught.is_err())])
        })
        .unwrap();
        let ud = lua.create_userdata(5u32, None).unwrap();
        let keep = lua.root(ud);
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| {
            lua.exec(
                "local r = {pcall(guard, function() local co = coroutine.wrap(function() local y = {} boom() end) co() end)}
                 local t = {} for i = 1, 100 do t[i] = {i} end
                 return table.unpack(r)",
                "=t",
            )
        }));
        assert!(matches!(r, Ok(Err(Error::Host(_))) | Err(_)), "{r:?}");
        let g = lua.globals();
        let _ = lua.table_get(g, Value::Int(1));
        let _ = lua.table_pairs(g);
        let _ = lua.table_len(g);
        let _ = lua.userdata::<u32>(keep.value());
        assert!(lua.create_table().is_err());
        assert!(lua.create_string("x").is_err());
        assert!(lua.create_function("f", |_, _| Ok(vec![])).is_err());
        assert!(lua.create_function_mut("f", |_, _| Ok(vec![])).is_err());
        assert!(lua.create_userdata(1u8, None).is_err());
        assert!(lua.create_userdata_tracked(1u8, None, 1).is_err());
        assert!(lua.set_userdata_size(keep.value(), 100).is_err());
        assert!(lua.collect_garbage().is_err());
        assert!(lua.load("return 1", "=t").is_err());
        assert!(lua.exec("return 1", "=t").is_err());
        assert!(lua.call(keep.value(), &[]).is_err());
        assert!(lua.call_global("print", &[]).is_err());
        assert!(lua.from_data(&Data::Int(1)).is_err());
        assert!(lua.get_global("x").is_err());
        assert!(lua.set_global("a", Value::Int(1)).is_err());
        assert!(lua.get(Value::Table(g), Value::Int(1)).is_err());
        assert!(lua.set(Value::Table(g), Value::Int(1), Value::Int(2)).is_err());
        assert!(lua.table_set(g, Value::Int(1), Value::Int(2)).is_err());
        assert!(lua.register("q", |_, _| Ok(vec![])).is_err());
        assert!(lua.register_mut("q", |_, _| Ok(vec![])).is_err());
        let _ = lua.take_output();
        let _ = lua.take_collected();
        let _ = lua.memory_used();
        let _ = lua.fuel_left();
        lua.set_limits(Limits::default());
        lua.open_libs(Libs::ALL);
        drop(keep);
    }
}

/// Probe: panics in a print sink, a time-limit clock, an `os` clock and a `create_function_mut`
/// function whose nested call panicked: all reach the host or poison, none corrupts.
#[test]
fn s5api_probe_panics_in_host_callbacks() {
    for mut lua in states() {
        lua.set_print(|s| assert!(!s.contains("die"), "sink"));
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec("print('ok') pcall(print, 'die') return 1", "=t")));
        assert!(r.is_err());
        assert!(matches!(lua.exec("return 1", "=t"), Err(Error::Host(_))));
    }
    for mut lua in states() {
        // Read as the call starts, before anything runs: the state is left as it was.
        lua.set_time_limit(Some((Arc::new(|| panic!("clock")), Duration::from_secs(1))));
        assert!(std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec("return 1", "=t"))).is_err());
        lua.set_time_limit(None);
        assert_eq!(lua.exec("return 1", "=t").unwrap(), [Value::Int(1)]);
    }
    for mut lua in states() {
        lua.set_os_clocks(Some(Arc::new(|| panic!("wall"))), None);
        assert!(std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec("return pcall(os.time)", "=t"))).is_err());
        assert!(matches!(lua.exec("return 1", "=t"), Err(Error::Host(_))));
    }
    for mut lua in states() {
        let calls = Mutex::new(0);
        lua.register_mut("m", move |lua, a| {
            *calls.lock().unwrap() += 1;
            let f = a[0];
            let r = std::panic::catch_unwind(AssertUnwindSafe(|| lua.call(f, &[])));
            Ok(vec![Value::Bool(r.is_ok())])
        })
        .unwrap();
        lua.register("boom", |_, _| panic!("boom")).unwrap();
        let r = std::panic::catch_unwind(AssertUnwindSafe(|| lua.exec("local ok, e = pcall(m, boom) return ok, e", "=t")));
        assert!(matches!(r, Ok(Err(Error::Host(_))) | Err(_)), "{r:?}");
        assert!(matches!(lua.exec("return m(print)", "=t"), Err(Error::Host(_))));
    }
    // A userdata's Drop runs in the collector, inside a call: its panic poisons too.
    struct Bomb;
    impl Drop for Bomb {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                panic!("drop");
            }
        }
    }
    for mut lua in states() {
        lua.create_userdata(Bomb, None).unwrap();
        lua.exec("return 1", "=t").unwrap();
        assert!(std::panic::catch_unwind(AssertUnwindSafe(|| lua.collect_garbage())).is_err());
        assert!(matches!(lua.exec("return 1", "=t"), Err(Error::Host(_))));
    }
}

/// Probe: `create_function_mut` reached again through a callback, a coroutine, a metamethod
/// and `pcall` is an ordinary, catchable error, and the function stays usable.
#[test]
fn s5api_probe_function_mut_reentrancy() {
    for mut lua in states() {
        let mut n = 0i64;
        lua.register_mut("m", move |lua, a| {
            n += 1;
            if let Some(f) = a.first().copied().filter(|v| !v.is_nil()) {
                lua.call(f, &[])?;
            }
            Ok(vec![Value::Int(n)])
        })
        .unwrap();
        let r = lua
            .exec(
                "local errs = {}
                 local function try(f) local ok, e = pcall(m, f) errs[#errs + 1] = ok and 'ok' or tostring(e) end
                 try(function() m() end)
                 try(function() coroutine.wrap(function() m() end)() end)
                 try(function() local _ = setmetatable({}, {__index = function() return m() end}).x end)
                 try(function() local ok, e = pcall(m) error(e, 0) end)
                 try(function() table.sort({3, 2, 1}, function(a, b) m() return a < b end) end)
                 for _, e in ipairs(errs) do assert(e:find('called again while it ran'), e) end
                 return m(), #errs",
                "=t",
            )
            .unwrap();
        assert!(matches!(r[..], [Value::Int(_), Value::Int(5)]), "{r:?}");
    }
}

/// Probe: app data taken, replaced and set again by host functions running inside each
/// other.
#[test]
fn s5api_probe_app_data_in_reentrant_host_functions() {
    struct Count(i64);
    for mut lua in states() {
        lua.set_app_data(Count(0));
        lua.register("bump", |lua, a| {
            let taken = lua.take_app_data::<Count>();
            lua.set_app_data(Count(100));
            if let Some(f) = a.first().copied() {
                lua.call(f, &[])?;
            }
            let inner = lua.take_app_data::<Count>().map_or(0, |c| c.0);
            lua.set_app_data(Count(taken.map_or(0, |c| c.0) + inner + 1));
            Ok(vec![Value::Int(lua.app_data::<Count>().map_or(-1, |c| c.0))])
        })
        .unwrap();
        let r = lua.exec("return bump(function() bump(function() bump() end) end)", "=t").unwrap();
        assert_eq!(r, [Value::Int(303)]);
        assert_eq!(lua.take_app_data::<Count>().map(|c| c.0), Some(303));
        assert!(lua.app_data_mut::<Count>().is_none());
    }
}

/// Probe: `set_userdata_size` growing, shrinking below `size_of::<T>()` and growing past the
/// cap; wrong values and stale or foreign handles refused; memory back to its baseline once
/// the userdata is collected.
#[test]
fn s5api_probe_userdata_size_accounting() {
    for mut lua in states_with(Limits::default().memory(16 << 20)) {
        lua.exec("return 1", "=t").unwrap();
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        let v = lua.create_userdata(Vec::<u8>::new(), None).unwrap();
        let r = lua.root(v);
        // The userdata's own record and its root (LR-17: what they take, not a flat 48),
        // with no declared bytes.
        lua.set_userdata_size(v, 0).unwrap();
        let own = lua.memory_used() - base;
        assert!((48..=256).contains(&own), "{own}");
        for n in [0usize, 10, 1 << 20, 3, 8 << 20, 0, 100] {
            lua.set_userdata_size(v, n).unwrap();
            assert_eq!(lua.memory_used() - base, own + n);
        }
        assert_eq!(lua.set_userdata_size(v, 32 << 20), Err(Error::Limit(LimitKind::Memory)));
        assert_eq!(lua.memory_used() - base, own + 100);
        assert!(lua.set_userdata_size(Value::Int(1), 1).is_err());
        let s = lua.create_string("x").unwrap();
        assert!(lua.set_userdata_size(s, 1).is_err());
        let foreign = Lua::new(Limits::default(), Libs::ALL).create_userdata(1u8, None).unwrap();
        assert!(lua.set_userdata_size(foreign, 1 << 20).is_err());
        drop(r);
        lua.exec("return 1", "=t").unwrap();
        lua.collect_garbage().unwrap();
        assert!(lua.set_userdata_size(v, 1).is_err());
        assert!(lua.memory_used() <= base + 200, "{} vs {base}", lua.memory_used());
        // From a host function, past the limit: the call stops, uncatchably.
        lua.register("grow", |lua, a| {
            let v = lua.create_userdata(0u8, None)?;
            lua.set_userdata_size(v, a[0].as_int().unwrap() as usize)?;
            Ok(vec![v])
        })
        .unwrap();
        assert_eq!(lua.exec("pcall(grow, 1 << 40) return 1", "=t"), Err(Error::Limit(LimitKind::Memory)));
        assert_eq!(lua.exec("return pcall(grow, -1)", "=t"), Err(Error::Limit(LimitKind::Memory)));
    }
}

/// Probe: clocks at the extremes (the time limit at `Duration::MAX`, `os` clocks at
/// `Duration::MAX`, UTC offsets at `i64::MIN`/`MAX`): errors or values, never a panic.
#[test]
fn s5api_probe_extreme_clocks() {
    for mut lua in states() {
        lua.set_os_clocks(Some(Arc::new(|| Duration::MAX)), Some(Arc::new(|| Duration::MAX)));
        for src in ["return os.time()", "return os.clock()", "return os.date('%c')", "return os.date('*t').year", "return os.date('!%Y')"] {
            let _ = lua.exec(src, "=t");
        }
        for off in [i64::MAX, i64::MIN, 0] {
            lua.set_utc_offset(off);
            for src in ["return os.date('%c')", "return os.time({year = 2000, month = 1, day = 1})", "return os.date('*t', 1 << 62)"] {
                let _ = lua.exec(src, "=t");
            }
        }
        // The deadline saturates instead of wrapping to the past.
        lua.set_time_limit(Some((Arc::new(|| Duration::from_secs(u64::MAX - 1)), Duration::MAX)));
        assert_eq!(lua.exec("local n = 0 for i = 1, 100000 do n = n + i end return 1", "=t").unwrap(), [Value::Int(1)]);
        // A clock that goes backwards never reaches the deadline; fuel still stops the call.
        let t = Arc::new(Mutex::new(u64::MAX / 2));
        let t2 = Arc::clone(&t);
        lua.set_time_limit(Some((
            Arc::new(move || {
                let mut t = t2.lock().unwrap();
                *t = t.saturating_sub(1_000_000);
                Duration::from_nanos(*t)
            }),
            Duration::from_millis(1),
        )));
        lua.set_limits(Limits::default().fuel(1_000_000));
        assert_eq!(lua.exec("while true do end", "=t"), Err(Error::Limit(LimitKind::Fuel)));
        lua.set_time_limit(Some((Arc::new(|| Duration::ZERO), Duration::ZERO)));
        lua.set_limits(Limits::default());
        assert_eq!(lua.exec("while true do end", "=t"), Err(Error::Limit(LimitKind::Time)));
    }
}

/// Probe: limits changed from inside a host function, mid-call, in every field: the call
/// goes on or stops with an error, and the state is usable afterwards.
#[test]
fn s5api_probe_set_limits_mid_call() {
    for mut lua in states() {
        lua.register("lim", |lua, a| {
            let mut l = lua.limits();
            match a[0].as_int().unwrap() {
                0 => l.output = 0,
                1 => l.memory = 0,
                2 => l.call_depth = 0,
                3 => l.fuel = 0,
                4 => l.native_depth = 0,
                5 => l.string_len = 0,
                6 => l.stack_slots = 0,
                7 => l.error_len = 0,
                8 => l.stack_bytes = 0,
                9 => l = Limits::unmetered().strict_5_4(true).hash_seed(1),
                _ => l = Limits::default().fuel(1).memory(1).call_depth(1).native_depth(0).output(0),
            }
            lua.set_limits(l);
            Ok(vec![])
        })
        .unwrap();
        for i in 0..11 {
            let src = format!(
                "print(('x'):rep(100)) lim({i}) print(('y'):rep(100))
                 local function f(n) if n > 0 then return f(n - 1) end return 0 end f(50)
                 local s = ('z'):rep(10) .. 'q' local t = {{}} for j = 1, 100 do t[j] = j end
                 table.sort(t, function(a, b) return a > b end)
                 return pcall(error, 'msg')"
            );
            let r = lua.exec(&src, "=t");
            if i == 0 || i == 3 || i == 7 || i == 9 {
                assert!(r.is_ok(), "{i}: {r:?}");
            }
            assert!(!lua.limits().strict_5_4);
            lua.set_limits(Limits::default());
            let _ = lua.take_output();
            assert_eq!(lua.exec("return 1", "=t").unwrap(), [Value::Int(1)], "{i}");
        }
    }
}

/// Probe: `raw_equal` is Lua's `rawequal`: exact int/float comparison at 2^53 and 2^63, NaN
/// unequal to itself, long strings by content; stale and foreign handles never read.
#[test]
fn s5api_probe_raw_equal() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let r = lua
        .exec("return 2^53, math.tointeger(2^53), (2^53|0) + 1, 0/0, string.rep('a', 50), string.rep('a', 50)", "=t")
        .unwrap();
    assert!(lua.raw_equal(&r[0], &r[1]));
    assert!(!lua.raw_equal(&r[0], &r[2]));
    assert!(!lua.raw_equal(&r[3], &r[3]));
    assert!(lua.raw_equal(&r[4], &r[5]) && r[4] != r[5]);
    assert!(lua.raw_equal(&Value::Num(-0.0), &Value::Int(0)));
    assert!(!lua.raw_equal(&Value::Int(i64::MAX), &Value::Num(9_223_372_036_854_775_807.0)));
    assert!(lua.raw_equal(&Value::Int(i64::MIN), &Value::Num(-9_223_372_036_854_775_808.0)));
    let mut other = Lua::new(Limits::default(), Libs::ALL);
    let o = other.create_string("a".repeat(50)).unwrap();
    assert!(!lua.raw_equal(&r[4], &o));
    lua.exec("return 1", "=t").unwrap();
    lua.collect_garbage().unwrap();
    assert!(!lua.raw_equal(&r[4], &r[5]));
}

/// Probe: adversarial data both ways: a shared-table DAG stopped by the node budget, zero and
/// maximal limits, NaN and nil keys skipped, float keys normalized.
#[test]
fn s5api_probe_adversarial_data() {
    for mut lua in states() {
        let v = lua.exec("local u = {} for i = 1, 40 do u = {a = u, b = u} end return u, 0/0", "=t").unwrap();
        assert!(matches!(lua.to_data(v[0], DataLimits { depth: 64, nodes: 100_000, bytes: 100 }), Err(Error::Host(_))));
        let zero = DataLimits { depth: 0, nodes: 0, bytes: 0 };
        assert!(lua.to_data(Value::Nil, zero).is_err());
        assert!(lua.from_data_with(&Data::Int(1), zero).is_err());
        let max = DataLimits { depth: usize::MAX, nodes: usize::MAX, bytes: usize::MAX };
        assert!(matches!(lua.to_data(v[1], max), Ok(Data::Num(f)) if f.is_nan()));
        let cyc = lua.exec("local t = {} t.t = t return t", "=t").unwrap()[0];
        assert!(matches!(lua.to_data(cyc, max), Err(Error::Host(_))));
        let d = Data::Table(vec![
            (Data::Num(f64::NAN), Data::Int(1)),
            (Data::Nil, Data::Int(2)),
            (Data::Num(1.0), Data::Int(3)),
            (Data::Num(-0.0), Data::Int(4)),
            (Data::Int(1), Data::Num(f64::NAN)),
        ]);
        let t = lua.from_data(&d).unwrap();
        let out = lua.to_data(t, DataLimits::default()).unwrap();
        let Data::Table(mut pairs) = out else { panic!() };
        pairs.sort_by_key(|(k, _)| format!("{k:?}"));
        assert!(matches!(&pairs[..], [(Data::Int(0), Data::Int(4)), (Data::Int(1), Data::Num(f))] if f.is_nan()), "{pairs:?}");
    }
}

/// Probe: `hash_seed` replays exactly (results, `pairs` order, fuel) with the fast loop on or
/// off, at seeds 0 and `u64::MAX` too; `strict_5_4` stays fixed through `set_limits` and a
/// later `open_libs`.
#[test]
fn s5api_probe_seed_and_strictness_for_life() {
    for seed in [0u64, u64::MAX, 1] {
        let mut outs = Vec::new();
        for fast in [true, false] {
            let mut lua = Lua::new(Limits::default().hash_seed(seed), Libs::ALL);
            lua.set_fast_loop(fast);
            let r = lua
                .exec(
                    "local t = {} for i = 1, 200 do t['k' .. i] = i t[i * 1.5] = i end
                     local s = {} for k in pairs(t) do s[#s + 1] = tostring(k) end
                     return table.concat(s, ','), math.random(1 << 40), math.random()",
                    "=t",
                )
                .unwrap();
            outs.push((lua.to_string_lossy(r[0]).unwrap(), format!("{:?}", &r[1..]), lua.fuel_left()));
        }
        assert_eq!(outs[0], outs[1]);
    }
    let mut lua = Lua::new(Limits::default().strict_5_4(true), Libs::BASE);
    lua.set_limits(Limits::default());
    lua.open_libs(Libs::MATH);
    assert_eq!(lua.exec("return math.pow, math.atan2", "=t").unwrap(), [Value::Nil, Value::Nil]);
    assert!(lua.exec("return setmetatable({}, {__lt = function() return true end}) <= {}", "=t").is_err());
}

/// Probe: a host function that toggles the (hidden) fast-loop switch, re-opens libraries and
/// collects mid-call; the editor entries (`check`, `format`) on very deep sources.
#[test]
fn s5api_probe_switches_mid_call_and_deep_editor_input() {
    for mut lua in states() {
        lua.register("flip", |lua, a| {
            lua.set_fast_loop(a[0].as_int().unwrap_or(0) % 2 == 0);
            if a[0].as_int().unwrap_or(0) % 7 == 0 {
                lua.open_libs(Libs::ALL);
                lua.collect_garbage()?;
            }
            Ok(vec![])
        })
        .unwrap();
        let r = lua
            .exec(
                "local s = 0 for i = 1, 2000 do flip(i) for j = 1, 10 do s = s + j end s = s + #string.rep('a', i % 5) end return s",
                "=t",
            )
            .unwrap();
        assert_eq!(r, [Value::Int(2000 * 55 + 400 * 10)]);
    }
    let deep = format!("return {}1{}", "(".repeat(200_000), ")".repeat(200_000));
    let _ = remedy_lua::check(deep.as_bytes(), "deep.lua");
    let _ = remedy_lua::format(&deep, "deep.lua");
    let deep = format!("return {}1{}", "{".repeat(200_000), "}".repeat(200_000));
    let _ = remedy_lua::check(deep.as_bytes(), "deep.rlua");
    let _ = remedy_lua::format(&deep, "deep.rlua");
}
