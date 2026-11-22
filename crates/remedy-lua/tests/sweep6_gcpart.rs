//! Sweep 6, S6GC-2e's incremental half: while marking is incremental a
//! table of more than 4,096 slots is traversed that many slots a step, with the script
//! running between chunks. Every write into it then marks what it overwrites and what it
//! writes (and what it moves); a rebuild starts the traversal over. These tests write into
//! such a table every way a script or the host can between single collector steps, and
//! check with weak-table probes that nothing it holds was freed: each object made is also
//! put in a weak table, so an object marking missed is cleared from there (and freed) while
//! the table still holds it. Each runs with the fast loop on and off under three seeds: the
//! fuel must be the same with the fast loop on and off, and under every seed.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Libs, Limits, Lua, Value};

/// The probes and the driver. `new()` makes an object `{id}` and puts it in a weak probe
/// (64 of them, so each stays below the size traversed in chunks); `take()` takes one out of
/// the pool `U`, made before the cycle and traversed after `T`: still unmarked, and from
/// then on only where it is stored, so a write into `T`'s traversed part that did not mark
/// it would have it freed (objects made during a cycle are marked as they are made);
/// `check(t)` counts what `t` holds (keys, values, its metatable and its `__index`) that is
/// no longer the object its probe holds; `run(T, mutate, steps)` runs `mutate(T, s)` before
/// each of the first 60 of `steps` single steps of the collector (while `T` is traversed
/// in chunks, 12 chunks of 8 steps or so: writes after its traversal send it to be
/// traversed again whole in the atomic step, which would mark what a write missed),
/// checking `T` after each cycle and at the end.
const PRELUDE: &str = r#"
collectgarbage('stop')
SH = {} for s = 1, 64 do SH[s] = setmetatable({}, {__mode = 'v'}) end
ID = 0
function new() ID = ID + 1 local o = {ID} SH[ID % 64 + 1][ID] = o return o end
function pool() U = {} for i = 1, 20000 do U[i] = new() end UN = 20000 end
function take()
  if UN == 0 then return new() end
  local v = U[UN] U[UN] = nil UN = UN - 1 return v
end
local function ok(v)
  if type(v) ~= 'table' then return true end
  local i = rawget(v, 1)
  return type(i) == 'number' and SH[i % 64 + 1][i] == v
end
function check(t)
  local bad = 0
  for k, v in next, t do
    if not ok(k) or not ok(v) then bad = bad + 1 end
  end
  local m = getmetatable(t)
  if m and (not ok(m) or not ok(rawget(m, '__index'))) then bad = bad + 1 end
  return bad
end
function run(T, mutate, steps)
  pool()
  collectgarbage()
  local cycles, bad = 0, 0
  for s = 1, steps do
    if s <= 60 then mutate(T, s) end
    if collectgarbage('step') then cycles = cycles + 1 bad = bad + check(T) end
  end
  collectgarbage()
  return cycles, bad + check(T)
end
-- An array part of 20,000 objects (32,768 slots) and 8,000 string keys: 10 chunks.
function mixed()
  local T = {}
  for i = 1, 20000 do T[i] = new() end
  for i = 1, 8000 do T['h' .. i] = new() end
  return T
end
-- 30,000 string keys and no array part (65,536 nodes): 16 chunks, all hash nodes; and
-- `GK`, 24,000 more keys, made before (so adding them allocates nothing, which would make
-- the steps after bigger).
function hashy()
  local T = {}
  for i = 1, 30000 do T['h' .. i] = new() end
  GK = {} for i = 1, 24000 do GK[i] = 'g' .. i end
  return T
end
function pick(s, j, n) return (s * 7919 + j * 104729) % n + 1 end
"#;

/// The states each case runs in: the fast loop on and off under each of three seeds.
const STATES: [(bool, u64); 6] =
    [(true, 1), (false, 1), (true, 7), (false, 7), (true, 0xdead_beef), (false, 0xdead_beef)];

