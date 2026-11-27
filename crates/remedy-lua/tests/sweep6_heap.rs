//! Sweep 6, heap and latency findings (S6GC-H2, S6GC-M1, S6GC-M2,
//! S6-4, S6-5). Each test is named `s6_<id>_<what>` and runs with the fast loop on and
//! off where the loop matters. Resident memory is measured in a child process of this
//! test binary (`s6_heap_child`), so other tests' allocations do not count.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;
use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};
use std::time::{Duration, Instant};

fn state(limits: Limits, fast: bool) -> Lua {
    let mut lua = Lua::new(limits, Libs::ALL);
    lua.set_fast_loop(fast);
    lua
}

fn size(lua: &Lua, key: &str) -> usize {
    lua.internal_sizes().into_iter().find(|(k, _)| *k == key).unwrap_or_else(|| panic!("no size {key}")).1
}

/// A line of /proc/self/status, in bytes.
fn status_bytes(key: &str) -> usize {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0)
        * 1024
}

/// Runs `case` of `child_case` in a child process: (peak resident bytes past the child's
/// baseline, the case's own report).
fn in_child(case: &str) -> (usize, String) {
    let _heavy = common::memory_heavy();
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args(["sweep6_heap::s6_heap_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env("S6HEAP_CHILD", case)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    // (The test harness may print its own text before ours on a line.)
    let field =
        |k: &str| text.lines().find_map(|l| l.split_once(k).map(|(_, v)| v)).unwrap_or_default().to_owned();
    let peak: usize = field("S6HEAP_PEAK=")
        .parse()
        .unwrap_or_else(|_| panic!("child {case} failed: {text}\n{}", String::from_utf8_lossy(&out.stderr)));
    (peak, field("S6HEAP_RESULT="))
}

/// The cases measured in a child: what each returns.
fn child_case(case: &str) -> String {
    match case {
        // S6GC-H2: coroutines parked until memory runs out.
        "parked" => {
            let mut lua = state(Limits::default().memory(32 << 20).fuel(1 << 40), true);
            let r = lua.exec(
                "local t = {} local f = function() coroutine.yield() end local n = 0 \
                 while true do local c = coroutine.create(f) coroutine.resume(c) n = n + 1 t[n] = c end",
                "=t",
            );
            format!("{r:?}")
        }
        // S6GC-M1: a table of 2^23 slots holding one shared table, collected, dropped, then
        // a new 2^23 array.
        "scratch" => {
            let mut lua = state(Limits::default().memory(160 << 20).fuel(1 << 40), true);
            let r = lua.exec(
                "local N = 1 << 23 local s = {} local t = {} for i = 1, N do t[i] = s end \
                 collectgarbage() t = nil collectgarbage() \
                 local u = {} for i = 1, N do u[i] = i end collectgarbage() return #u",
                "=t",
            );
            format!("{:?} {}", r.map(|v| v.len()), lua.memory_used())
        }
        other => format!("unknown case {other}"),
    }
}

#[test]
#[ignore = "helper: run by the other tests in a child process"]
fn s6_heap_child() {
    let Ok(case) = std::env::var("S6HEAP_CHILD") else { return };
    let base = status_bytes("VmRSS");
    let r = child_case(&case);
    println!("S6HEAP_RESULT={r}");
    println!("S6HEAP_PEAK={}", status_bytes("VmHWM").saturating_sub(base));
}

/// S6GC-H2 (high): `shrink_parked` cut a suspended coroutine's stack with `shrink_to`,
/// which the allocator does in place, leaving the rest of the 4.2 KB register window as a
/// hole too small for the next coroutine's window: coroutines parked until the memory
/// limit held 316 MB resident at a 64 MiB cap (1,255 MB at 256 MiB, about 5x). The stack
/// is now copied into an allocation of its own size and the old one freed whole: 82 MB at
/// 64 MiB and 315 MB at 256 MiB (1.2 to 1.3x, the slab and table overhead). Here at a
/// 32 MiB cap: peak resident memory under 1.6x the cap (5x before).
#[test]
#[cfg(target_os = "linux")]
fn s6_gch2_parked_coroutines_stay_near_the_cap() {
    let (peak, r) = in_child("parked");
    assert!(r.contains("Memory"), "{r}");
    let ratio = peak as f64 / (32 << 20) as f64;
    assert!(ratio < 1.6, "parked coroutines: resident {ratio:.2}x the 32 MiB cap");
}

/// S6GC-M1 (medium): the collector's scratch buffers (`trav_vals`, `trav_ups`, `gray`,
/// `eph_links`, `eph_waiting`) are not charged and kept their peak capacity: traversing a
/// table of 2^23 slots that all hold one table filled `trav_vals` with 128 MiB of copies,
/// kept after the cycle, so a later 2^23 array took 275 MB resident against 128 MB
/// accounted under a 160 MiB cap. A table is now traversed a chunk of 4,096 slots at a
/// time, and buffers past 64 KiB are given back when a cycle ends: the scratch capacity
/// is at most 64 KiB a buffer after the cycle (16 MiB here before), and the case runs at
/// 132 MB resident.
#[test]
fn s6_gcm1_collector_scratch_is_given_back() {
    let _heavy = common::memory_heavy();
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(160 << 20).fuel(1 << 40), fast);
        // A big table of one shared table, an ephemeron table of many entries, a long
        // gray list (many small tables), all collected.
        lua.exec(
            "local s = {} local t = {} for i = 1, 1 << 20 do t[i] = s end \
             local e = setmetatable({}, {__mode = 'k'}) local keep = {} \
             for i = 1, 50000 do local k = {} keep[i] = k e[k] = {} end \
             local g = {} for i = 1, 100000 do g[i] = {} end \
             collectgarbage() collectgarbage()",
            "=t",
        )
        .unwrap();
        lua.collect_garbage().unwrap();
        let scratch = size(&lua, "gc_scratch_bytes");
        // Each of its seven buffers keeps at most 64 KiB.
        assert!(scratch <= 7 * (64 << 10), "fast {fast}: the collector keeps {scratch} bytes of scratch");
    }
}

