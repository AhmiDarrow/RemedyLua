//! Sweep 6: regression tests for what it found. Each test is named
//! `s6_<id>_<what>` after its finding, and runs with the fast loop
//! on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Libs, Limits, Lua};

fn state(limits: Limits, fast: bool) -> Lua {
    let mut lua = Lua::new(limits, Libs::ALL);
    lua.set_fast_loop(fast);
    lua
}

/// S6-RES-3: a short string was charged `40 + len` bytes against about 70 resident (1.3 to
/// 1.5x too little, more for 23 to 40 bytes, which take an allocation of their own). Now
/// each string object is charged what it holds: about 64 bytes, plus 16 for its own
/// allocation past the inline bytes. Checked two ways: the charge per string, and resident
/// memory against the account over two million strings.
#[test]
fn s6_res3_short_strings_are_charged_what_they_hold() {
    for fast in [true, false] {
        for (len, least) in [(7, 64 + 7), (30, 64 + 30 + 16), (60, 64 + 60 + 16)] {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 36), fast);
            lua.exec("collectgarbage('stop') T = {} for i = 1, 100000 do T[i] = false end", "=s").unwrap();
            let before = lua.memory_used();
            let pad = "x".repeat(len - 7);
            lua.exec(&format!("local p = '{pad}' for i = 1, 100000 do T[i] = p .. string.format('%07d', i) end"), "=s").unwrap();
            let per = (lua.memory_used() - before) / 100_000;
            // Each step also leaves the 7-byte `format` result (garbage, not collected).
            let format_part = if len > 7 { 64 + 7 } else { 0 };
            assert!(per >= least + format_part, "fast {fast}: {len}-byte strings charged {per} bytes each, want {}", least + format_part);
        }
    }
}

/// Resident memory (bytes) of this process.
fn rss() -> usize {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("VmRSS"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0)
        * 1024
}

/// The resident side of S6-RES-3, in its own process (other tests share this one's heap).
#[test]
#[ignore = "measures this process's resident memory: run alone"]
fn s6_res3_resident_strings_match_the_account() {
    for len in [7, 23, 40] {
        let mut lua = state(Limits::default().memory(2 << 30).fuel(1 << 40), true);
        lua.exec("collectgarbage('stop') T = {} for i = 1, 2000000 do T[i] = false end", "=s").unwrap();
        let (r0, m0) = (rss(), lua.memory_used());
        let pad = "x".repeat(len - 7);
        lua.exec(&format!("local p = '{pad}' for i = 1, 2000000 do T[i] = p .. string.format('%07d', i) end"), "=s").unwrap();
        let ratio = (rss() - r0) as f64 / (lua.memory_used() - m0) as f64;
        assert!(ratio < 1.25, "{len}-byte strings: resident {ratio:.2}x the account");
    }
}

/// S6-RES-4: probe steps (and error-naming work) are counted per thread, so a state used
/// from another state's host function billed its uncharged steps to the outer script: an
/// inner `table_get`, `table_len`, `table_pairs` or `to_data` on a table of colliding
/// integer keys, or an inner call whose last lookup walked a long chain. Each state now
/// keeps its own: the outer script's fuel is the same whatever the inner state does.
#[test]
fn s6_res4_a_nested_state_does_not_bill_the_outer_one() {
    use remedy_lua::{DataLimits, Value};
    use std::sync::{Arc, Mutex};
    let outer_fuel = |colliding: bool, fast: bool| -> u64 {
        let mut inner = state(Limits::default().fuel(1 << 40), fast);
        // 600 integer keys on one chain of a 1,024-node part (`k mod 1023`), or spread out.
        let step = if colliding { 1023 } else { 1 };
        inner
            .exec(format!("T = {{}} for i = 1, 600 do T[i * {step} + 5000] = i end f = function() return T[300 * {step} + 5000] end"), "=inner")
            .unwrap();
        let t = match inner.get_global("T").unwrap() {
            Value::Table(t) => t,
            v => panic!("{v:?}"),
        };
        let f = inner.get_global("f").unwrap();
        let key = Value::Int(300 * step + 5000);
        let inner = Arc::new(Mutex::new(inner));
        let mut outer = state(Limits::default().fuel(10_000_000), fast);
        outer
            .register("probe", move |_, _| {
                let mut l = inner.lock().unwrap();
                assert_eq!(l.call(f, &[]).unwrap(), [Value::Int(300)]);
                assert_eq!(l.table_len(t), 0);
                assert_eq!(l.table_pairs(t).len(), 600);
                l.to_data(Value::Table(t), DataLimits::default()).unwrap();
                assert_eq!(l.table_get(t, key), Value::Int(300));
                Ok(vec![])
            })
            .unwrap();
        outer.exec("for i = 1, 200 do probe() end", "=outer").unwrap();
        10_000_000 - outer.fuel_left()
    };
    for fast in [true, false] {
        let (plain, colliding) = (outer_fuel(false, fast), outer_fuel(true, fast));
        assert_eq!(colliding, plain, "fast {fast}: the outer script paid for the inner state's probe steps");
    }
}

/// S6T-1 (critical): a constructor's list flush past the array part scanned every node of
/// the hash part for an integer key, free, at each flush of 50 items. `NewTable` presizes
/// at most `u16::MAX` items, so a constructor with `h` named fields and more items paid
/// `h` steps per 50 items: 15x plain code per unit of fuel at h = 60,000, 50x at 200,000
/// (growing with source size), and 23 s where C Lua takes 0.04 s. Now the flush looks up
/// the keys it takes (or scans, charged), and the array part grows geometrically.
#[test]
fn s6t_1_big_constructors_with_named_fields_are_paid() {
    use super::sweep5b_open::{baseline, ns_per_unit};
    for fast in [true, false] {
        let setup = "local H, P = 60000, 100000 local p = {'return {'} \
                     for i = 1, H do p[#p + 1] = 'a' .. i .. '=1,' end \
                     for i = 1, P do p[#p + 1] = '1,' end p[#p + 1] = '}' \
                     F = assert(load(table.concat(p)))";
        let base = baseline(fast);
        let ratio = (0..3).map(|_| ns_per_unit(setup, "F()", 3_000_000, fast) / base).fold(f64::MAX, f64::min);
        assert!(ratio < 5.0, "fast {fast}: a constructor of 60,000 fields and 100,000 items at {ratio:.1}x plain code per unit");
    }
    // The same tables as C Lua.
    let src = "local p = {'return {'} for i = 1, 300 do p[#p + 1] = '[' .. (i + 70000) .. ']=' .. i .. ',' end \
               for i = 1, 70100 do p[#p + 1] = i .. ',' end p[#p + 1] = 'x = 1}' \
               local t = assert(load(table.concat(p)))() local n, s = 0, 0 \
               for k, v in pairs(t) do n = n + 1 s = s + v end return #t, n, s, t[70050], t[70301]";
    let want = super::common::oracle(src, 1 << 32);
    assert!(want.is_ok(), "{want:?}");
    assert_eq!(super::common::remedy(src, 1 << 32), want);
}

/// S6GC-1 (critical): a wrapped coroutine's string error is copied (with the caller's
/// position before it) at each `coroutine.wrap` it crosses, and the copy was free: a 4 MiB
/// message ran at 300x plain code per unit of fuel, 16 MiB at 2,870x, and a chain of 100
/// wraps at about 5,000x. Each copy is paid now.
#[test]
fn s6gc_1_wrap_pays_for_copying_an_error() {
    use super::sweep5b_open::{baseline, ns_per_unit};
    for fast in [true, false] {
        let base = baseline(fast);
        let body = "local w = coroutine.wrap(function() error(BIG, 0) end) pcall(function() w() end)";
        let ratio = (0..3).map(|_| ns_per_unit("BIG = string.rep('x', 1 << 22)", body, 300_000, fast) / base).fold(f64::MAX, f64::min);
        assert!(ratio < 10.0, "fast {fast}: a 4 MiB error through wrap at {ratio:.1}x plain code per unit");
    }
    let src = "local w = coroutine.wrap(function() error(string.rep('x', 300), 0) end) \
               local ok, e = pcall(function() w() end) return ok, #e, e:sub(1, 4)";
    assert_eq!(super::common::remedy(src, 1 << 30), super::common::oracle(src, 1 << 30));
}

/// S6GC-2 (critical): marking charged `1 + slots` per object, a quarter unit of fuel a
/// slot, whatever reaching the object cost: a list of small tables linked in shuffled
/// order (a cache and TLB miss per link) cost 13x plain code per unit of fuel at 2^18
/// tables, 20x at 2^19 and 30x at 2^21. Each object reached is now collector work too.
#[test]
fn s6gc_2_marking_scattered_objects_is_paid() {
    use super::sweep5b_open::{baseline, ns_per_unit};
    let setup = "local n = 1 << 18 local objs = {} for i = 1, n do objs[i] = {false} end \
                 math.randomseed(7) for i = n, 2, -1 do local j = math.random(i) objs[i], objs[j] = objs[j], objs[i] end \
                 for i = 1, n - 1 do objs[i][1] = objs[i + 1] end HEAD = objs[1] objs = nil collectgarbage()";
    for fast in [true, false] {
        let base = baseline(fast);
        let ratio = (0..3).map(|_| ns_per_unit(setup, "collectgarbage()", 2_000_000, fast) / base).fold(f64::MAX, f64::min);
        assert!(ratio < 5.0, "fast {fast}: marking a shuffled list of 2^18 tables at {ratio:.1}x plain code per unit");
    }
}

/// S6-COR-1 (low, fuzz): a library function called as a metamethod was named by its own
/// name in argument errors (`bad argument #2 to 'setmetatable'`) where C Lua names the
/// event (`to 'index'`), and an iterator a library returns (`ipairs`, `gmatch`) by an
/// internal name (`'ipairs_aux'`) where C Lua finds no global name and says `'?'`.
#[test]
fn s6_cor_1_metamethods_and_iterators_are_named_as_c_lua_names_them() {
    for src in [
        "mid = setmetatable return pcall(function() return setmetatable({}, {__index = mid}).a end)",
        "local t = setmetatable({}, {__index = string.rep}) return pcall(function() return t.x end)",
        "local t = setmetatable({}, {__newindex = string.rep}) return pcall(function() t.x = 1 end)",
        "local t = setmetatable({}, {__add = string.rep}) return pcall(function() return t + 1 end)",
        "local t = setmetatable({}, {__lt = string.rep}) return pcall(function() return t < 1 end)",
        "local t = setmetatable({}, {__le = string.rep}) return pcall(function() return t <= 1 end)",
        "local t = setmetatable({}, {__concat = string.rep}) return pcall(function() return t .. 'a' end)",
        "local t = setmetatable({}, {__len = string.rep}) return pcall(function() return #t end)",
        "local t = setmetatable({}, {__unm = string.rep}) return pcall(function() return -t end)",
        "local m = {__eq = string.rep} return pcall(function() return setmetatable({}, m) == setmetatable({}, m) end)",
        "local f = ipairs({}) return pcall(f)",
        "local f = string.gmatch('a', 'a') return pcall(f, 1)",
        "return pcall(function() for _ in ipairs{}'concat' do end end)",
        "return pcall(string.rep)",
    ] {
        assert_eq!(super::common::remedy(src, 1 << 20), super::common::oracle(src, 1 << 20), "{src}");
    }
}

/// LIBS-8 (pre-existing): fuel followed the hash seed in rare
/// shapes with the incremental collector running. A step ended after the object that
/// passed its budget, so the step in which marking ended (and with it where the sweep
/// started relative to allocation) followed the traversal order, which follows the seed:
/// `... for i = 1, 139 do A.B['m' .. i] = ...` used 18,446 units under seeds 1 to 3 and
/// 18,447 under seed 4, and 7 of 500 programs of the family varied. Marking steps now
/// charge exactly their budget and carry the rest, and marking ends only once what is
/// left fits in a step (S6GC-2b): 0 of the family vary.
#[test]
fn s6_libs8_fuel_is_seed_independent_wherever_marking_ends() {
    let mut varied = Vec::new();
    for k in (1..=500).step_by(3) {
        let src = format!(
            "for i = 1, 2000 do _G['g' .. i] = i end A = {{B = {{}}}} \
             for i = 1, {k} do A.B['m' .. i] = {{a = i, b = {{}}}} end \
             local function f() return ('x'):rep(3) end local s = 0 \
             for i = 1, 20 do s = s + #f() + #tostring(i) end return s"
        );
        for fast in [true, false] {
            let fuel: Vec<u64> = [1u64, 2, 3, 4, 0xdead_beef, super::common::TEST_SEED]
                .iter()
                .map(|&seed| {
                    let mut lua = Lua::new(Limits::default().hash_seed(seed), Libs::ALL);
                    lua.set_fast_loop(fast);
                    lua.exec(&src, "=t").unwrap();
                    lua.limits().fuel - lua.fuel_left()
                })
                .collect();
            if fuel.iter().any(|&f| f != fuel[0]) {
                varied.push(format!("k {k} fast {fast}: {fuel:?}"));
            }
        }
    }
    assert!(varied.is_empty(), "fuel followed the seed:\n{}", varied.join("\n"));
}