/// `(cycles, fuel)` per state: the same with the fast loop on and off under each seed, and
/// (when `seeds`) under every seed.
fn assert_same(name: &str, seen: &[((bool, u64), i64, u64)], seeds: bool) {
    for pair in seen.chunks(2) {
        assert!(
            pair[0].1 == pair[1].1 && pair[0].2 == pair[1].2,
            "{name}: fuel or cycles differ between the fast loop and the full path: {seen:?}"
        );
    }
    if seeds {
        assert!(
            seen.iter().all(|s| s.1 == seen[0].1 && s.2 == seen[0].2),
            "{name}: fuel or cycles differ between seeds: {seen:?}"
        );
    }
}

/// Runs `body` (which returns `cycles, bad`) after the prelude in each state; asserts
/// nothing was freed, marking finished cycles, and fuel agrees (`assert_same`).
fn same_everywhere(name: &str, body: &str, seeds: bool) {
    same_in(&STATES, "", name, body, seeds);
}

/// `same_everywhere` in `states`, with `setup` run first.
fn same_in(states: &[(bool, u64)], setup: &str, name: &str, body: &str, seeds: bool) {
    let mut seen = Vec::new();
    for &(fast, seed) in states {
        let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(seed), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(setup, "=setup").unwrap();
        lua.exec(PRELUDE, "=prelude").unwrap();
        let r = lua
            .exec(body, &format!("={name}"))
            .unwrap_or_else(|e| panic!("{name}, fast {fast}, seed {seed}: {e:?}"));
        // (Each host call starts with the whole budget: its fuel is the budget less what it
        // left.)
        let used = lua.limits().fuel - lua.fuel_left();
        let (cycles, bad) = match r.as_slice() {
            [Value::Int(c), Value::Int(b), ..] => (*c, *b),
            _ => panic!("{name}: {r:?}"),
        };
        assert_eq!(bad, 0, "{name}, fast {fast}, seed {seed}: {bad} entries freed while the table held them");
        assert!(cycles >= 1, "{name}, fast {fast}, seed {seed}: no cycle finished");
        seen.push(((fast, seed), cycles, used));
    }
    assert_same(name, &seen, seeds);
}

/// Stores of every kind the instructions make: array slots and string fields overwritten,
/// new keys (which move colliding entries and rebuild the part as it fills), entries
/// removed and stored elsewhere, swaps, integer keys outside the array part (negative, past
/// it, floats), objects as keys.
#[test]
fn s6gc_2e_instruction_stores_into_a_table_traversed_in_chunks() {
    let cases = [
        ("array", "T[pick(s, j, 20000)] = take()"),
        ("hash", "T['h' .. pick(s, j, 8000)] = take()"),
        ("new_keys", "for q = 1, 8 do T['n' .. s .. '_' .. j .. '_' .. q] = take() end"),
        (
            "remove_and_store_elsewhere",
            "local a = 'h' .. pick(s, j, 8000) local v = T[a] T[a] = nil T['d' .. s .. '_' .. j] = v \
             local i = pick(s, j + 9, 20000) local w = T[i] T[i] = nil T[40000 + s * 8 + j] = w",
        ),
        (
            "swaps",
            "local i, k = pick(s, j, 20000), pick(s, j + 50, 20000) T[i], T[k] = T[k], T[i] \
             local a, b = 'h' .. pick(s, j, 8000), 'h' .. pick(s, j + 50, 8000) T[a], T[b] = T[b], T[a]",
        ),
        (
            "integer_keys",
            "T[-(s * 8 + j)] = take() T[100000 + s * 8 + j] = take() T[s + j / 16 + 0.5] = take()",
        ),
        ("object_keys", "T[take()] = take()"),
    ];
    for (name, store) in cases {
        same_everywhere(
            name,
            &format!("return run(mixed(), function(T, s) for j = 1, 8 do {store} end end, 500)"),
            true,
        );
    }
}