/// The resident side of S6GC-M1 (275 MB before, 132 MB after, for 128 MiB accounted).
#[test]
#[cfg(target_os = "linux")]
fn s6_gcm1_resident_scratch_stays_small() {
    let (peak, r) = in_child("scratch");
    assert!(r.starts_with("Ok(1)"), "{r}");
    let used: usize = r.split_whitespace().nth(1).unwrap().parse().unwrap();
    let ratio = peak as f64 / used as f64;
    assert!(ratio < 1.4, "resident {ratio:.2}x the account ({peak} for {used})");
}

/// S6GC-M2 (medium): a suspended or dead coroutine's whole stack was marked, stale slots
/// above its live part too (only the running thread had its dead slots cleared), so
/// garbage a returned call left there stayed alive, unlike C Lua: a weak table kept it,
/// and six parked coroutines each with a dead 16 MiB string ran out of a 64 MiB cap where
/// C Lua succeeds. Now only the live part is marked (as C Lua's `traversethread`) and the
/// atomic step clears the rest: the weak entry goes (as in C Lua), the six fit.
#[test]
fn s6_gcm2_stale_slots_of_parked_coroutines_are_garbage() {
    let weak = "local w = setmetatable({}, {__mode='v'}) \
                local function g2(k) local s = setmetatable({}, nil) w[k] = s return 1 end \
                local co = coroutine.create(function() g2(2) coroutine.yield() end) \
                coroutine.resume(co) for i = 1, 10 do collectgarbage() end \
                local dead = coroutine.create(function() g2(3) end) coroutine.resume(dead) \
                for i = 1, 10 do collectgarbage() end \
                return w[2], w[3], coroutine.status(co), coroutine.status(dead)";
    assert_eq!(common::remedy(weak, 10_000_000), common::oracle(weak, 10_000_000));
    assert_eq!(common::remedy(weak, 10_000_000).unwrap(), "nil nil \"suspended\" \"dead\"");
    let six = "local function g(i) local s = string.rep(tostring(i), 16 << 20) return #s end \
               local cs = {} \
               for i = 1, 6 do \
                 local co = coroutine.create(function() g(i) coroutine.yield() return i end) \
                 assert(coroutine.resume(co)) cs[i] = co \
               end \
               local n = 0 for i = 1, 6 do local ok, v = coroutine.resume(cs[i]) n = n + v end return n";
    let _heavy = common::memory_heavy();
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(64 << 20).fuel(1 << 40), fast);
        let r = lua.exec(six, "=t").map(|v| common::show(&lua, v[0]));
        assert_eq!(r.unwrap(), "21", "fast {fast}");
    }
}