/// S6GC-2e (raised from open in review): the atomic phase ran in one go whatever fuel the
/// call had left, so a script ran up to one whole atomic phase past its fuel limit: 460 ms
/// over an 800,000-entry ephemeron chain, growing with the memory cap. It now stops at the
/// call's fuel and is suspended; the next call from the host finishes it before any script
/// code runs. Here calls of 200,000 units step the collector over ephemeron chains of
/// 200,000 and 800,000 entries spread over 64 tables: the longest call stays bounded (it
/// was the whole atomic phase, growing with the chain), the chain survives intact, and
/// fuel is the same with the fast loop on and off. (While marking is incremental a single
/// table is still traversed whole: one table's worth per step.)
#[test]
fn s6gc_2e_the_atomic_phase_stops_at_the_fuel_limit() {
    // Timed (the longest call): measured alone, not beside the other heavy tests.
    let _alone = super::common::measuring();
    for n in [200_000, 800_000] {
        let mut runs = Vec::new();
        for fast in [true, false] {
            let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40), fast);
            lua.exec(
                format!(
                    "local mt = {{__mode = 'k'}} TABS = {{}} for j = 1, 64 do TABS[j] = setmetatable({{}}, mt) end \
                     local ks = {{}} for i = 1, {n} + 1 do ks[i] = {{}} end \
                     for i = {n}, 1, -1 do TABS[i % 64 + 1][ks[i]] = ks[i + 1] end ROOT = ks[1] ks = nil"
                ),
                "=setup",
            )
            .unwrap();
            let f = lua.load("while true do collectgarbage('step') end", "=step").unwrap();
            lua.set_limits(lua.limits().fuel(200_000));
            let (mut longest, mut fuel) = (0u64, Vec::new());
            for _ in 0..300 {
                let start = remedy_lua_oracle::thread_cpu();
                let r = lua.call(f.value(), &[]);
                longest = longest.max(remedy_lua_oracle::thread_cpu().saturating_sub(start));
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
                fuel.push(lua.fuel_left());
            }
            lua.set_limits(lua.limits().fuel(1 << 40));
            let left = lua
                .exec("local n, k, i = 0, ROOT, 1 while TABS[i % 64 + 1][k] do k = TABS[i % 64 + 1][k] n = n + 1 i = i + 1 end return n", "=walk")
                .unwrap();
            assert_eq!(left, [remedy_lua::Value::Int(n)], "fast {fast}: the chain");
            // 200,000 units of plain code take a few milliseconds.
            assert!(longest < 60_000_000, "n {n}, fast {fast}: a 200,000-unit call ran {} ms", longest / 1_000_000);
            runs.push(fuel);
        }
        assert_eq!(runs[0], runs[1], "n {n}: fuel differs between the fast loop and the full path");
    }
}

/// Minor page faults this thread has taken (Linux; 0 elsewhere): a call that writes fresh
/// pages pays for each, about 1 us here and 13 us in the gates' VMs.
fn minor_faults() -> u64 {
    std::fs::read_to_string("/proc/thread-self/stat")
        .ok()
        .and_then(|t| t.rsplit(')').next().and_then(|r| r.split_whitespace().nth(7)).and_then(|x| x.parse().ok()))
        .unwrap_or(0)
}

/// S6GC-2e, the incremental half: while marking was incremental a big table was
/// traversed in one step, so a call overran its fuel by one table's traversal (42 to 48 ms
/// here for an 800,000-entry table overwritten and grown between steps). Now it is
/// traversed 4,096 slots a step, with a write barrier that marks what each write into it
/// overwrites and writes (see `Heap::part_step`), and a step does no more work than the
/// call's fuel still pays for. The longest 200,000-unit call stays within a few times the
/// typical one (it was 8.7 times), nothing reachable is freed, and fuel is the same with
/// the fast loop on and off and under every seed. (`W` is made at its full size first: an
/// array part doubling from 2^20 to 2^21 slots is one 20 ms operation of its own, past
/// what any step bounds.)
#[test]
fn s6gc_2e_a_big_table_is_marked_a_chunk_a_step() {
    let mut runs = Vec::new();
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (true, 0xdead_beef)] {
        let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), fast);
        lua.exec("T = {} for i = 1, 800000 do T['k' .. i] = {i} end W = setmetatable({}, {__mode = 'v'}) for i = 1, 1 << 21 do W[i] = false end for i = 1 << 21, 1, -1 do W[i] = nil end N = 800000", "=setup").unwrap();
        // Each call: churn T (overwrite values, add keys so it rehashes as it grows), keep
        // a weak probe of every value written, and step the collector.
        let f = lua
            .load(
                "local j = 0 while true do j = j + 1 \
                 local k = 'k' .. (j % 800000 + 1) local v = {j} T[k] = v W[#W + 1] = v \
                 if j % 7 == 0 then T['k' .. (N + 1)] = {N + 1} N = N + 1 end \
                 if j % 50 == 0 then collectgarbage('step') end end",
                "=churn",
            )
            .unwrap();
        // The control: plain code of the same kind (field reads and writes, a loop) that
        // allocates nothing, in a state of its own (calls into this one would move its
        // collector's steps). Each churn call is timed against the control call just before
        // it, so load from the tests beside this one slows both alike.
        let mut other = state(Limits::default().fuel(200_000), fast);
        other.exec("CK = {} CT = {} for i = 1, 1000 do CK[i] = 'c' .. i CT[CK[i]] = i end", "=setup").unwrap();
        let control = other
            .load("local j, t, n = 0, CT, 0 while true do j = j + 1 local k = CK[j % 1000 + 1] t[k] = j n = n + t[k] end", "=control")
            .unwrap();
        lua.set_limits(lua.limits().fuel(200_000));
        lua.gc_most_slots_at_once();
        let (mut ratios, mut fuel) = (Vec::new(), Vec::new());
        // (Not under a memory-heavy test: its bursts slowed one call and not the control.)
        let quiet = super::common::measuring();
        for _ in 0..200 {
            let start = remedy_lua_oracle::thread_cpu();
            let r = other.call(control.value(), &[]);
            let plain = remedy_lua_oracle::thread_cpu().saturating_sub(start);
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
            let faults = minor_faults();
            let start = remedy_lua_oracle::thread_cpu();
            let r = lua.call(f.value(), &[]);
            let churn = remedy_lua_oracle::thread_cpu().saturating_sub(start);
            let faults = minor_faults().saturating_sub(faults);
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
            ratios.push((churn as f64 / plain.max(1) as f64, ratios.len(), churn / 1000, plain / 1000, faults));
            fuel.push(lua.fuel_left());
        }
        drop(quiet);
        // The regression itself, counted rather than timed: no traversal scans more than
        // one 4,096-slot chunk of a table at once (it was the whole 2^21-node table).
        let most = lua.gc_most_slots_at_once();
        assert!(most <= 4096, "fast {fast}, seed {seed}: {most} slots of a table marked at once");
        fuel.push(most as u64);
        lua.set_limits(lua.limits().fuel(1 << 40));
        // Every key of T still holds its table, each holding a number.
        let ok = lua
            .exec("collectgarbage() local n = 0 for i = 1, N do local v = T['k' .. i] if type(v) == 'table' and type(v[1]) == 'number' then n = n + 1 end end return n == N, n, N", "=check")
            .unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}, seed {seed}: something reachable was freed: {ok:?}");
        ratios.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (typical, (longest, at, us, control, faults)) = (ratios[ratios.len() / 2].0, ratios[ratios.len() - 1]);
        // Each call against the control call before it: the longest at most 4 times the
        // typical such ratio.
        assert!(longest < 4.0 * typical, "fast {fast}, seed {seed}: the longest call {longest:.2} times its control, the typical one {typical:.2} (call {at}: {us} us, its control {control} us, {faults} page faults; the typical call {})", ratios[ratios.len() / 2].4);
        runs.push(fuel);
    }
    assert!(runs.iter().all(|r| *r == runs[0]), "fuel differs between the fast loop, the full path or seeds");
}

/// S6-OUT-1 (from sec/metering): with `Limits::output` lifted and no print sink, the buffer
/// `take_output` drains grew without bound outside the memory limit (a loop of `print`
/// filled it past any cap). It is charged to the memory account until the host takes it:
/// a script printing past the cap stops on the memory limit, and taking the output gives
/// the memory back.
#[test]
fn s6_out_1_the_untaken_output_buffer_is_charged() {
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(16 << 20).output(usize::MAX).fuel(1 << 40), fast);
        let before = lua.memory_used();
        lua.exec("for i = 1, 1000 do print(string.rep('x', 1000)) end", "=small").unwrap();
        assert!(lua.memory_used() >= before + 1_000_000, "fast {fast}: 1 MB of output not charged ({} -> {})", before, lua.memory_used());
        assert_eq!(lua.take_output().len(), 1000 * 1001);
        let r = lua.exec("local s = string.rep('x', 1 << 20) while true do print(s) end", "=flood");
        assert_eq!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Memory)), "fast {fast}");
        let out = lua.take_output();
        assert!(out.len() <= 16 << 20, "fast {fast}: {} bytes of output past a 16 MiB cap", out.len());
        lua.collect_garbage().unwrap();
        assert!(lua.memory_used() < before + (1 << 20), "fast {fast}: memory not given back: {} -> {}", before, lua.memory_used());
    }
}

/// LR-22 (long property test, seeds 30005 and 30013): string-to-number coercion paid a
/// unit per 64 bytes (and nothing up to 64, or 256 for `check_int`'s arguments), while
/// long numerals take the float parser's big-number path: arithmetic on a 300-digit
/// string ran 4 to 5x plain code per unit, up to 9.7x on a loaded machine (bound 10x).
/// Now a unit per 16 bytes from 16 on, through one function for operators and arguments.
/// Fuel for a 300-byte numeral against a 3-byte one, eight operators and four integer
/// arguments a round: at least the per-byte rate, the same on both paths and under every
/// seed.
#[test]
fn lr22_string_coercion_is_charged_by_length() {
    let fuel = |len: usize, fast: bool, seed: u64| {
        let src = format!(
            "local s = '1.' .. string.rep('0', {len} - 2) local n = 0 \
             for i = 1, 100 do \
               local a, b, c, d, e, f, g, h = s + 1, s * '2', -s, s - 1, s / 2, s % 3, s ^ 1, s // 2 \
               n = n + #string.sub('abc', s, s) + string.byte('a', s) + #string.rep('x', s) end \
             return n"
        );
        let mut lua = Lua::new(Limits::default().hash_seed(seed), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(&src, "=t").unwrap();
        lua.limits().fuel - lua.fuel_left()
    };
    let mut all = Vec::new();
    for fast in [true, false] {
        for seed in [1u64, 2, 0xdead_beef, super::common::TEST_SEED] {
            all.push((fuel(3, fast, seed), fuel(300, fast, seed)));
        }
    }
    assert!(all.iter().all(|&f| f == all[0]), "fuel followed the path or the seed: {all:?}");
    let (short, long) = all[0];
    // 100 rounds of 12 coercions of 300 bytes, at a unit per 16 bytes.
    assert!(long - short >= 100 * 12 * (300 / 16), "a 300-byte numeral is undercharged: {short} vs {long}");
}

/// Sweep 6 review (low): `string.pack`, `packsize` and `unpack` copy their format and the
/// copy was outside the memory account. A 4 MiB format (spaces, which pack nothing) with
/// less than 4 MiB of room left must stop on the memory limit; with room, it packs.
#[test]
fn fx2_4_pack_format_copy_is_charged() {
    for (room, fits) in [(3 << 20, false), (6 << 20, true)] {
        for f in ["string.pack(F)", "string.packsize(F)", "string.unpack(F, '')"] {
            let mut lua = Lua::new(Limits::default(), Libs::ALL);
            lua.exec("F = string.rep(' ', 4 << 20) collectgarbage()", "=s").unwrap();
            let used = lua.memory_used();
            lua.set_limits(lua.limits().memory(used + room));
            let r = lua.exec(format!("return {f}"), "=t").map(|_| ());
            match (fits, r) {
                (true, Ok(())) | (false, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Memory))) => {}
                (_, r) => panic!("{f} with {room} bytes of room: {r:?}"),
            }
        }
    }
}