/// The library's writes: `rawset`, `table.insert` and `table.remove` in the middle (each
/// shifting entries), `table.move` within the table, `table.sort` (its comparator steps the
/// collector, so the sort's writes land mid-traversal), and `setmetatable` with a new
/// metatable (and `__index`) only the table holds. (`table.concat` and `table.unpack`
/// write nothing.)
#[test]
fn s6gc_2e_library_stores_into_a_table_traversed_in_chunks() {
    let cases = [
        (
            "rawset",
            "for j = 1, 8 do rawset(T, pick(s, j, 20000), take()) rawset(T, 'r' .. s .. '_' .. j, take()) end",
        ),
        (
            "insert_remove",
            "table.insert(T, 19990, take()) table.insert(T, 19000 + s % 900, take()) table.remove(T, 19980) table.remove(T, 18000 + s % 900)",
        ),
        (
            "append_pop",
            "for j = 1, 8 do table.insert(T, take()) end table.remove(T) local x = table.remove(T) T[pick(s, 1, 20000)] = x",
        ),
        (
            "move",
            "table.move(T, pick(s, 1, 19000), pick(s, 1, 19000) + 15, pick(s, 2, 19000)) T[pick(s, 3, 20000)] = take()",
        ),
        (
            "sort",
            "if s % 60 == 5 then local n = 0 math.randomseed(s) \
             table.sort(T, function(a, b) n = n + 1 if n % 4000 == 0 then collectgarbage('step') end return a[1] > b[1] end) end \
             T[pick(s, 1, 20000)] = take()",
        ),
        (
            "setmetatable",
            "if s % 7 == 0 then local m = take() m.__index = take() setmetatable(T, m) end T[pick(s, 1, 20000)] = take()",
        ),
    ];
    for (name, store) in cases {
        same_everywhere(name, &format!("return run(mixed(), function(T, s) {store} end, 500)"), true);
    }
}

/// Rebuilds while the table is traversed in chunks: string keys removed and as many new
/// ones added (the hash part is rebuilt), the array part grown past its size, and integer
/// keys moving into it, several times a cycle. While the traversal is in the array part a
/// rebuild moves nothing behind it; in the hash nodes (`hashy`) it starts over.
#[test]
fn s6gc_2e_rebuilds_of_a_table_traversed_in_chunks() {
    let cases = [
        (
            "hash_rebuilds",
            "if s % 25 == 0 then for j = 1, 3000 do T['h' .. pick(s, j, 8000)] = nil end \
             for j = 1, 3000 do T['g' .. s .. '_' .. j] = take() end end T['h' .. pick(s, 1, 8000)] = take()",
            true,
        ),
        ("array_growth", "for j = 1, 250 do T[#T + 1] = take() end", true),
        (
            "into_the_array",
            "if s % 30 == 0 then for j = 1, 2000 do T[32768 + (s // 30) * 2000 + j] = take() end end T[pick(s, 1, 20000)] = take()",
            true,
        ),
    ];
    for (name, store, seeds) in cases {
        same_everywhere(name, &format!("return run(mixed(), function(T, s) {store} end, 500)"), seeds);
    }
    // (New keys past three quarters of the nodes, 49,152, place every key again: in the
    // 5th batch, with the traversal in the hash nodes.)
    let hashy = "if s % 10 == 0 then for j = 1, 4000 do T['h' .. pick(s, j, 30000)] = nil end \
                 for j = 1, 4000 do T[GK[(s // 10 - 1) * 4000 + j]] = take() end end \
                 for j = 1, 8 do T['h' .. pick(s, j, 30000)] = take() end";
    same_everywhere("hash_nodes_rebuilt", &format!("return run(hashy(), function(T, s) {hashy} end, 500)"), true);
}