/// S6GC-M2's clearing must not touch what a parked coroutine still needs: its registers,
/// varargs, open upvalues, pending to-be-closed variables, a yield inside `pcall` or a
/// metamethod, the values a resume passes in. Each resume runs after collections under
/// the collector's stress mode (one at every allocation), on a 64 KiB stack.
#[test]
fn s6_gcm2_parked_coroutines_keep_their_live_slots() {
    let src = r#"
        local log = {}
        local function mk(seed)
          return coroutine.create(function(a, ...)
            local va = table.pack(...)
            local up = {seed}
            local function bump() up[1] = up[1] + 1 return up[1] end
            local c <close> = setmetatable({}, {__close = function() log[#log + 1] = 'closed' .. seed end})
            local t = setmetatable({}, {__index = function(_, k) return coroutine.yield('idx' .. k) end})
            local r = {}
            for i = 1, 3 do
              local ok, v = pcall(function(x) return coroutine.yield(x, bump()) end, a + i)
              local x = va[1 + (i % va.n)]
              if type(x) == 'table' then x = 't' .. x[1] end
              r[#r + 1] = tostring(v) .. ':' .. x
              collectgarbage()
            end
            r[#r + 1] = t.key
            r[#r + 1] = select('#', ...) .. bump()
            return table.concat(r, ',')
          end)
        end
        local out = {}
        for s = 1, 4 do
          local co = mk(s * 10)
          local vals = {coroutine.resume(co, s, 'x' .. s, {s}, 'z' .. s)}
          local n = 0
          while coroutine.status(co) == 'suspended' do
            for i = 1, 3 do collectgarbage() end
            local junk = {} for i = 1, 50 do junk[i] = {i} end
            n = n + 1
            vals = {coroutine.resume(co, 'r' .. n .. '-' .. s)}
          end
          out[#out + 1] = tostring(vals[2])
        end
        -- One left suspended with a pending close, closed after collections.
        local co = mk(99)
        coroutine.resume(co, 1, 'a')
        for i = 1, 3 do collectgarbage() end
        out[#out + 1] = tostring(coroutine.close(co))
        return table.concat(out, ' | ') .. ' ' .. table.concat(log, ',')
    "#;
    let theirs = common::oracle(src, 100_000_000);
    assert!(theirs.is_ok(), "{theirs:?}");
    for fast in [true, false] {
        let src = src.to_owned();
        let ours = common::small_stack(move || {
            let mut lua = Lua::new(
                Limits::default().memory(256 << 20).fuel(1 << 34).hash_seed(common::TEST_SEED),
                Libs::ALL,
            );
            lua.set_fast_loop(fast);
            lua.set_gc_stress(true);
            match lua.exec(common::wrap(&src), "=wrapper") {
                Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
                Err(e) => Err(e.to_string()),
            }
        });
        assert_eq!(ours, theirs, "fast {fast}");
    }
}

/// S6-4 (high): `host_rets` kept, for each running host function, uncharged copies of the
/// results of its last `Lua::call`. A host function `h(a, b)` that calls `a()` (dropping
/// the results) and then `b(a, b)`, through which the script recurses with `a` returning
/// `table.unpack` of 100,000 copies of one table, held 74 levels of 100,000 copies: 128 MB
/// resident against 5.3 MB accounted under an 8 MiB cap. A level's part now keeps each
/// object once past 256 results: at most a few dozen entries here (6 million before, at
/// 60 levels).
#[test]
fn s6_4_host_results_are_kept_once_per_object() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    for fast in [true, false] {
        let most = Arc::new(AtomicUsize::new(0));
        let mut lua = state(Limits::default().memory(8 << 20), fast);
        let m = Arc::clone(&most);
        lua.register("h", move |lua, args| {
            let (a, b) = (args[0], args[1]);
            let first = lua.call(a, &[])?;
            assert_eq!(first.len(), 100_000);
            drop(first);
            m.fetch_max(size(lua, "host_rets"), Ordering::Relaxed);
            lua.call(b, &[a, b])
        })
        .unwrap();
        let r = lua
            .exec(
                "local one = {} T = {} for i = 1, 100000 do T[i] = one end \
                 local depth = 0 \
                 local function b(a, b2) depth = depth + 1 if depth < 60 then return h(a, b2) end return depth end \
                 return h(function() return table.unpack(T) end, b)",
                "=t",
            )
            .unwrap();
        assert_eq!(r, vec![Value::Int(60)]);
        let most = most.load(Ordering::Relaxed);
        assert!(most < 60 * 300, "fast {fast}: host_rets held {most} values");
        assert_eq!(size(&lua, "host_rets"), 0);
    }
}

/// How long past a 20 ms time limit, and past an interrupt sent at 20 ms, `src` stops:
/// the shortest of three runs each (the machine is shared; a fix shows in every run).
fn overrun(src: &str) -> (Duration, Duration) {
    let _heavy = common::memory_heavy();
    let limit = Duration::from_millis(20);
    let mut best = (Duration::MAX, Duration::MAX);
    for _ in 0..3 {
        let mut lua = state(Limits::unmetered().memory(1 << 30), true);
        let start = Instant::now();
        let clock: remedy_lua::Clock = std::sync::Arc::new(move || start.elapsed());
        lua.set_time_limit(Some((clock, limit)));
        let r = lua.exec(src, "=t");
        best.0 = best.0.min(start.elapsed().saturating_sub(limit));
        assert!(matches!(r, Err(Error::Limit(LimitKind::Time))), "{src}: {r:?}");
        lua.set_time_limit(None);
        let h = lua.interrupt_handle();
        let t = std::thread::spawn(move || {
            std::thread::sleep(limit);
            h.interrupt();
            Instant::now()
        });
        let r = lua.exec(src, "=t");
        let end = Instant::now();
        let at = t.join().unwrap();
        assert!(matches!(r, Err(Error::Limit(LimitKind::Interrupted))), "{src}: {r:?}");
        best.1 = best.1.min(end.saturating_duration_since(at));
    }
    best
}

/// S6-5 (medium): single natives that do much work per charge stopped late (20 ms
/// limit): `string.rep('a', 16e6)` 160 to 256 ms past it (a byte-wise copy after one
/// charge), `table.unpack({}, 1, 3e6)` 170 to 350 ms (the deadline is read only when a
/// slice of fuel runs out, which one big charge reaches once), `string.byte(s, 1, -1)` of
/// 3M bytes 54 to 74 ms (here 12M: some 200 ms). `rep` now doubles what it wrote (memory
/// speed) and looks at the interrupt and deadline every 4 MB; `unpack`, `byte` and the
/// placing of many results look at them every 65,536 values: each stops within a few
/// milliseconds (bound 30 ms).
#[test]
fn s6_5_big_natives_stop_at_the_time_limit() {
    let mut late = Vec::new();
    for src in [
        "while true do string.rep('a', 16e6) end",
        "while true do string.rep('ab', 100e6, ',') end",
        "while true do table.unpack({}, 1, 12e6) end",
        "local s = string.rep('a', 12e6) while true do string.byte(s, 1, -1) end",
        "local s = string.rep('a', 3e6) while true do local t = {string.byte(s, 1, -1)} end",
        // S6-5b (the long property run): 4M code points ran 61 to 75 ms past the limit.
        "local s = string.rep('aaab', 1048576) while true do utf8.codepoint(s, 1, -1) end",
    ] {
        let (time, interrupt) = overrun(src);
        let bound = Duration::from_millis(30);
        if time >= bound || interrupt >= bound {
            late.push(format!(
                "{src}: stopped {time:?} past the time limit, {interrupt:?} past the interrupt"
            ));
        }
    }
    assert!(late.is_empty(), "{late:#?}");
}

/// `rep`'s doubling builds what the piece-by-piece copy did, separators included, and the
/// same fuel with the fast loop on and off.
#[test]
fn s6_5_rep_builds_the_same_strings() {
    let src = "local out = {} \
               for _, c in ipairs({{'', 0, nil}, {'a', 1, nil}, {'ab', 3, ','}, {'', 5, '-+'}, {'xyz', 1000, ''}, \
                                   {'q', 70000, nil}, {'ab', 3e6, ', '}, {'', 4e6, 'z'}, {'abc', 5, ''}}) do \
                 local s = string.rep(c[1], c[2], c[3]) \
                 local h = 0 for i = 1, #s, 997 do h = (h * 31 + s:byte(i)) % 1000000007 end \
                 out[#out + 1] = #s .. ':' .. h .. ':' .. s:sub(1, 12) .. ':' .. s:sub(-12) \
               end \
               return table.concat(out, ' ')";
    assert_eq!(common::remedy(src, 100_000_000), common::oracle(src, 100_000_000));
}