/// FX2-8: gsub sized a replacement before copying it (SEC-M14) and asked for room up to
/// the string limit only. A replacement that ran past the limit then never found its room
/// and spun with no look at fuel or the stop (lr29's gsub ran forever). It must stop on
/// the limit, as the copy did before.
#[test]
fn fx2_8_gsub_past_the_string_limit_stops() {
    let r = super::common::with_deadline(1 << 20, std::time::Duration::from_secs(10), || {
        let mut lua = Lua::new(Limits::default().string_len(4096), Libs::ALL);
        lua.exec("return string.gsub(string.rep('ab', 100), '%w+', string.rep('%0', 64))", "=t").map(|_| ())
    });
    match r {
        Some(Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Memory))) => {}
        r => panic!("gsub past the string limit: {r:?}"),
    }
}

/// S6-STK-1: a level of nested host calls (Lua calling a host function that calls Lua)
/// took 8.7 KB of native stack, 6 KB of it `Lua::run`'s frame: an error result was 48
/// bytes, and each call site's took a stack slot of its own. The default 512 KiB
/// `stack_bytes` held 59 levels, so any growth of the frame stopped the 60 that
/// `sweep6_heap::s6_4` nests ("C stack overflow"). The error is one boxed word now: 5 KB a
/// level, and 90 levels fit (1.5x that), with the same fuel fast and full.
#[test]
fn s6_stk_1_ninety_nested_host_levels_fit_the_default_stack() {
    let mut fuel = Vec::new();
    for fast in [true, false] {
        let mut lua = state(Limits::default(), fast);
        lua.register("h", |lua, args| {
            let (a, b) = (args[0], args[1]);
            drop(lua.call(a, &[])?);
            lua.call(b, &[a, b])
        })
        .unwrap();
        let r = lua
            .exec(
                "local depth = 0 \
                 local function b(a, b2) depth = depth + 1 if depth < 90 then return h(a, b2) end return depth end \
                 return h(function() return 1 end, b)",
                "=t",
            )
            .unwrap();
        assert_eq!(r, vec![remedy_lua::Value::Int(90)], "fast {fast}");
        fuel.push(lua.fuel_left());
    }
    assert_eq!(fuel[0], fuel[1]);
}

/// SEC7-T1 (critical): a memory error part way through a resize lost the table's keys.
/// Past the swap of parts, the keys leaving the array part were held only in a local list
/// and put back one by one; when `nodes` had to grow and the charge failed, the rest of
/// the list was dropped: 524,289 keys became 274,432 to 458,752, `T[1 << 20]` nil. The
/// write still fails at the limit, with every key kept.
#[test]
fn sec7_t1_a_memory_error_mid_resize_keeps_every_key() {
    for fast in [true, false] {
        for room in [1 << 20, 4 << 20, 16 << 20] {
            let mut lua = Lua::new(Limits::default(), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(
                "T = {s = 1} for i = 1, 1 << 20 do T[i] = i end \
                 for i = (1 << 18) + 1, 3 * (1 << 18) do T[i] = nil end collectgarbage() collectgarbage()",
                "=setup",
            )
            .unwrap();
            let used = lua.memory_used();
            let full = lua.limits();
            lua.set_limits(full.memory(used + room));
            let r = lua.exec("pcall(function() for j = 1, 64 do T['n' .. j] = j end end)", "=fill");
            lua.set_limits(full);
            let got = lua
                .exec("local n = 0 for k in pairs(T) do if type(k) == 'number' then n = n + 1 end end return n, T[1 << 20], T[1], T[1 << 18]", "=count")
                .unwrap();
            let shown: Vec<String> = got.iter().map(|v| format!("{v:?}")).collect();
            assert_eq!(
                shown,
                ["Int(524288)", "Int(1048576)", "Int(1)", "Int(262144)"],
                "fast {fast}, room {room}: after {r:?}"
            );
        }
    }
}

/// SEC7-H2 (critical): a host store across a suspended atomic step kept a freed object.
/// `table_get` read a weak table's value the suspended step was about to clear, still
/// white; `table_set` checked it live before `enter` finished the step, which then cleared
/// the entry and swept the object away from under the live table it was stored in. The
/// step is finished before the check now: the value is refused as collected, or (a stop
/// in propagation, where the barrier keeps it) stored whole.
#[test]
fn sec7_h2_a_value_read_across_a_suspended_atomic_step_is_not_freed_where_stored() {
    const SETUP: &str = "collectgarbage('stop') G = {} W = setmetatable({}, {__mode = 'v'}) \
        local S = {} for i = 1, 400000 do local t = {i} W[i] = t S[i] = t end S = nil";
    let big = Limits::default().memory(512 << 20).fuel(1 << 40);
    let mut lua = Lua::new(big, Libs::ALL);
    lua.exec(SETUP, "=setup").unwrap();
    lua.exec("collectgarbage()", "=gc").unwrap();
    let total = big.fuel - lua.fuel_left();
    let (mut stored, mut refused) = (0, 0);
    for k in 1..20u64 {
        let mut lua = Lua::new(big, Libs::ALL);
        lua.exec(SETUP, "=setup").unwrap();
        let global = |lua: &mut Lua, n: &str| match lua.get_global(n).unwrap() {
            remedy_lua::Value::Table(t) => t,
            v => panic!("{v:?}"),
        };
        let (w, g) = (global(&mut lua, "W"), global(&mut lua, "G"));
        lua.set_limits(big.fuel(total * k / 20));
        let _ = lua.exec("collectgarbage()", "=gc");
        let v = lua.table_get(w, remedy_lua::Value::Int(400000));
        lua.set_limits(big);
        if !matches!(v, remedy_lua::Value::Table(_)) {
            continue;
        }
        if lua.table_set(g, remedy_lua::Value::Int(1), v).is_err() {
            refused += 1;
            continue;
        }
        stored += 1;
        let after = lua
            .exec("collectgarbage() collectgarbage() local y = {} for i = 1, 3000 do y[i] = {i} end Y = y return rawget(G[1], 1)", "=after")
            .unwrap();
        assert!(matches!(after.first(), Some(remedy_lua::Value::Int(400000))), "fuel {}/20: G[1] lost what was stored in it: {after:?}", k);
    }
    assert!(stored > 0 && refused > 0, "stored {stored}, refused {refused}: both cases are to be seen");
}

/// SEC7-N4 (medium, a regression from SEC-M14): `%.Ns` checked the length of, made room
/// for and held the whole string, not the N bytes written, so a short result beside long
/// output raised a memory error: `string.format('%s%.1s', a, b)` of two 9 MiB strings is
/// 9437185 bytes in C Lua. The bytes written are what is checked and held.
#[test]
fn sec7_n4_a_precision_counts_the_bytes_it_writes() {
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        lua.set_fast_loop(fast);
        let r = lua
            .exec("local a, b = ('x'):rep(9 << 20), ('y'):rep(9 << 20) return #string.format('%s%.1s', a, b), #string.format(a .. '%.3s', b)", "=t")
            .unwrap();
        assert_eq!(r.iter().map(|v| v.as_int()).collect::<Vec<_>>(), [Some(9437185), Some(9437187)], "fast {fast}");
        // 12 MiB beside, five bytes written: fits in 32 MiB.
        let mut lua = Lua::new(Limits::default().memory(32 << 20), Libs::ALL);
        lua.set_fast_loop(fast);
        let r = lua.exec("local s = ('x'):rep(12 << 20) return string.format('%.5s|%5.2s', s, s)", "=t").unwrap();
        assert_eq!(r.first().and_then(|v| lua.to_string_lossy(*v)).as_deref(), Some("xxxxx|   xx"), "fast {fast}");
    }
}

/// SEC7-H1 (critical): the collector's fuel followed the hash seed. The atomic step
/// traverses again every weak table and thread marking reached, and one marking had not
/// reached (an ephemeron's value whose key came later, which follows the order objects are
/// reached in, so the seed) only once: 400 weak-table values cost 5960 to 6191 by the
/// seed, coroutine values 5264 to 5649. Each now pays both traversals, whichever comes
/// first: the same fuel for every seed, with the fast loop on and off.
#[test]
fn sec7_h1_ephemeron_values_cost_the_same_for_every_seed() {
    let setup = |value: &str, n: usize| {
        format!(
            "R = {{}} local E = setmetatable({{}}, {{__mode = 'k'}}) \
             for i = 1, {n} do local k = {{}} R['w' .. i] = {{k}} E[k] = {value} end R.e = E collectgarbage()"
        )
    };
    let cases = [
        (setup("setmetatable({}, {__mode = 'k'})", 400), "collectgarbage()"),
        (setup("setmetatable({}, {__mode = 'k'})", 400), "for _ = 1, 3 do repeat until collectgarbage('step') end"),
        (setup("coroutine.create(print)", 400), "collectgarbage()"),
        (setup("setmetatable({1, 2, 3}, {__mode = 'v'})", 400), "collectgarbage()"),
        // An ephemeron table traversed in chunks.
        (setup("setmetatable({}, {__mode = 'k'})", 6000), "collectgarbage()"),
    ];
    for (setup, body) in &cases {
        let mut fuel = Vec::new();
        for seed in [1u64, 7, 0xdead_beef, 12345] {
            for fast in [true, false] {
                let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(seed), Libs::ALL);
                lua.set_fast_loop(fast);
                lua.exec(setup, "=setup").unwrap();
                lua.exec(body, "=gc").unwrap();
                fuel.push(lua.limits().fuel - lua.fuel_left());
            }
        }
        assert!(fuel.iter().all(|&f| f == fuel[0]), "{setup} / {body}: fuel by seed (fast, full) {fuel:?}");
    }
}

/// SEC7-H4 (medium): the interner's slots never shrank. After 3M short strings died,
/// `memory_used` came back to its baseline but 4,194,304 slots (32 MB, written so
/// resident) stayed, outside the account, while a script filled the limit beside them.
/// A sweep gives them back when an eighth or fewer were in use since the sweep before, a
/// piece at a time between looks at the stop, with the same fuel for every seed and both
/// loops; strings found again after it are the same strings.
#[test]
fn sec7_h4_the_interner_gives_back_its_slots() {
    let slots = |lua: &Lua| lua.internal_sizes().into_iter().find(|(k, _)| *k == "interner_slots").unwrap().1;
    let mut fuel = Vec::new();
    for seed in [1u64, 7, 0xdead_beef] {
        for fast in [true, false] {
            let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), Libs::ALL);
            lua.set_fast_loop(fast);
            let mut used = 0;
            lua.exec(
                "keep = {} for i = 1, 1000 do keep[i] = 'k' .. i end \
                 local t = {} for i = 1, 3000000 do t[i] = 's' .. i end t = nil collectgarbage() collectgarbage()",
                "=die",
            )
            .unwrap();
            used += lua.limits().fuel - lua.fuel_left();
            // (Given back the sweep after the one the strings died in: the most there were
            // since the last, 3M, kept them that one.)
            assert!(slots(&lua) <= 1 << 13, "seed {seed}, fast {fast}: {} interner slots kept", slots(&lua));
            let r = lua
                .exec(
                    "local u = {} for i = 1, 100000 do u[i] = 'u' .. i end u = nil collectgarbage() \
                     local same = 0 for i = 1, 1000 do if keep[i] == 'k' .. i then same = same + 1 end end return same",
                    "=again",
                )
                .unwrap();
            used += lua.limits().fuel - lua.fuel_left();
            assert_eq!(r.first().and_then(|v| v.as_int()), Some(1000), "seed {seed}, fast {fast}");
            fuel.push(used);
        }
    }
    assert!(fuel.iter().all(|&f| f == fuel[0]), "fuel by seed (fast, full): {fuel:?}");
}

/// SEC7-T4 (high): a refused rehash or key move did uncharged work in every call. The
/// fuel check came after the read-only passes (the census and `resize_scan` for a rehash,
/// the live count for a key move), so a call that could not pay read every part first,
/// and again in every call when the table was written between: at a budget of 20,000 a
/// call, 14 to 19 ms a call over 2^22 keys (33 to 39x plain code per unit), 3 ms for the
/// key move. A call that cannot pay for the passes alone is refused before them; the
/// fuel and results are as before.
#[test]
fn sec7_t4_a_refused_rehash_reads_nothing() {
    for fast in [true, false] {
        for (setup, each) in [
            ("T = {} for i = 1, 1 << 22 do T[i] = i end", "T[1] = T[1]  T[(1 << 22) + 1] = 1"),
            ("T = {} for i = 1, 1 << 21 do T[i * 3] = i end", "T.s = 1"),
        ] {
            let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(setup, "=setup").unwrap();
            let f = lua.load(each, "=each").unwrap();
            lua.set_limits(lua.limits().fuel(20_000));
            let _quiet = super::common::measuring();
            let start = remedy_lua_oracle::thread_cpu();
            for _ in 0..20 {
                let r = lua.call(f.value(), &[]);
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{each}: {r:?}");
            }
            let spent = remedy_lua_oracle::thread_cpu().saturating_sub(start);
            assert!(spent < 20_000_000, "fast {fast}, {each}: 20 refused calls took {} ms", spent / 1_000_000);
        }
    }
}