/// A constructor of 6,000 items whose items step the collector: the table is reached and
/// traversed in chunks while it is still being filled, 50 items a `SETLIST`.
#[test]
fn s6gc_2e_a_constructor_fills_a_table_traversed_in_chunks() {
    let items = vec!["f()"; 6000].join(", ");
    let body = format!(
        "local c = 0 local function f() c = c + 1 local o = take() if c % 40 == 0 then collectgarbage('step') end return o end \
         pool() collectgarbage() collectgarbage('step') \
         local T = {{{items}}} return run(T, function(T, s) T[pick(s, 1, 6000)] = take() end, 300)"
    );
    same_everywhere("constructor", &body, true);
    // COV-5: a library one field bigger, under more seeds. The fuel followed the seed
    // (2,542,794 against 2,542,791): when the atomic step comes follows the order objects
    // are reached in, and clearing the dead stack there was charged by where its live part
    // ended.
    let mut states = Vec::new();
    for seed in [0, 7, u64::MAX, super::common::TEST_SEED] {
        states.extend([(true, seed), (false, seed)]);
    }
    same_in(&states, "math.cov5 = 0", "constructor_with_a_library_field", &body, true);
    // And objects made at every step, at varying call depth: closures over a local at
    // each level (open upvalues). They are made gray while marking is incremental, and
    // how many follows when marking ends, so the seed; traversed as collector work, the
    // fuel followed it (3,033,742 against 3,032,173). They pay as they are made now.
    let items = vec!["f()"; 6000].join(", ");
    let deep = format!(
        "local c = 0 local function deep(d) local u = d local g = function() return u end if d > 0 then local r = deep(d - 1) return r end return take() end \
         local function f() c = c + 1 local o = deep(c % 13) if c % 40 == 0 then collectgarbage('step') end return o end \
         pool() collectgarbage() collectgarbage('step') \
         local T = {{{items}}} return run(T, function(T, s) T[pick(s, 1, 6000)] = deep(s % 11) end, 300)"
    );
    same_in(&states, "", "objects_made_while_marking", &deep, true);
    // Construction ends while the table is still traversed in chunks, with its list
    // written behind the traversal (4,000 fields first make it more slots than the list;
    // the collector steps at three items only: a cycle starts, the first chunk passes the
    // whole array part, and the second does not come before the end). Nothing writes into
    // it after, so nothing sends it to be traversed again: what a `SETLIST` did not mark
    // is freed.
    let fields: Vec<String> = (1..=4000).map(|i| format!("x{i} = 1")).collect();
    let items = vec!["f()"; 3000].join(", ");
    let body = format!(
        "local c = 0 local function f() c = c + 1 local o = take() if c == 100 or c == 120 or c == 140 then collectgarbage('step') end return o end \
         pool() collectgarbage() \
         local T = {{{}, {items}}} return run(T, function() end, 300)",
        fields.join(", ")
    );
    same_everywhere("constructor_ends_mid_traversal", &body, true);
}

/// The host's writes (`table_set`, raw, and `set`, with metamethods) between single steps,
/// into a table traversed in chunks, of objects only it then holds. The host holds the
/// table by a root, so it is reached first (as a script's local is): reached through the
/// globals, when its traversal starts would follow their hash order, and with it what
/// writes before then leave unmarked (as for any table, whole or in chunks). Its fuel is
/// the same under every seed: each entry from the host dropped, uncharged, the marking
/// work the last step did past its budget (`gc_mark_over`), and how much that was followed
/// the order objects are reached in (a few hundred units across seeds); it is owed now.
#[test]
fn s6gc_2e_host_stores_into_a_table_traversed_in_chunks() {
    let mut seen = Vec::new();
    for (fast, seed) in STATES {
        let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(seed), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(PRELUDE, "=prelude").unwrap();
        let t = match lua.exec("local T = mixed() pool() collectgarbage() return T", "=build").unwrap()[..] {
            [Value::Table(t)] => t,
            ref v => panic!("{v:?}"),
        };
        let _held = lua.root(Value::Table(t));
        let check = lua.load("collectgarbage() return check(...)", "=check").unwrap();
        let new = lua.load("return take()", "=take").unwrap();
        let step = lua.load("return collectgarbage('step')", "=step").unwrap();
        // Every call's fuel, summed (each starts with the whole budget: the fuel left after
        // the last call alone was that call's).
        let mut used = 0u64;
        let mut call = |lua: &mut Lua, f: Value, a: &[Value]| {
            let r = lua.call(f, a).unwrap();
            used += lua.limits().fuel - lua.fuel_left();
            r
        };
        let mut cycles = 0i64;
        for s in 0..500i64 {
            // (Only while it is traversed in chunks, as `run` does.)
            for j in 0..if s < 60 { 4i64 } else { 0 } {
                let v = call(&mut lua, new.value(), &[])[0];
                lua.table_set(t, Value::Int((s * 7919 + j * 104_729) % 20000 + 1), v).unwrap();
                let v = call(&mut lua, new.value(), &[])[0];
                let k = lua.create_string(format!("x{s}_{j}")).unwrap();
                lua.table_set(t, k, v).unwrap();
                let v = call(&mut lua, new.value(), &[])[0];
                let k = lua.create_string(format!("h{}", (s * 31 + j) % 8000 + 1)).unwrap();
                lua.set(Value::Table(t), k, v).unwrap();
            }
            if call(&mut lua, step.value(), &[])[0] == Value::Bool(true) {
                cycles += 1;
            }
        }
        let bad = call(&mut lua, check.value(), &[Value::Table(t)]);
        assert_eq!(bad, [Value::Int(0)], "fast {fast}, seed {seed}: entries freed while the table held them");
        assert!(cycles >= 1, "fast {fast}, seed {seed}: no cycle finished");
        seen.push(((fast, seed), cycles, used));
    }
    assert_same("host", &seen, true);
}