/// SEC7-T4 under SEC8-F4: SEC7-T4's array shape at a
/// call's fuel below one piece of the census (512 slots at `CENSUS_UNITS` 4: 128 fuel), on
/// both loops and three seeds. Its first 20 calls stay within SEC7-T4's 20 ms. It gets in
/// after a bounded number of calls whenever a call has fuel left after its instructions.
/// The calls together pay the growth's whole cost, and each call's instructions again (a
/// stop takes the call's fuel, SEC8-F4).
#[test]
fn sec7_t4_a_growth_under_a_piece_a_call_is_timed_paid_and_made() {
    const FUEL: u64 = 100;
    let mut seen = None;
    for fast in [true, false] {
        for seed in [1u64, 7, 0xdead_beef] {
            let each = "T[1] = T[1]  T[(1 << 22) + 1] = 1";
            let fresh = || {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec("T = {} for i = 1, 1 << 22 do T[i] = i end", "=setup").unwrap();
                let f = lua.load(each, "=each").unwrap();
                (lua, f)
            };
            let (mut lua, f) = fresh();
            lua.call(f.value(), &[]).unwrap();
            let whole = (1u64 << 40) - lua.fuel_left();
            // (The write's instructions before the growth: the steps a call pays again.)
            let (mut lua, f) = fresh();
            let again = {
                let g = lua.load("T[1] = T[1]  local _ = T", "=again").unwrap();
                lua.call(g.value(), &[]).unwrap();
                (1u64 << 40) - lua.fuel_left()
            };
            lua.set_limits(lua.limits().fuel(FUEL));
            let (mut calls, mut total) = (0u64, 0u64);
            let _quiet = super::common::measuring();
            let start = remedy_lua_oracle::thread_cpu();
            loop {
                calls += 1;
                assert!(calls <= 2 * whole / (FUEL - again) + 20, "fast {fast}, seed {seed}: not in after {calls} calls of {whole}");
                let r = lua.call(f.value(), &[]);
                total += FUEL - lua.fuel_left();
                if calls == 20 {
                    let spent = remedy_lua_oracle::thread_cpu().saturating_sub(start);
                    assert!(spent < 20_000_000, "fast {fast}, seed {seed}: 20 calls of {FUEL} took {} ms", spent / 1_000_000);
                }
                if r.is_ok() {
                    break;
                }
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
            }
            // (One piece of the census: 512 slots at 4 units.)
            let piece = 128;
            assert!(
                (whole..=whole + piece + calls * again).contains(&total),
                "fast {fast}, seed {seed}: {total} in {calls} calls for {whole}, {again} again a call"
            );
            lua.set_limits(lua.limits().fuel(1 << 40));
            let ok = lua.exec("for i = 1, 1 << 22 do if T[i] ~= i then return false end end return T[(1 << 22) + 1] == 1", "=check").unwrap();
            assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}, seed {seed}");
            let got = (whole, calls, total);
            assert_eq!(*seen.get_or_insert(got), got, "fast {fast}, seed {seed}: by loop or seed");
        }
    }
}

/// SEC7-T3 (high): a resize given up between pieces kept its new part beside the old one
/// but counted only the difference: each table so stopped held its old part outside the
/// account until the resize was taken up again (64 MB for 2^21 slots: 257 MB counted, 420
/// MB resident). Kept, the new part is counted whole: the count while it is kept is the
/// count after it finishes plus the old part, about twice the growth for a doubling.
#[test]
fn sec7_t3_a_kept_resize_counts_its_new_part_whole() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let _heavy = super::common::memory_heavy();
    let mut cases = vec![("T = {} for i = 1, 3 << 17 do T[i + 0.5] = i end", "for i = 1, 4 do T[-i - 0.5] = i end")];
    // (The array part's new buffer beside its old one: one buffer only. Segmented, it grows
    // by blocks with nothing beside it: `res_table_a_growth_given_up_goes_on_in_place`
    // checks what is counted there.)
    if cfg!(not(feature = "segmented-arrays")) {
        cases.push(("T = {} for i = 1, 1 << 20 do T[i] = i end", "for i = 1, 4 do T[(1 << 20) + i] = i end"));
    }
    for (setup, write) in cases {
        let state = || {
            let mut lua = Lua::new(Limits::default().memory(1 << 31).fuel(1 << 40).hash_seed(7), Libs::ALL);
            lua.exec(setup, "=setup").unwrap();
            lua.collect_garbage().unwrap();
            lua
        };
        // The clock's reads of the whole write.
        let total = {
            let mut lua = state();
            let f = lua.load(write, "=w").unwrap();
            let n = std::sync::Arc::new(AtomicUsize::new(0));
            let m = std::sync::Arc::clone(&n);
            lua.set_time_limit(Some((std::sync::Arc::new(move || { m.fetch_add(1, Ordering::Relaxed); std::time::Duration::ZERO }), std::time::Duration::from_secs(3600))));
            lua.call(f.value(), &[]).unwrap();
            n.load(Ordering::Relaxed)
        };
        // Stopped at points across the write: some stop keeps the new part beside the old.
        let mut most = (0, 0);
        for k in 1..=20 {
            let mut lua = state();
            let used0 = lua.memory_used();
            let f = lua.load(write, "=w").unwrap();
            let n = std::sync::Arc::new(AtomicUsize::new(0));
            let m = std::sync::Arc::clone(&n);
            let at = (total * k / 21).max(1);
            lua.set_time_limit(Some((
                std::sync::Arc::new(move || {
                    if m.fetch_add(1, Ordering::Relaxed) + 1 >= at { std::time::Duration::from_secs(7200) } else { std::time::Duration::ZERO }
                }),
                std::time::Duration::from_secs(3600),
            )));
            if lua.call(f.value(), &[]).is_ok() {
                continue;
            }
            lua.set_time_limit(None);
            lua.collect_garbage().unwrap();
            let kept = lua.memory_used().saturating_sub(used0);
            lua.call(f.value(), &[]).unwrap();
            lua.collect_garbage().unwrap();
            let grown = lua.memory_used().saturating_sub(used0);
            assert!(grown > 1 << 20, "{setup}: grew {grown}");
            most = most.max((kept, grown));
        }
        let (kept, grown) = most;
        assert!(kept * 8 >= grown * 15, "{setup}: at most {} MB counted kept, {} MB once finished", kept >> 20, grown >> 20);
    }
}

/// SEC7-G1 (high; cause COV-5): marking is charged by a table's size when
/// the cycle started, and its growth was to be paid by the resizes, but a resize pays a
/// sixteenth of a unit a slot copied where marking a slot costs `1 + obj_work / 4`: a table
/// grown by a million slots mid-cycle was marked for nothing, in one step (10 to 18 ms of a
/// 200,000-unit call, `s6gc_2e`'s flake). A table grown past that size while marking is
/// under way now pays for its growth's marking when it grows: the same growth costs more
/// fuel mid-cycle than with no cycle under way, by at least a quarter unit a slot, and the
/// same fuel for every seed and both loops.
#[test]
fn sec7_g1_growth_during_marking_is_paid_for() {
    let grow = "for i = 1, 1 << 20 do T[i] = i end";
    let mut by = Vec::new();
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (false, 0xdead_beef)] {
        let fuel = |marking: bool| {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
            lua.exec("collectgarbage('stop') T = {} collectgarbage()", "=setup").unwrap();
            if marking {
                // A cycle started: marking under way, and no step after it (stopped).
                lua.exec("collectgarbage('step')", "=step").unwrap();
            }
            lua.exec(grow, "=grow").unwrap();
            lua.limits().fuel - lua.fuel_left()
        };
        let (idle, mid) = (fuel(false), fuel(true));
        // (Less what the resizes' sixteenths round away, one a resize.)
        assert!(mid + 32 >= idle + (1 << 20) / 4, "fast {fast}, seed {seed}: grown mid-cycle {mid}, with no cycle {idle}");
        by.push((idle, mid));
    }
    assert!(by.iter().all(|f| *f == by[0]), "fuel differs by seed or loop: {by:?}");
}

/// SEC7-G1, the looks: the atomic phase looked at the interrupt and the deadline every
/// 4,096 units of its work, and a table's slots past its size when the cycle started add
/// none (their marking was paid as it grew), so it scanned all of them between two looks:
/// 3.6 ms between two reads of the deadline, and a call of 18 ms where its neighbours took
/// 2 (call 162 of `s6gc_2e_a_big_table_is_marked_a_chunk_a_step`). It looks by the slots
/// it scans as well now. The shape of that call: a table doubled after the cycle started,
/// written again after it was marked, so traversed again in the atomic phase, whole, in
/// pieces; fuel unchanged.
#[test]
fn sec7_g1_the_atomic_phase_looks_by_the_slots_it_scans() {
    let mut fuel = Vec::new();
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (false, 7), (true, 0xdead_beef), (false, 0xdead_beef)] {
        let mut lua = state(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(seed), fast);
        // T at 2^17 nodes when a cycle starts, then 2^18: its growth is past the snapshot.
        lua.exec("T = {} for i = 1, 100000 do T['k' .. i] = {i} end E = setmetatable({}, {__mode = 'k'}) collectgarbage() collectgarbage('step')", "=setup").unwrap();
        lua.exec("for i = 100001, 140000 do T['k' .. i] = {i} end", "=grow").unwrap();
        let f = lua
            .load("local j = 0 while true do j = j + 1 T['k' .. (j % 1000 + 1)] = {j} E[{}] = j if j % 20 == 0 then collectgarbage('step') end end", "=churn")
            .unwrap();
        lua.set_limits(lua.limits().fuel(100_000));
        lua.gc_most_slots_between_looks();
        let mut fl = Vec::new();
        for _ in 0..80 {
            let r = lua.call(f.value(), &[]);
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
            fl.push(lua.fuel_left());
        }
        let most = lua.gc_most_slots_between_looks();
        // A look every 4,096 units of work and slots scanned, after a piece of at most
        // 4,096 slots: at most 8,192 slots between two (it was 2^17, the growth).
        assert!(most <= 8192, "fast {fast}, seed {seed}: {most} slots scanned between two looks");
        lua.set_limits(lua.limits().fuel(1 << 40));
        let ok = lua.exec("collectgarbage() for i = 1, 140000 do if type(T['k' .. i]) ~= 'table' then return false end end return true", "=check").unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}, seed {seed}");
        fuel.push(fl);
    }
    assert!(fuel.iter().all(|f| *f == fuel[0]), "fuel differs between seeds or loops");
}

/// SEC7-G1's reproducer, `s6gc_2e_a_big_table_is_marked_a_chunk_a_step`'s shape at
/// `n` keys: a string-keyed table and a weak array grown in the cycle the calls run in,
/// then 200,000-unit calls that overwrite and grow the first and step the collector. The
/// fuel left after each call and how far it got (the weak array's border), and the trace
/// (`Lua::gc_trace`, on `clock` when given).
fn g1_growth(n: usize, seed: u64, fast: bool, calls: usize, clock: Option<remedy_lua::Clock>) -> (Vec<u64>, Vec<remedy_lua::TraceRecord>, u64) {
    let (fuel, recs, dropped, _) = g1_growth_sliced(n, seed, fast, calls, clock, None);
    (fuel, recs, dropped)
}

/// `g1_growth` with each call's collector polls bounded to `slots` (`Lua::gc_sliced_call`):
/// also the polls each call took, and what the program left (N, #W, a checksum of T).
fn g1_growth_sliced(
    n: usize,
    seed: u64,
    fast: bool,
    calls: usize,
    clock: Option<remedy_lua::Clock>,
    slots: Option<usize>,
) -> (Vec<u64>, Vec<remedy_lua::TraceRecord>, u64, (Vec<u32>, Vec<remedy_lua::Value>)) {
    g1_growth_polled(n, seed, fast, calls, clock, slots, 200_000)
}