/// A step's budget follows the debt, so the debt of a big allocation (or of many while
/// the collector was stopped) made one step of millions of units, done in one go past the
/// call's fuel (40 ms for an array part doubling in
/// `s6gc_2e_a_big_table_is_marked_a_chunk_a_step`; 35 to 45 ms here). A step now does at
/// most what the call's fuel still pays for and leaves the rest owed, done by the steps
/// after it: no call runs long, the cycle still ends, and fuel is the same with the fast
/// loop on and off.
#[test]
fn s6gc_2e_a_step_does_no_more_than_the_call_pays_for() {
    let mut runs = Vec::new();
    for fast in [true, false] {
        // The ratio of each of three fresh runs: its longest call to its typical one. The
        // smallest is bounded: a call too long by the step it ran is so in every run, while
        // one slowed by the tests running beside this one (thread time counts their cache
        // and memory traffic) is not.
        let mut ratios = Vec::new();
        for _ in 0..3 {
            let mut lua = Lua::new(Limits::default().memory(512 << 20).fuel(1 << 40), Libs::ALL);
            lua.set_fast_loop(fast);
            // A million tables to mark, a cycle started, then 128 MiB allocated with the
            // collector stopped.
            lua.exec("T = {} for i = 1, 1000000 do T[i] = {i} end collectgarbage() collectgarbage('step') collectgarbage('stop') S = {}", "=setup").unwrap();
            for _ in 0..8 {
                lua.exec("S[#S + 1] = string.rep('x', 16 << 20)", "=alloc").unwrap();
            }
            let restart = lua.load("collectgarbage('restart') local t while true do t = {} end", "=restart").unwrap();
            let churn = lua.load("local t while true do t = {} end", "=churn").unwrap();
            lua.set_limits(lua.limits().fuel(200_000));
            let mut fuel = Vec::new();
            let mut times = Vec::new();
            let _quiet = super::common::measuring();
            for i in 0..60 {
                let f = if i == 0 { &restart } else { &churn };
                let start = remedy_lua_oracle::thread_cpu();
                let r = lua.call(f.value(), &[]);
                times.push(remedy_lua_oracle::thread_cpu().saturating_sub(start));
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
                fuel.push(lua.fuel_left());
            }
            times.sort_unstable();
            let (typical, longest) = (times[times.len() / 2], times[times.len() - 1]);
            ratios.push((longest * 10 / typical.max(1), longest / 1000, typical / 1000));
            lua.set_limits(lua.limits().fuel(1 << 40));
            let left = lua
                .exec("local n = 0 for i = 1, #T do if T[i][1] == i then n = n + 1 end end return n, #S[8]", "=check")
                .unwrap();
            assert_eq!(left, [Value::Int(1_000_000), Value::Int(16 << 20)], "fast {fast}");
            runs.push(fuel);
        }
        // (Measured: the longest at 1.5 times the typical call, a few milliseconds; 7 times,
        // 36 ms, before.)
        let best = ratios.iter().min().copied().unwrap_or_default();
        assert!(best.0 < 30, "fast {fast}: the longest call {} us, the typical one {} us (of three runs: {ratios:?})", best.1, best.2);
    }
    assert!(runs.iter().all(|r| *r == runs[0]), "fuel differs between runs, or between the fast loop and the full path");
}