/// `g1_growth_sliced` with `fuel` units a call.
fn g1_growth_polled(
    n: usize,
    seed: u64,
    fast: bool,
    calls: usize,
    clock: Option<remedy_lua::Clock>,
    slots: Option<usize>,
    call_fuel: u64,
) -> (Vec<u64>, Vec<remedy_lua::TraceRecord>, u64, (Vec<u32>, Vec<remedy_lua::Value>)) {
    let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), fast);
    let w = (2 * n).next_power_of_two();
    lua.exec(
        &format!("T = {{}} for i = 1, {n} do T['k' .. i] = {{i}} end W = setmetatable({{}}, {{__mode = 'v'}}) for i = 1, {w} do W[i] = false end for i = {w}, 1, -1 do W[i] = nil end N = {n}"),
        "=setup",
    )
    .unwrap();
    let f = lua
        .load(
            format!(
                "local j = 0 while true do j = j + 1 \
                 local k = 'k' .. (j % {n} + 1) local v = {{j}} T[k] = v W[#W + 1] = v \
                 if j % 7 == 0 then T['k' .. (N + 1)] = {{N + 1}} N = N + 1 end \
                 if j % 50 == 0 then collectgarbage('step') end end"
            ),
            "=churn",
        )
        .unwrap();
    lua.set_limits(lua.limits().fuel(call_fuel));
    lua.gc_trace(clock, 64 << 10);
    // (How far each call got, read without running anything: the weak array's border.)
    let Ok(remedy_lua::Value::Table(wt)) = lua.get_global("W") else { panic!("W") };
    let mut fuel = Vec::new();
    let mut polls = Vec::new();
    for _ in 0..calls {
        let r = match slots {
            None => lua.call(f.value(), &[]),
            Some(k) => {
                let (r, p) = lua.gc_sliced_call(f.value(), &[], k);
                polls.push(p);
                r
            }
        };
        assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
        fuel.push(lua.fuel_left());
        fuel.push(u64::try_from(lua.table_len(wt)).unwrap_or(u64::MAX));
    }
    let (recs, dropped) = lua.gc_trace_take();
    lua.set_limits(lua.limits().fuel(1 << 40));
    let left = lua.exec("local s = 0 for i = 1, N do local v = T['k' .. i] s = s + (v and v[1] or -1) end collectgarbage() return N, #W, s", "=left").unwrap();
    (fuel, recs, dropped, (polls, left))
}

/// SEC7-G1 step 3: a marking step paused at the end of a host poll (after a chunk) and
/// resumed inside its loop is invisible: for each seed, both loops, and polls of one
/// chunk, four and sixty-four, each 200,000-unit call stops at its fuel limit (a tight
/// limit, every call) with the same fuel left as with no polls, and the program leaves the
/// same state (how far it got in its fuel, its tables). With polls of one chunk the calls
/// do pause (more than one poll each). SEC7-G2: the weak array and the per-call
/// progress also agree across seeds, even as the hash table grows during marking.
#[test]
fn sec7_g1_a_paused_step_is_invisible_to_the_fuel() {
    let mut reference = None;
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (false, 7), (true, 0xdead_beef), (false, 0xdead_beef)] {
        let (fuel, _, _, (_, left)) = g1_growth_sliced(60_000, seed, fast, 60, None, None);
        let baseline = reference.get_or_insert_with(|| (fuel.clone(), left.clone()));
        assert_eq!((&fuel, &left), (&baseline.0, &baseline.1), "fast {fast}, seed {seed}");
        for slots in [4096usize, 4 * 4096, 64 * 4096] {
            let (sf, _, _, (polls, sleft)) = g1_growth_sliced(60_000, seed, fast, 60, None, Some(slots));
            assert_eq!(sf, fuel, "fast {fast}, seed {seed}, polls of {slots} slots: fuel");
            assert_eq!(sleft, left, "fast {fast}, seed {seed}, polls of {slots} slots: the program's state");
            if slots == 4096 {
                assert!(polls.iter().sum::<u32>() > polls.len() as u32, "fast {fast}, seed {seed}: no call paused ({polls:?})");
            }
        }
    }
}


/// SEC7-G1, the trace itself: it changes no fuel (on and off, every seed, both loops),
/// keeps one record a call and one a step, and stops at its buffer's end, counting the
/// rest.
#[test]
fn sec7_g1_the_collectors_trace_changes_no_fuel() {
    let clock: remedy_lua::Clock = {
        let start = std::time::Instant::now();
        std::sync::Arc::new(move || start.elapsed())
    };
    let mut seen = Vec::new();
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (false, 0xdead_beef)] {
        let (off, none, _) = g1_growth(20_000, seed, fast, 40, None);
        assert!(none.is_empty());
        let (on, recs, dropped) = g1_growth(20_000, seed, fast, 40, Some(clock.clone()));
        assert_eq!(off, on, "fast {fast}, seed {seed}: the trace changed the fuel");
        assert_eq!(dropped, 0);
        let polls: Vec<_> = recs.iter().filter(|r| r.op == remedy_lua::TraceOp::Poll).collect();
        assert_eq!(polls.len(), 40, "one record a call");
        assert!(polls.iter().all(|p| p.fuel >= 200_000 && p.end >= p.start));
        let steps = recs.iter().filter(|r| r.op != remedy_lua::TraceOp::Poll).count();
        assert!(steps > 40, "fast {fast}, seed {seed}: {steps} steps traced");
        // The slots of each call's steps add up to its poll's.
        for p in &polls {
            let s: u64 = recs.iter().filter(|r| r.op != remedy_lua::TraceOp::Poll && r.call == p.call).map(|r| r.slots).sum();
            assert_eq!(s, p.slots, "call {}", p.call);
        }
        seen.push(on);
    }
    // A small buffer: full, the rest counted.
    let mut lua = state(Limits::default().fuel(1 << 40), true);
    lua.gc_trace(Some(clock), 8);
    for _ in 0..20 {
        lua.exec("local t = {} for i = 1, 20000 do t[i] = {} end collectgarbage()", "=c").unwrap();
    }
    let (recs, dropped) = lua.gc_trace_take();
    assert_eq!(recs.len(), 8);
    assert!(dropped > 0);
}

/// SEC7-G1, the plain call's pause: slots a chunked traversal scans past the table's size
/// when the cycle started (a hash part grown in the cycle, paid as it grew) spent no step's
/// budget, so a step ended at its slots alone (`STEP_SLOTS`, 64 Ki) and a call took such
/// steps one after another: on `s6gc_2e_a_big_table_is_marked_a_chunk_a_step`'s workload
/// call 40 (from 0) took 20 to 24 ms against 5 to 6 ms typical (3.7 to 4.2 times), 16 steps
/// of 64 Ki slots over the 2^20 nodes `T` grew by (2^20 to 2^21) in the setup's last rehash.
/// They spend it now, uncharged, at what scanning one costs against a unit of fuel,
/// measured (`heap::GROWN_SLOT_WORK`). Two asserts, as sweep 8 asks:
/// - the work bound, counted: no call (the setup's included) scans more grown slots, at
///   that rate, than its marking steps' budgets cover, and a chunk's (`Lua::gc_pace_probe`;
///   call 40 scanned 1,048,576 against budgets of 47,880 units on 5dc4ba3);
/// - the latency, timed on the thread's CPU: call 40 within 2 times the typical call.
/// Where steps end follows the program alone: the fuel and those counts per call are the
/// same under every seed, in both loops, with plain calls and with polls of one chunk (the
/// G1 reproducer). (The setup's budgets are left out of that: what its steps were given
/// follows the seed by a few dozen slots in 2 million, on 5dc4ba3 as here.)
#[test]
fn sec7_g1_a_call_scans_no_more_prepaid_slots_than_its_steps_pay_for() {
    let run = |seed: u64, fast: bool, calls: usize, call_fuel: u64, poll: Option<usize>, timed: bool| {
        let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), fast);
        let p0 = lua.gc_pace_probe();
        lua.exec("T = {} for i = 1, 800000 do T['k' .. i] = {i} end W = setmetatable({}, {__mode = 'v'}) for i = 1, 1 << 21 do W[i] = false end for i = 1 << 21, 1, -1 do W[i] = nil end N = 800000", "=setup").unwrap();
        let p1 = lua.gc_pace_probe();
        let f = lua
            .load(
                "local j = 0 while true do j = j + 1 \
                 local k = 'k' .. (j % 800000 + 1) local v = {j} T[k] = v W[#W + 1] = v \
                 if j % 7 == 0 then T['k' .. (N + 1)] = {N + 1} N = N + 1 end \
                 if j % 50 == 0 then collectgarbage('step') end end",
                "=churn",
            )
            .unwrap();
        lua.set_limits(lua.limits().fuel(call_fuel));
        // (Per call: the grown slots scanned and the budgets in slots; the setup's first.)
        let setup = (p1.0 - p0.0, p1.1 - p0.1, p1.2);
        let (mut per_call, mut fuel, mut cpu) = (Vec::new(), Vec::new(), Vec::new());
        let quiet = timed.then(super::common::measuring);
        for _ in 0..calls {
            let before = lua.gc_pace_probe();
            let start = remedy_lua_oracle::thread_cpu();
            let r = match poll {
                None => lua.call(f.value(), &[]),
                Some(k) => lua.gc_sliced_call(f.value(), &[], k).0,
            };
            cpu.push(remedy_lua_oracle::thread_cpu().saturating_sub(start));
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
            let after = lua.gc_pace_probe();
            per_call.push((after.0 - before.0, after.1 - before.1, after.2));
            fuel.push(lua.fuel_left());
        }
        drop(quiet);
        (setup, per_call, fuel, cpu)
    };
    let mut seen = Vec::new();
    for (seed, fast) in [(1u64, true), (1, false), (7, true), (0xdead_beef, false)] {
        let (setup, per_call, fuel, cpu) = run(seed, fast, 200, 200_000, None, true);
        for (call, &(grown, budget, rate)) in std::iter::once(&setup).chain(&per_call).enumerate() {
            assert!(grown * rate <= budget + 4096 * rate, "seed {seed} fast {fast}: call {call} (0: the setup) scanned {grown} grown slots at {rate} units, its steps' budgets {budget} units");
        }
        assert!(setup.0 + per_call.iter().map(|p| p.0).sum::<u64>() > 500_000, "seed {seed} fast {fast}: the grown slots were scanned");
        let mut sorted = cpu.clone();
        sorted.sort_unstable();
        let typical = sorted[sorted.len() / 2];
        println!("seed {seed} fast {fast}: call 40 {} us, typical {} us ({:.2} times), longest {} us", cpu[40] / 1000, typical / 1000, cpu[40] as f64 / typical.max(1) as f64, sorted[sorted.len() - 1] / 1000);
        assert!(cpu[40] < 2 * typical, "seed {seed} fast {fast}: call 40 took {} us of CPU, the typical call {} us", cpu[40] / 1000, typical / 1000);
        seen.push(((seed, fast), setup.0, per_call, fuel));
    }
    for s in &seen {
        assert_eq!((s.1, &s.2, &s.3), (seen[0].1, &seen[0].2, &seen[0].3), "{:?} against {:?}: the setup's grown slots, each call's grown slots and budgets, then its fuel", s.0, seen[0].0);
    }
    // The G1 reproducer's frame-sized calls, polled a chunk at a time: the same under every
    // seed and loop.
    let mut polled = Vec::new();
    for (seed, fast) in [(1u64, true), (7, false), (0xdead_beef, true)] {
        let (setup, per_call, fuel, _) = run(seed, fast, 300, 20_000, Some(4096), false);
        polled.push(((seed, fast), setup.0, per_call, fuel));
    }
    for s in &polled {
        assert_eq!((s.1, &s.2, &s.3), (polled[0].1, &polled[0].2, &polled[0].3), "polled, {:?} against {:?}", s.0, polled[0].0);
    }
}