/// S6GC-2e-R: the atomic phase read only the host's interrupt as it went (every 16,384
/// units), and a marking or sweeping step neither it nor the deadline's clock: with ample
/// fuel, the step that ran an atomic phase over an ephemeron chain ran tens of
/// milliseconds without a read of its clock, so as far past its time limit. They now look
/// at the deadline as often as the atomic phase looks at the interrupt, every 4,096 units
/// (16,384 left 4 to 7 ms between reads on a loaded machine). A clock that notes
/// when it is read: the longest gap between two reads, over two cycles of a 300,000-entry
/// ephemeron chain stepped to their end.
#[test]
fn s6gc_2e_the_collector_reads_the_deadline_often() {
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(
            "local mt = {__mode = 'k'} E = {} for j = 1, 64 do E[j] = setmetatable({}, mt) end \
             local ks = {} for i = 1, 300001 do ks[i] = {} end \
             for i = 300000, 1, -1 do E[i % 64 + 1][ks[i]] = ks[i + 1] end ROOT = ks[1] ks = nil",
            "=setup",
        )
        .unwrap();
        let reads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let noted = std::sync::Arc::clone(&reads);
        let start = std::time::Instant::now();
        let clock: remedy_lua::Clock = std::sync::Arc::new(move || {
            if let Ok(mut r) = noted.lock() {
                r.push(remedy_lua_oracle::thread_cpu());
            }
            start.elapsed()
        });
        lua.set_time_limit(Some((clock, std::time::Duration::from_secs(3600))));
        let steps = lua.load("local n = 0 while n < 2 do if collectgarbage('step') then n = n + 1 end end", "=steps").unwrap();
        let _quiet = super::common::measuring();
        reads.lock().unwrap().clear();
        lua.call(steps.value(), &[]).unwrap();
        let r = reads.lock().unwrap().clone();
        let gap = r.windows(2).map(|w| w[1].saturating_sub(w[0])).max().unwrap_or(0);
        // (Measured: about 0.3 ms, some 1.5 ms with the machine busy; before, tens.)
        assert!(gap < 2_000_000, "fast {fast}: {} us between two reads of the deadline ({} reads)", gap / 1000, r.len());
        let left = lua
            .exec("local n, k, i = 0, ROOT, 1 while E[i % 64 + 1][k] do k = E[i % 64 + 1][k] n = n + 1 i = i + 1 end return n", "=walk")
            .unwrap();
        assert_eq!(left, [Value::Int(300_000)], "fast {fast}: the chain");
    }
}

/// S6R-2: a second rebuild of the table traversed in chunks marked the rest of it whole,
/// in one step: 4.4 ms at 300,000 keys, 37 ms at 1.5 million, growing with the table
/// (here 516,096 slots at once).
/// Every rebuild starts it over in chunks now, and the fuel stays the same under every
/// seed and with the fast loop on and off.
#[test]
fn s6r_2_a_table_rebuilt_twice_while_traversed_stays_in_chunks() {
    // (T a local: reached at once from the stack, so the churn lands inside its traversal;
    // each churn's new keys rebuild its hash part.)
    let prog = "collectgarbage('stop') local G = {} for i = 1, 200000 do G[i] = 'g' .. i end \
                local T = {} for i = 1, 200000 do T['h' .. i] = {i} end collectgarbage() \
                local gi = 0 local function churn(n) for j = 1, n do gi = gi + 1 local k = G[gi] T[k] = true T[k] = nil end end \
                for s = 1, 1500 do if s == 20 or s == 40 then churn(80000) end collectgarbage('step') end \
                local n = 0 for i = 1, 200000 do if T['h' .. i][1] == i then n = n + 1 end end return n";
    let mut fuel = Vec::new();
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (true, 0xdead_beef)] {
        let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.gc_most_slots_at_once();
        let r = lua.exec(prog, "=t").unwrap();
        assert_eq!(r, [Value::Int(200_000)], "fast {fast}, seed {seed}");
        let most = lua.gc_most_slots_at_once();
        assert!(most <= 4096, "fast {fast}, seed {seed}: {most} slots marked at once");
        fuel.push((1u64 << 40) - lua.fuel_left());
    }
    assert!(fuel.iter().all(|f| *f == fuel[0]), "fuel differs: {fuel:?}");
}