/// SEC8-G3: a full collection's pass is traced (`TraceOp::Full`), and changes no fuel. At
/// the memory limit an allocation's emergency collection ran untraced, so a per-cycle
/// trace read the calls it took (each spending its fuel in it) as an atomic phase of
/// hundreds of calls. Here 60,000 live tables under a 16 MiB limit and a churn of garbage
/// in calls of 20,000 units: calls run full collections (each a record with its slots and
/// fuel), the slots of each call's records still add up to its poll's, and the fuel of
/// every call and the progress are the same with the trace on and off, in both loops. A
/// host's `collectgarbage()` is a record too.
#[test]
fn sec8_g3_full_collections_are_traced() {
    use remedy_lua::TraceOp;
    let run = |fast: bool, traced: bool| {
        let mut lua = state(Limits::default().memory(16 << 20).fuel(1 << 40).hash_seed(1), fast);
        let f = lua.load("local j = 0 while true do j = j + 1 local t = {j, j} P = (P or 0) + 1 end", "=churn").unwrap();
        lua.exec("K = {} for i = 1, 60000 do K[i] = {i} end collectgarbage('stop')", "=setup").unwrap();
        lua.set_limits(lua.limits().fuel(20_000));
        if traced {
            lua.gc_trace(Some(std::sync::Arc::new(|| std::time::Duration::ZERO)), 1 << 16);
        }
        let mut seen = Vec::new();
        for _ in 0..1200 {
            let r = lua.call(f.value(), &[]);
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
            seen.push(lua.fuel_left());
        }
        // (Read after the calls: a host read finishes a suspended atomic phase first, an
        // entry of its own, SEC7-H2, which would fall between two calls' records.)
        let recs = lua.gc_trace_take().0;
        lua.set_limits(lua.limits().fuel(1 << 40));
        let progress = lua.get_global("P").ok().and_then(|v| v.as_int()).unwrap_or(0);
        ((seen, progress), recs)
    };
    for fast in [true, false] {
        let (off, none) = run(fast, false);
        assert!(none.is_empty());
        let (on, recs) = run(fast, true);
        assert_eq!(off, on, "fast {fast}: the trace changed the fuel or the progress");
        let full: Vec<_> = recs.iter().filter(|r| r.op == TraceOp::Full).collect();
        let calls: std::collections::BTreeSet<u32> = full.iter().map(|r| r.call).collect();
        assert!(calls.len() > 50, "fast {fast}: {} calls ran a full collection", calls.len());
        assert!(full.iter().all(|r| r.fuel > 0 && r.end >= r.start), "fast {fast}: {full:?}");
        // A pass that began while marking scans table slots; one that began in the sweep
        // may only sweep, which scans none (SEC8-F7: charged to the end of the call's fuel
        // and no further, a pass can be all sweep).
        let marking: Vec<_> = full.iter().filter(|r| r.bytes == remedy_lua::FULL_BEGAN_MARK).collect();
        assert!(!marking.is_empty(), "fast {fast}: no pass began while marking");
        assert!(marking.iter().all(|r| r.slots > 0), "fast {fast}: {marking:?}");
        for p in recs.iter().filter(|r| r.op == TraceOp::Poll) {
            let s: u64 = recs.iter().filter(|r| r.op != TraceOp::Poll && r.call == p.call).map(|r| r.slots).sum();
            assert_eq!(s, p.slots, "fast {fast}: call {}", p.call);
        }
    }
    let mut lua = state(Limits::default().fuel(1 << 40), true);
    lua.exec("T = {} for i = 1, 20000 do T[i] = {} end T = nil", "=garbage").unwrap();
    lua.gc_trace(Some(std::sync::Arc::new(|| std::time::Duration::ZERO)), 64);
    lua.exec("collectgarbage()", "=full").unwrap();
    let (recs, _) = lua.gc_trace_take();
    assert!(recs.iter().any(|r| r.op == TraceOp::Full && r.slots > 0 && r.fuel > 0), "{recs:?}");
}

/// SEC7-G1's report (`--nocapture` for it): the growth reproducer at full size, its work
/// given as 2,000 calls of 20,000 units (a frame-sensitive host's slice: the script's own
/// run in one is about half a millisecond), run with plain calls (each poll a whole call)
/// and with polls of one 4,096-slot chunk too (`Lua::gc_sliced_call`), side by side: the
/// polls, their total time, the typical and the worst poll, the most collector time in
/// one poll and the slots of a poll, with the slowest poll and its steps around it; then
/// the same with 200 calls of 200,000 units, for comparison. Asserted on the polls of one
/// chunk at 20,000 units a call: none scans more than 16 chunks (the work bound), none
/// takes over 2 ms of the thread's CPU time (the latency target), and the collector's
/// share of none is over 2 ms of it. Wall time is printed beside it, not asserted.
#[test]
fn sec7_g1_growth_report() {
    use super::common::slowest_around;
    let start = std::time::Instant::now();
    let clock: remedy_lua::Clock = std::sync::Arc::new(move || start.elapsed());
    let summary = |name: &str, recs: &[remedy_lua::TraceRecord], dropped: u64| {
        let polls: Vec<_> = recs.iter().filter(|r| r.op == remedy_lua::TraceOp::Poll).collect();
        let mut walls: Vec<_> = polls.iter().map(|p| p.wall()).collect();
        walls.sort_unstable();
        let total: std::time::Duration = walls.iter().sum();
        let gc = polls.iter().map(|p| recs.iter().filter(|r| r.op != remedy_lua::TraceOp::Poll && r.poll == p.poll).map(|r| r.wall()).sum::<std::time::Duration>()).max().unwrap_or_default();
        let mut slots: Vec<_> = polls.iter().map(|p| p.slots).collect();
        slots.sort_unstable();
        println!(
            "{name}: {} polls, total {} ms, typical {} us, p99 {} us, worst {} us; most collector time in a poll {} us; slots a poll typical {}, most {}; {dropped} records dropped",
            polls.len(),
            total.as_millis(),
            walls[walls.len() / 2].as_micros(),
            walls[walls.len() * 99 / 100].as_micros(),
            walls[walls.len() - 1].as_micros(),
            gc.as_micros(),
            slots[slots.len() / 2],
            slots[slots.len() - 1]
        );
        (walls[walls.len() - 1], gc, slots[slots.len() - 1])
    };
    let (_, before, d0, _) = g1_growth_polled(800_000, 1, true, 2000, Some(clock.clone()), None, 20_000);
    summary("20,000 units a call, plain calls", &before, d0);
    let (_, recs, dropped, _) = g1_growth_polled(800_000, 1, true, 2000, Some(clock.clone()), Some(4096), 20_000);
    let (worst_wall, _, _) = summary("20,000 units a call, polls of one chunk, wall", &recs, dropped);
    // On the thread's CPU time (faults included): a hypervisor can stop the vCPU in a poll with no
    // switch the guest sees, and a pure arithmetic loop is stopped as often (SEC7-G1's probe).
    let cpu: remedy_lua::Clock = std::sync::Arc::new(|| std::time::Duration::from_nanos(remedy_lua_oracle::thread_cpu()));
    let (_, cpus, dropped_cpu, _) = g1_growth_polled(800_000, 1, true, 2000, Some(cpu), Some(4096), 20_000);
    let (worst, gc, most) = summary("20,000 units a call, polls of one chunk, cpu", &cpus, dropped_cpu);
    // (The same polls in both runs: the fuel sets them.)
    let poll_of = |rs: &[remedy_lua::TraceRecord]| -> std::collections::HashMap<_, std::time::Duration> {
        rs.iter().filter(|r| r.op == remedy_lua::TraceOp::Poll).map(|r| (r.poll, r.wall())).collect()
    };
    let (walls, cpu_polls) = (poll_of(&recs), poll_of(&cpus));
    let off = walls.iter().map(|(p, w)| w.saturating_sub(cpu_polls.get(p).copied().unwrap_or_default())).max().unwrap_or_default();
    let total: std::time::Duration = walls.values().sum();
    println!("worst cpu {} us, worst wall {} us, worst wall - cpu {} us, {} polls, total {} ms", worst.as_micros(), worst_wall.as_micros(), off.as_micros(), walls.len(), total.as_millis());
    let (_, big, d1, _) = g1_growth_polled(800_000, 1, true, 200, Some(clock.clone()), None, 200_000);
    summary("200,000 units a call, plain calls", &big, d1);
    let (_, bigs, d2, _) = g1_growth_polled(800_000, 1, true, 200, Some(clock), Some(4096), 200_000);
    summary("200,000 units a call, polls of one chunk", &bigs, d2);
    let mut steps: Vec<_> = recs.iter().filter(|r| r.op != remedy_lua::TraceOp::Poll).collect();
    steps.sort_by_key(|r| std::cmp::Reverse(r.wall()));
    for r in steps.iter().take(5) {
        println!("slow step: call {} {:?} {} us, {} slots, {} fuel", r.call, r.op, r.wall().as_micros(), r.slots, r.fuel);
    }
    let around = slowest_around(&cpus, remedy_lua::TraceOp::Poll, 3);
    println!("{around}");
    assert_eq!((dropped, dropped_cpu), (0, 0), "the trace's buffer");
    assert!(most <= 16 * 4096, "work bound: a poll scanned {most} slots\n{around}");
    assert!(gc <= std::time::Duration::from_millis(2), "the collector's share: {} us in a poll\n{around}", gc.as_micros());
    assert!(worst <= std::time::Duration::from_millis(2), "latency target: a poll took {} us\n{around}", worst.as_micros());
}


/// SEC7-G1 step 3: a host function the script calls runs exactly once a call whatever the
/// call's polls: the count it keeps, the fuel and how far each call got are the same with
/// polls of one chunk as with none, per seed, both loops (a pause never runs an instruction
/// twice: one that paused before its work runs again without its step, one after it goes
/// on with the next).
#[test]
fn sec7_g1_a_host_function_runs_once_across_pauses() {
    use std::sync::atomic::{AtomicU64, Ordering};
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (false, 7)] {
        let run = |slots: Option<usize>| {
            let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), fast);
            let hits = std::sync::Arc::new(AtomicU64::new(0));
            let h = hits.clone();
            lua.register("count", move |_, _| Ok(vec![remedy_lua::Value::Int(h.fetch_add(1, Ordering::Relaxed) as i64 + 1)])).unwrap();
            lua.exec("T = {} for i = 1, 40000 do T['k' .. i] = {i} end W = setmetatable({}, {__mode = 'v'}) N = 40000", "=setup").unwrap();
            let f = lua
                .load(
                    "local j = 0 while true do j = j + 1 local k = 'k' .. (j % N + 1) local v = {j} T[k] = v W[#W + 1] = v \
                     if j % 3 == 0 then C = count() end if j % 40 == 0 then collectgarbage('step') end end",
                    "=churn",
                )
                .unwrap();
            lua.set_limits(lua.limits().fuel(150_000));
            let mut seen = Vec::new();
            let mut polls = 0;
            for _ in 0..30 {
                let r = match slots {
                    None => lua.call(f.value(), &[]),
                    Some(k) => {
                        let (r, p) = lua.gc_sliced_call(f.value(), &[], k);
                        polls += p;
                        r
                    }
                };
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
                seen.push((lua.fuel_left(), hits.load(Ordering::Relaxed)));
            }
            (seen, polls)
        };
        let (plain, _) = run(None);
        let (sliced, polls) = run(Some(4096));
        assert!(polls > 30, "fast {fast}, seed {seed}: no call paused ({polls} polls)");
        assert_eq!(sliced, plain, "fast {fast}, seed {seed}: fuel left and the host function's count after each call");
    }
}

/// SEC7-G1 step 3: a pending call dropped leaves the state usable: nothing else runs in
/// it while the call is pending, and once dropped what was reachable still is, and the
/// calls after behave as in a state that never paused (each stops at its fuel).
#[test]
fn sec7_g1_a_pending_call_dropped_leaves_the_state_usable() {
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40), fast);
        lua.exec("T = {} for i = 1, 60000 do T['k' .. i] = {i} end W = setmetatable({}, {__mode = 'v'}) N = 60000", "=setup").unwrap();
        let f = lua.load("local j = 0 while true do j = j + 1 local v = {j} T['k' .. (j % N + 1)] = v W[#W + 1] = v end", "=churn").unwrap();
        lua.set_limits(lua.limits().fuel(200_000));
        let mut dropped = 0;
        for _ in 0..40 {
            let _ = lua.gc_start_sliced(f.value(), &[], 4096);
            if lua.gc_is_pending() {
                // While pending, nothing else may run in it.
                let r = lua.exec("return 1", "=other");
                assert!(matches!(&r, Err(remedy_lua::Error::Host(m)) if m.contains("pending")), "fast {fast}: {r:?}");
                lua.gc_drop_pending();
                assert!(!lua.gc_is_pending());
                dropped += 1;
            }
        }
        assert!(dropped > 5, "fast {fast}: only {dropped} calls paused");
        // Calls after it run as calls do.
        for _ in 0..5 {
            let r = lua.call(f.value(), &[]);
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "fast {fast}: {r:?}");
        }
        lua.set_limits(lua.limits().fuel(1 << 40));
        let ok = lua
            .exec("collectgarbage() local n = 0 for i = 1, N do local v = T['k' .. i] if type(v) == 'table' and type(v[1]) == 'number' then n = n + 1 end end return n == N, n", "=check")
            .unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}: {ok:?}");
    }
}