/// S6GC: a slab's free tail was cut in one go when its sweep reached the end, its room
/// given back with it: 2,000,000 dead tables made 50 to 66 ms without a look at the
/// deadline (25 ms at 1,000,000). Cut a piece at a time now, each piece collector work,
/// with a look between them. A clock that notes when it is read: the longest stretch
/// between two reads (or from the call's start or to its end) in a full collection after
/// 1,000,000 tables died. (Measured: 1 to 2 ms; 25 ms before. Stepped, the stretches
/// end where the fuel's slices do, and follow what a unit of sweeping costs.)
#[test]
fn s6gc_a_slab_is_cut_a_piece_at_a_time() {
    let mut fuel = Vec::new();
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7)] {
        let mut lua = Lua::new(Limits::default().memory(2 << 30).fuel(1 << 40).hash_seed(seed), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec("collectgarbage('stop') local t = {} for i = 1, 1000000 do t[i] = {} end t = nil", "=setup").unwrap();
        let reads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let noted = std::sync::Arc::clone(&reads);
        let start = std::time::Instant::now();
        let clock: remedy_lua::Clock = std::sync::Arc::new(move || {
            if let Ok(mut r) = noted.lock() {
                r.push(remedy_lua_oracle::thread_cpu());
            }
            start.elapsed()
        });
        lua.set_time_limit(Some((clock, std::time::Duration::from_secs(3600))));
        let f = lua.load("collectgarbage()", "=b").unwrap();
        let _quiet = super::common::measuring();
        reads.lock().unwrap().clear();
        let t0 = remedy_lua_oracle::thread_cpu();
        lua.call(f.value(), &[]).unwrap();
        let mut r = reads.lock().unwrap().clone();
        r.insert(0, t0);
        r.push(remedy_lua_oracle::thread_cpu());
        let gap = r.windows(2).map(|w| w[1].saturating_sub(w[0])).max().unwrap_or(0);
        assert!(gap < 5_000_000, "fast {fast}, seed {seed}: {} us between two reads of the deadline", gap / 1000);
        fuel.push((1u64 << 40) - lua.fuel_left());
    }
    assert!(fuel.iter().all(|f| *f == fuel[0]), "fuel differs: {fuel:?}");
}

/// COV-5, the chunked half: a table past `TABLE_CHUNK` slots traversed a chunk a step
/// (`Heap::part_step`) was charged by its size when reached, its chunks and the rebuilds
/// that started them over, and when it is reached follows the order objects are reached in
/// (the seed). It is charged by its slots when the cycle started, each once, as
/// `traverse_table` charges a table. A big table reached behind 3,000 small ones (each
/// seed orders them its own way), grown and rebuilt while it is marked: the same fuel under
/// every seed and on both paths.
#[test]
fn cov5_a_big_table_reached_late_costs_its_snapshot() {
    let src = "local roots = {} for i = 1, 3000 do roots['r' .. i] = {i, {i}, {i}} end \
               local big = {} for i = 1, 20000 do big[i] = {i} end roots.big = big ROOTS = roots \
               collectgarbage('collect') collectgarbage('incremental', 100, 1) local s = 0 \
               for j = 1, 4000 do for i = 1, 8 do big[#big + 1] = {j} end \
                 if j % 200 == 0 then big['s' .. j] = j end local g = {j} s = s + #g end return s";
    let mut fuel = Vec::new();
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (true, 0xdead_beef), (true, 12345), (false, 99)] {
        let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(src, "=t").unwrap();
        fuel.push((1u64 << 40) - lua.fuel_left());
    }
    assert!(fuel.iter().all(|&f| f == fuel[0]), "fuel followed the seed or the path: {fuel:?}");
}