/// SEC7-G1 step 3, the host's entry (`unstable-resumable`): `start_call`, `resume_call`
/// until ready, gives what `call` gives, fuel and results; `cancel_call` drops it.
#[cfg(feature = "unstable-resumable")]
#[test]
fn sec7_g1_start_call_gives_what_call_gives() {
    let setup = "T = {} for i = 1, 60000 do T['k' .. i] = {i} end N = 60000";
    let body = "local j = 0 while true do j = j + 1 T['k' .. (j % N + 1)] = {j} if j % 50 == 0 then collectgarbage('step') end end";
    let mut a = state(Limits::default().memory(512 << 20).fuel(1 << 40), true);
    let mut b = state(Limits::default().memory(512 << 20).fuel(1 << 40), true);
    for l in [&mut a, &mut b] {
        l.exec(setup, "=setup").unwrap();
        l.set_limits(l.limits().fuel(200_000));
    }
    let (fa, fb) = (a.load(body, "=b").unwrap(), b.load(body, "=b").unwrap());
    let mut paused = 0;
    for _ in 0..40 {
        let ra = a.call(fa.value(), &[]);
        let mut p = b.start_call(fb.value(), &[], 4096);
        while matches!(p, remedy_lua::CallPoll::Pending) {
            paused += 1;
            assert!(b.call_pending());
            p = b.resume_call();
        }
        let remedy_lua::CallPoll::Ready(rb) = p else { unreachable!() };
        assert_eq!(format!("{ra:?}"), format!("{rb:?}"));
        assert_eq!(a.fuel_left(), b.fuel_left());
    }
    assert!(paused > 0);
    // Dropped while pending: usable after.
    loop {
        if let remedy_lua::CallPoll::Pending = b.start_call(fb.value(), &[], 4096) {
            b.cancel_call();
            break;
        }
    }
    assert!(!b.call_pending());
    assert!(b.exec("return 1", "=after").is_ok());
}

/// SEC8-S1, segmented storage (wasm; here the `segmented-arrays` build, chunks of 1,024): a
/// slab past its first chunk grows by chunks, its objects never moved (a slab of 2M tables
/// doubled from 144 to 288 MiB in one `realloc`, a full copy on wasm), and every object is
/// where it was. Both paths.
#[cfg(feature = "segmented-arrays")]
#[test]
fn sec8_s1_a_slab_grows_without_moving_its_objects() {
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
        lua.exec("T = {} for i = 1, 4096 do T[i] = {i} end S = {} for i = 1, 4096 do S[i] = 's' .. i end", "=setup").unwrap();
        lua.slab_objects_moved();
        lua.exec("for i = 4097, 1 << 18 do T[i] = {i} S[i] = 's' .. i end", "=grow").unwrap();
        assert_eq!(lua.slab_objects_moved(), 0, "fast {fast}: objects moved");
        let ok = lua.exec("collectgarbage() for i = 1, 1 << 18 do if T[i][1] ~= i or S[i] ~= 's' .. i then return false end end return true", "=look").unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}");
    }
}

/// SEC8-F3's churn: 200,000 live tables, a weak-keyed `W`, each iteration a new table
/// written into both and a collector step every 50, one call of `fuel` at a time (each
/// stops on it). `X = 42` is the global read between calls.
fn f3_churn(fast: bool, seed: u64) -> (Lua, remedy_lua::Root) {
    let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), fast);
    lua.exec("X = 42 N = 0 T = {} for i = 1, 200000 do T[i] = {i} end W = setmetatable({}, {__mode = 'k'})", "=setup").unwrap();
    let f = lua
        .load("while true do N = N + 1 local t = {N} W[t] = N T[N % 200000 + 1] = t if N % 50 == 0 then collectgarbage('step') end end", "=churn")
        .unwrap();
    lua.set_limits(lua.limits().fuel(5_000));
    (lua, f)
}

/// SEC8-F3 (low): `get_global` made its name with `create_string`, an entry, and every
/// outermost entry first finishes an atomic phase a call before stopped in: while that did
/// not fit in the call's fuel the read failed with `Limit(Fuel)` (331 of 20,000 reads here
/// on 9a9e595). The name is looked up now, nothing entered: every read is 42.
#[test]
fn sec8_f3_a_host_read_does_not_wait_on_a_suspended_atomic_phase() {
    for fast in [true, false] {
        let (mut lua, f) = f3_churn(fast, 1);
        let mut failed = Vec::new();
        for call in 0..20_000 {
            assert!(lua.call(f.value(), &[]).is_err(), "each call stops on its fuel");
            match lua.get_global("X") {
                Ok(remedy_lua::Value::Int(42)) => {}
                r => failed.push((call, format!("{r:?}"))),
            }
        }
        assert!(failed.is_empty(), "fast {fast}: {} reads failed, the first {:?}", failed.len(), failed.first());
    }
}

/// SEC8-F3: a host read between calls changes nothing the script's calls do. (Entering, it
/// finished a suspended atomic phase, which the next call then did not: the script's
/// progress, so its fuel, followed whether the host had read.) Each call's progress is
/// read with `table_get`, which never entered.
#[test]
fn sec8_f3_host_reads_leave_the_scripts_fuel_alone() {
    for fast in [true, false] {
        for seed in [1u64, 0xdead_beef] {
            let mut runs = Vec::new();
            for read in [false, true] {
                let (mut lua, f) = f3_churn(fast, seed);
                lua.set_limits(lua.limits().fuel(1 << 40));
                let n = lua.create_string("N").unwrap();
                lua.set_limits(lua.limits().fuel(5_000));
                let g = lua.globals();
                let mut progress = Vec::new();
                for _ in 0..3_000 {
                    let _ = lua.call(f.value(), &[]);
                    if read {
                        assert!(matches!(lua.get_global("X"), Ok(remedy_lua::Value::Int(42))));
                        assert_eq!(lua.get_global("N").unwrap(), lua.table_get(g, n));
                    }
                    progress.push((lua.table_get(g, n), lua.fuel_left()));
                }
                runs.push(progress);
            }
            assert!(runs[0] == runs[1], "fast {fast}, seed {seed}: the host's reads changed the script's progress");
        }
    }
}

/// SEC8-F3: `get_global` looks its name up without making it; what it finds is what a key
/// made from the name finds (`table_get`), for short and long names, names that differ
/// from a global's only at the end, and names never made, under three seeds.
#[test]
fn sec8_f3_a_name_looked_up_is_the_name_made() {
    for seed in [1u64, 7, 0xdead_beef] {
        let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), true);
        let mut names: Vec<String> = Vec::new();
        for len in [1usize, 2, 7, 8, 9, 39, 40, 41, 64, 100, 1000, 70_000] {
            let base: String = (0..len).map(|i| (b'a' + (i * 7 % 26) as u8) as char).collect();
            let mut other = base.clone();
            other.pop();
            other.push('Z');
            names.push(base);
            names.push(other);
        }
        // A quarter of the names globals (the long ones too), a quarter made and kept
        // elsewhere, the rest never made.
        lua.exec("H = {}", "=h").unwrap();
        let held = match lua.get_global("H").unwrap() {
            remedy_lua::Value::Table(t) => t,
            v => panic!("{v:?}"),
        };
        for (i, n) in names.iter().enumerate() {
            if i % 4 == 0 {
                lua.set_global(n, remedy_lua::Value::Int(i as i64)).unwrap();
            } else if i % 4 == 1 {
                let k = lua.create_string(n).unwrap();
                lua.table_set(held, remedy_lua::Value::Int(i as i64), k).unwrap();
            }
        }
        lua.exec("collectgarbage()", "=gc").unwrap();
        let g = lua.globals();
        for (i, n) in names.iter().enumerate() {
            let read = lua.get_global(n).unwrap();
            let k = lua.create_string(n).unwrap();
            assert_eq!(read, lua.table_get(g, k), "seed {seed}, name {i} ({} bytes)", n.len());
            assert_eq!(read == remedy_lua::Value::Int(i as i64), i % 4 == 0, "seed {seed}, name {i}");
        }
        assert_eq!(lua.get_global("never made, nowhere").unwrap(), remedy_lua::Value::Nil);
    }
}

/// SEC8-F3: a read that does not enter still refuses while a call is pending (SEC7-G1).
#[cfg(feature = "unstable-resumable")]
#[test]
fn sec8_f3_a_read_is_refused_while_a_call_is_pending() {
    let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40), true);
    lua.exec("X = 42 T = {} for i = 1, 60000 do T['k' .. i] = {i} end", "=setup").unwrap();
    lua.set_limits(lua.limits().fuel(200_000));
    let f = lua.load("local j = 0 while true do j = j + 1 T['k' .. (j % 60000 + 1)] = {j} if j % 50 == 0 then collectgarbage('step') end end", "=b").unwrap();
    let mut seen = false;
    for _ in 0..40 {
        if let remedy_lua::CallPoll::Pending = lua.start_call(f.value(), &[], 4096) {
            assert!(lua.get_global("X").is_err());
            lua.cancel_call();
            seen = true;
            break;
        }
    }
    assert!(seen, "no call paused");
    assert!(matches!(lua.get_global("X"), Ok(remedy_lua::Value::Int(42))));
}

/// SEC8-F3 O2: every read-only host accessor that takes a name (`get_global`, and
/// `call_global`'s lookup, dotted or not) looks it up, not made: with the state stalled at
/// its memory cap, reading an absent name reports absence, interns nothing, spends no fuel
/// and leaves the collector where it was (`collector_status`: its phase, the fuel owed to
/// it, the floor on what its cycle still needs; with the account, what a step or an
/// allocation would move).
#[test]
fn sec8_f3_o2_reads_of_an_absent_name_at_the_memory_cap_change_nothing() {
    use remedy_lua::{Error, LimitKind, Value};
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(16 << 20).fuel(1 << 40), fast);
        let r = lua.exec("T = {} local i = 0 while true do i = i + 1 T[i] = {i} end", "=fill");
        assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "fast {fast}: {r:?}");
        let probe = |lua: &Lua| {
            let interned = lua.internal_sizes().into_iter().find(|e| e.0 == "interned").unwrap().1;
            (interned, lua.fuel_left(), lua.collector_status(), lua.memory_used())
        };
        let before = probe(&lua);
        let a = "absent, never made";
        assert_eq!(lua.get_global(a).unwrap(), Value::Nil, "fast {fast}");
        for path in [a.to_string(), format!("{a}.x"), format!("T.{a}"), format!("T.1.{a}")] {
            assert_eq!(lua.call_global(&path, &[]).unwrap(), None, "fast {fast}: {path}");
        }
        assert_eq!(probe(&lua), before, "fast {fast}");
    }
}

/// SEC8-F7's run's size: under 5 s for the four runs a test makes.
const F7_N: usize = 200_000;
const F7_CAP: usize = 128;
const F7_CALLS: usize = 1_000;

/// SEC8-F7's state: the churn's program set up under the limit, its function, and `W`,
/// at 20,000 units a call.
fn f7_state(seed: u64, fast: bool, n: usize, cap: usize) -> (Lua, remedy_lua::Root, remedy_lua::TableRef) {
    let mut lua = state(Limits::default().memory(cap << 20).fuel(1 << 40).hash_seed(seed), fast);
    let w = (2 * n).next_power_of_two();
    lua.exec(
        &format!("T = {{}} for i = 1, {n} do T['k' .. i] = {{i}} end W = setmetatable({{}}, {{__mode = 'v'}}) for i = 1, {w} do W[i] = false end for i = {w}, 1, -1 do W[i] = nil end N = {n}"),
        "=setup",
    )
    .unwrap();
    let f = lua
        .load(
            format!(
                "local j = 0 while true do j = j + 1 local k = 'k' .. (j % {n} + 1) local v = {{j}} T[k] = v W[#W + 1] = v \
                 if j % 7 == 0 then T['k' .. (N + 1)] = {{N + 1}} N = N + 1 end if j % 50 == 0 then collectgarbage('step') end end"
            ),
            "=churn",
        )
        .unwrap();
    lua.set_limits(lua.limits().fuel(20_000));
    let Ok(remedy_lua::Value::Table(wt)) = lua.get_global("W") else { panic!("W") };
    (lua, f, wt)
}

/// Under allocation pressure a short call can finish a script store between collector
/// steps. k2 is the first strong field replaced on every invocation of this workload.
#[test]
fn sec8_g3_short_calls_keep_replacing_live_values_under_pressure() {
    let mut reference = None;
    for (seed, fast) in [(1, true), (7, false)] {
        let (mut lua, f, _) = f7_state(seed, fast, F7_N, F7_CAP);
        let owner = lua.get_global("T").unwrap().as_table().unwrap();
        let first_key = lua.create_string("k2").unwrap();
        let counter = lua.create_string("N").unwrap();
        let mut previous = lua.table_get(owner, first_key);
        let mut stalled = 0;
        let mut progress = Vec::new();
        lua.gc_trace(Some(std::sync::Arc::new(|| std::time::Duration::ZERO)), 1 << 21);
        for _ in 0..F7_CALLS {
            let r = lua.call(f.value(), &[]);
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))));
            assert!(lua.memory_used() <= F7_CAP << 20);
            let current = lua.table_get(owner, first_key);
            let table = current.as_table().unwrap();
            assert!(lua.table_get(table, remedy_lua::Value::Int(1)).as_int().is_some());
            stalled = if current == previous { stalled + 1 } else { 0 };
            assert!(stalled <= 120, "{stalled} calls without a store");
            previous = current;
            progress.push(lua.table_get(lua.globals(), counter).as_int().unwrap());
        }
        let (trace, dropped) = lua.gc_trace_take();
        assert_eq!(dropped, 0);
        assert!(trace.iter().all(|r| r.op != remedy_lua::TraceOp::Full));
        if let Some(expected) = &reference {
            assert_eq!(&progress, expected);
        }
        reference = Some(progress);
    }
}

/// Measures newly appended weak values near the cap. Every append in f7_state stores
/// a fresh table; T is the only strong script owner of those values after a call ends.
/// The host snapshots contain unrooted, generation-checked handles.
#[test]
#[ignore = "SEC8-G3 retention measurement, 800,000 keys and a 512 MiB state"]
fn sec8_g3_newborn_retention() {
    use remedy_lua::{TableRef, TraceOp};
    use std::collections::HashSet;

    fn values(lua: &Lua, table: TableRef) -> HashSet<TableRef> {
        lua.table_pairs(table).into_iter().filter_map(|(_, value)| value.as_table()).collect()
    }

    const CAP: usize = 512 << 20;
    for seed in [1, 7] {
        let (mut lua, f, weak_table) = f7_state(seed, true, 800_000, CAP >> 20);
        let strong_table = lua.get_global("T").unwrap().as_table().unwrap();
        let mut snapshot = None;
        let mut measured = false;
        for call in 1..=3300 {
            let result = lua.call(f.value(), &[]);
            assert!(matches!(result, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))));
            let used = lua.memory_used();
            assert!(used <= CAP);
            if CAP - used > 32 << 20 {
                continue;
            }
            if snapshot.is_none() {
                snapshot = Some((call, values(&lua, weak_table)));
                lua.gc_trace(Some(std::sync::Arc::new(|| std::time::Duration::ZERO)), 1 << 19);
            }
            if CAP - used > 1024 {
                continue;
            }

            let (first, before) = snapshot.take().unwrap();
            let (trace, dropped) = lua.gc_trace_take();
            assert_eq!(dropped, 0);
            let starts = trace.iter().filter(|r| r.op == TraceOp::Start).count();
            let strong = values(&lua, strong_table);
            let weak = values(&lua, weak_table);
            let new: Vec<_> = weak.difference(&before).copied().collect();
            let weak_only = new.iter().filter(|v| !strong.contains(v)).count();

            lua.set_limits(lua.limits().fuel(1 << 40));
            lua.collect_garbage().unwrap();
            let remaining = values(&lua, weak_table);
            for v in &new {
                assert_eq!(remaining.contains(v), strong.contains(v), "seed {seed}, table {v:?}");
            }
            println!("seed {seed}, calls {first}..={call}, {starts} incremental cycle starts, {} new, {weak_only} weak-only cleared", new.len());
            measured = true;
            break;
        }
        if !measured {
            println!("seed {seed}: the workload stayed outside the final KiB through 3300 calls");
        }
    }
}

/// SEC8-F7's run: the G1 reproducer's core (`n` string keys, the weak `W`, the churn) under a
/// memory limit of `cap` MiB, `calls` calls of 20,000 units, traced. Per call: its fuel
/// left, its progress (`#W`) and the collector's fuel (every record but the call's own and
/// resizes'); the calls with a full collection; and `gc_owed_probe` after the calls, then
/// after a collection with fuel to spare.
fn f7_run(seed: u64, fast: bool, n: usize, cap: usize, calls: usize) -> (Vec<(u64, i64, u64)>, usize, [(u64, u64, u64); 2]) {
    use remedy_lua::TraceOp;
    let (mut lua, f, wt) = f7_state(seed, fast, n, cap);
    // Exercise the emergency path even if incremental collection would avoid the cap.
    lua.exec("collectgarbage('stop')", "=force-emergency").unwrap();
    lua.gc_trace(Some(std::sync::Arc::new(|| std::time::Duration::ZERO)), 1 << 21);
    let mut out = Vec::new();
    for _ in 0..calls {
        let r = lua.call(f.value(), &[]);
        assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
        out.push((lua.fuel_left(), lua.table_len(wt), 0));
    }
    let (recs, dropped) = lua.gc_trace_take();
    assert_eq!(dropped, 0, "the trace's buffer");
    let mut full = std::collections::BTreeSet::new();
    for r in recs.iter().filter(|r| !matches!(r.op, TraceOp::Poll | TraceOp::Resize)) {
        if r.op == TraceOp::Full {
            full.insert(r.call);
        }
        if let Some(o) = out.get_mut(r.call as usize - 1) {
            o.2 += r.fuel;
        }
    }
    let after = lua.gc_owed_probe();
    lua.set_limits(lua.limits().fuel(1 << 40));
    lua.exec("collectgarbage()", "=settle").unwrap();
    (out, full.len(), [after, lua.gc_owed_probe()])
}

/// SEC8-F7 (medium): a full collection stopped by the call's fuel
/// dropped the work it had done past it. `full_gc` charged after each piece of 2^14 units of
/// work; past the call's fuel the charge was refused (the call stopped) and the units past it
/// were neither charged nor carried. At the memory limit, where the emergency collection
/// spans hundreds of calls, that was work done free in nearly every one (up to a piece and an
/// object past the call's fuel), by an amount that followed where pieces fell in the order
/// objects are reached: the seed (5,766,987 units against 5,773,106 for seeds 1 and 7 over
/// 3,300 calls of the G1 reproducer at 512 MiB). Now a piece is no bigger than what the call
/// can pay, and every collector charge past the call's fuel is owed (`owe_gc`), charged by
/// the collector's next steps or collection, at the next cycle's start when a cycle ended
/// with some owed (`gc_mark_over` was zeroed there).
#[test]
fn sec8_f7_a_full_collection_stopped_on_fuel_owes_what_it_did() {
    for seed in [1u64, 7] {
        for fast in [true, false] {
            let (_, full, [after, settled]) = f7_run(seed, fast, F7_N, F7_CAP, F7_CALLS);
            assert!(full > 20, "seed {seed}, fast {fast}: {full} calls with a full collection (the limit is to be reached)");
            // Owed past the calls' fuel (dropped before), some of it carried over a cycle's
            // end and charged at the next one's start, and all of it charged by the end.
            assert!(after.0 > 0, "seed {seed}, fast {fast}: nothing owed {after:?}");
            assert!(after.1 > 0, "seed {seed}, fast {fast}: no cycle ended owing {after:?}");
            assert_eq!(settled.2, 0, "seed {seed}, fast {fast}: still owed after a collection with fuel to spare {settled:?}");
        }
    }
}

/// SEC8-F7: each call's fuel, progress and collector fuel are the same whatever the seed,
/// on both paths, through emergency collections that stop on the fuel (the work they owe
/// charged in the records of the calls that pay it: a suspended atomic phase's by the step
/// that resumes it).
#[test]
fn sec8_f7_per_call_collector_fuel_follows_no_seed() {
    let (base, full, _) = f7_run(1, true, F7_N, F7_CAP, F7_CALLS);
    assert!(full > 20, "{full} calls with a full collection");
    for (seed, fast) in [(1u64, false), (7, true), (7, false)] {
        let (other, _, _) = f7_run(seed, fast, F7_N, F7_CAP, F7_CALLS);
        let first = (0..base.len()).find(|&i| base[i] != other[i]);
        assert!(first.is_none(), "seed {seed}, fast {fast}: call {:?} (fuel left, #W, collector fuel) {:?} against seed 1 fast {:?}", first.map(|i| i + 1), first.map(|i| other[i]), first.map(|i| base[i]));
    }
}

/// Overwrite children of a string-keyed table while marking. Which old children
/// survive affects the next cycle's fuel. Exercise both a whole table and one
/// that spans several collector steps, comparing progress at every call boundary.
#[test]
fn sec7_g2_the_next_cycle_charges_the_same_whatever_the_seed() {
    for (entries, calls) in [(500, 20), (9_000, 100)] {
        let mut seen = Vec::new();
        for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (false, 7), (true, 0xdead_beef), (true, 42)] {
            let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), fast);
            lua.exec(&format!("S = {{}} for i = 1, {entries} do S['k' .. i] = {{v = {{i}}}} end"), "=setup").unwrap();
            let f = lua.load(&format!("J = J or 0 while true do J = J + 1 local i = J * 37 % {entries} + 1 S['k' .. i].v = {{J}} end"), "=churn").unwrap();
            lua.set_limits(lua.limits().fuel(100_000));
            let mut progress = Vec::new();
            for _ in 0..calls {
                let r = lua.call(f.value(), &[]);
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
                progress.push(lua.get_global("J").unwrap());
            }
            seen.push(((fast, seed), progress));
        }
        for s in &seen {
            assert_eq!(s.1, seen[0].1, "{entries} entries, {:?} against {:?}: J after each call", s.0, seen[0].0);
        }
    }
}

/// SEC8-F3 O5 (with SEC8-G3's known limitation): `collector_status` tells a host where the
/// collector stands and what fuel it is known to need, owed apart from still to come.
/// Idle with nothing owed when no cycle runs; every phase seen over the churn at the limit
/// (marking, a suspended atomic phase, sweeping), each saying more may be found; owed fuel
/// that the next call's collector charges first (every call whose owed fuel was its whole
/// fuel made no progress, and its collector charged at least that); a floor that the
/// collector's charges to the cycle's end meet. Reading it moves nothing.
#[test]
fn sec8_o5_the_collector_status_tells_owed_from_to_come() {
    use remedy_lua::{CollectorPhase, TraceOp};
    let mut idle = state(Limits::default().fuel(1 << 40), true);
    idle.exec("X = {} for i = 1, 1000 do X[i] = {} end collectgarbage() collectgarbage()", "=idle").unwrap();
    let s = idle.collector_status();
    assert_eq!((s.phase, s.owed_fuel, s.remaining_fuel_at_least, s.more_may_be_found), (CollectorPhase::Idle, 0, 0, false), "{s:?}");
    for fast in [true, false] {
        let (mut lua, f, wt) = f7_state(1, fast, F7_N, F7_CAP);
        lua.gc_trace(Some(std::sync::Arc::new(|| std::time::Duration::ZERO)), 1 << 21);
        let mut before = Vec::new();
        let mut border = Vec::new();
        for _ in 0..F7_CALLS {
            let (mem, fuel) = (lua.memory_used(), lua.fuel_left());
            let s = lua.collector_status();
            assert_eq!((lua.memory_used(), lua.fuel_left()), (mem, fuel), "reading the status moved something");
            assert_eq!(s.more_may_be_found, s.phase != CollectorPhase::Idle, "{s:?}");
            if s.phase == CollectorPhase::Idle {
                assert_eq!(s.remaining_fuel_at_least, 0, "{s:?}");
            }
            before.push(s);
            let _ = lua.call(f.value(), &[]);
            border.push(lua.table_len(wt));
        }
        let phases: std::collections::BTreeSet<_> = before.iter().map(|s| format!("{:?}", s.phase)).collect();
        for p in ["Marking", "AtomicSuspended", "Sweeping"] {
            assert!(phases.contains(p), "fast {fast}: {p} never seen: {phases:?}");
        }
        let (recs, dropped) = lua.gc_trace_take();
        assert_eq!(dropped, 0);
        let mut gc = vec![0u64; F7_CALLS];
        for r in recs.iter().filter(|r| !matches!(r.op, TraceOp::Poll | TraceOp::Resize)) {
            gc[r.call as usize - 1] += r.fuel;
        }
        let (mut owed_calls, mut floors) = (0, 0);
        for (i, s) in before.iter().enumerate() {
            if s.owed_fuel > 0 {
                owed_calls += 1;
                assert!(gc[i] >= s.owed_fuel.min(20_000) - 1, "fast {fast}: call {}: owed {} but its collector charged {}", i + 1, s.owed_fuel, gc[i]);
            }
            if s.owed_fuel >= 20_000 && i > 0 {
                assert_eq!(border[i], border[i - 1], "fast {fast}: call {}: a whole call owed, yet the script progressed", i + 1);
            }
            // A floor: the collector charges at least that much from this call to the end of
            // the cycle (the first status after a sweep that is not sweeping). (It can rise
            // as the sweep goes: objects made since are in slabs it has not reached.)
            if s.phase != CollectorPhase::Idle {
                let end = (i + 1..before.len()).find(|&j| before[j].phase == CollectorPhase::Idle || (before[j - 1].phase == CollectorPhase::Sweeping && before[j].phase != CollectorPhase::Sweeping));
                if let Some(j) = end {
                    let charged: u64 = gc[i..j].iter().sum();
                    assert!(charged >= s.remaining_fuel_at_least, "fast {fast}: call {}: {s:?}, but {charged} charged to the cycle's end (call {})", i + 1, j);
                    floors += 1;
                }
            }
        }
        assert!(floors > 0, "fast {fast}: no floor checked");
        assert!(owed_calls > 0, "fast {fast}: nothing ever owed between calls");
    }
}
