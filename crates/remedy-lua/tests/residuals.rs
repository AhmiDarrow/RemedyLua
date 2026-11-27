//! The known residuals of sweep 5, each closed or bounded: regression tests named
//! `res_<item>_<what>`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Libs, Limits, Lua};
use std::sync::{Arc, Mutex};

fn state(limits: Limits, fast: bool) -> Lua {
    let mut lua = Lua::new(limits, Libs::ALL);
    lua.set_fast_loop(fast);
    lua
}

/// Probe accounting per state: the counters of uncharged probe steps are per thread, and
/// only the entries that go through `enter` kept them apart (S6-RES-4). A host function of
/// one state that compiled source in another (`Lua::load`, which does not enter the state)
/// left that compile's interner walks in the counter, and the outer script's next
/// instruction paid for them. Now every host function runs with the counters of the state
/// that called it set aside, so whatever it does to any state, that state's script pays
/// exactly its own steps.
#[test]
fn res_probes_a_host_function_cannot_bill_its_caller() {
    let seed = super::common::TEST_SEED;
    // Source whose strings all share one hash under `seed` (S5GC-3's colliding strings).
    let mut maker = state(Limits::default().fuel(1 << 40).memory(1 << 30).hash_seed(seed), true);
    let setup = super::sweep5_gc::colliding_strings(seed, 3000);
    let made = maker
        .exec(format!("{setup} return 'return {{' .. string.format(string.rep('%q,', #keep), table.unpack(keep)) .. '}}'"), "=mk")
        .unwrap();
    let src = maker.str_bytes(made[0]).unwrap().to_vec();
    let plain: Vec<u8> =
        format!("return {{{}}}", (0..3000).map(|i| format!("'p{i:020}',")).collect::<String>()).into_bytes();
    let outer_fuel = |source: Vec<u8>, fast: bool| -> u64 {
        let inner = Arc::new(Mutex::new(state(
            Limits::default().fuel(1 << 40).memory(1 << 30).hash_seed(seed),
            fast,
        )));
        let mut outer = state(Limits::default().fuel(100_000_000), fast);
        outer
            .register("compile", move |_, _| {
                let mut l = inner.lock().unwrap();
                l.load(&source, "=inner").unwrap();
                Ok(vec![])
            })
            .unwrap();
        outer.exec("local t = {} for i = 1, 3 do compile() t[i] = i end", "=outer").unwrap();
        100_000_000 - outer.fuel_left()
    };
    for fast in [true, false] {
        let (p, c) = (outer_fuel(plain.clone(), fast), outer_fuel(src.clone(), fast));
        assert_eq!(c, p, "fast {fast}: the outer script paid for another state's compile");
    }
}

/// Hostile sources of about `bytes` bytes, by name: the shapes whose front-end structures
/// grow biggest (tokens, statements, locals and the scope map, constants, functions).
pub(super) fn hostile_sources(bytes: usize) -> Vec<(&'static str, String)> {
    let fill = |line: &dyn Fn(usize) -> String| {
        let mut s = String::with_capacity(bytes + 100);
        let mut i = 0;
        while s.len() < bytes {
            s.push_str(&line(i));
            i += 1;
        }
        s
    };
    vec![
        (
            "locals",
            fill(&|j| {
                format!(
                    "local {}\n",
                    (0..100).map(|i| format!("v{}", j * 100 + i)).collect::<Vec<_>>().join(",")
                )
            }),
        ),
        ("one_local_per_line", fill(&|j| format!("local v{j} = {j}\n"))),
        ("tokens", format!("x = 1{}", "+1".repeat(bytes / 2))),
        ("strings", format!("local t = {{{}}}", fill(&|j| format!("'s{j}',")))),
        ("globals", fill(&|j| format!("g{j} = {j}\n"))),
        ("functions", fill(&|j| format!("function f{j}() return {j} end\n"))),
        ("numbers", format!("local t = {{{}}}", fill(&|j| format!("{}.5,", j)))),
        ("fields", format!("local t = {{{}}}", fill(&|j| format!("k{j}=1,")))),
    ]
}

/// The longest stretch between two looks at the clock while `src` compiles inside a call
/// (a load in a call looks at the deadline as it looks at the interrupt), from the call's
/// start to its end; also the whole time.
pub(super) fn longest_blind_stretch(src: &str) -> (std::time::Duration, std::time::Duration, String) {
    use std::time::{Duration, Instant};
    let mut lua = Lua::new(Limits::unmetered().memory(8 << 30), Libs::ALL);
    let s = lua.create_string(src).unwrap();
    lua.set_global("SRC", s).unwrap();
    let f = lua.load("local f = load(SRC) return f ~= nil", "=probe").unwrap();
    let rec: Arc<Mutex<(Option<Instant>, Duration, String)>> =
        Arc::new(Mutex::new((None, Duration::ZERO, String::new())));
    let r2 = Arc::clone(&rec);
    let origin = Instant::now();
    let want_bt = std::env::var("RES_BACKTRACE").is_ok();
    let prev: Arc<Mutex<Option<std::backtrace::Backtrace>>> = Arc::new(Mutex::new(None));
    let p2 = Arc::clone(&prev);
    lua.set_time_limit(Some((
        Arc::new(move || {
            let cur = want_bt.then(std::backtrace::Backtrace::force_capture);
            let now = Instant::now();
            let mut g = r2.lock().unwrap();
            if let Some(last) = g.0 {
                let gap = now - last;
                if gap > g.1 {
                    g.1 = gap;
                    if let (Some(c), Some(p)) = (&cur, p2.lock().unwrap().as_ref()) {
                        g.2 = format!("{p}\n----- to -----\n{c}");
                    }
                }
            }
            *p2.lock().unwrap() = cur;
            g.0 = Some(Instant::now());
            now - origin
        }),
        Duration::from_secs(3600),
    )));
    let t = Instant::now();
    rec.lock().unwrap().0 = Some(t);
    let ok = lua.call(f.value(), &[]).unwrap();
    let end = Instant::now();
    assert_eq!(ok, [remedy_lua::Value::Bool(true)]);
    let g = rec.lock().unwrap();
    let tail = end - g.0.unwrap_or(t);
    let last =
        prev.lock().unwrap().as_ref().map(|b| format!("after the last look, at:\n{b}")).unwrap_or_default();
    (g.1.max(tail), end - t, if tail > g.1 { last } else { g.2.clone() })
}

/// Interrupt latency of the front end: a load stops at its next look at the host's
/// interrupt or deadline. After S6-FRONT-4 (host loads), a load inside a call still had
/// single bulk operations between looks: the tokens, the tree and the active locals freed
/// at once, the token vector and the compiler's lists of locals and constants grown in one
/// `realloc`, the scope map rehashed whole, `grow_watched`'s old buffer unmapped whole,
/// `load`'s copy and scan of the source, 4,096 compiler steps (3 to 4 ms) per look. At 2 MB
/// in this test's build: 11 to 52 ms without a look (26 to 114 ms at 6 MB in release). Now
/// 1.3 to 8 ms here, 1.6 to 8.5 ms at 6 MB in release, except the string interner's
/// regrow (15 to 17 ms at 600,000 strings).
#[test]
fn res_front_no_long_stretch_without_a_look() {
    let _quiet = super::common::measuring();
    let mut bad = Vec::new();
    for (name, src) in hostile_sources(2 << 20) {
        let best = (0..3).map(|_| longest_blind_stretch(&src).0).min().unwrap_or_default();
        println!("{name}: {best:?}");
        if best > std::time::Duration::from_millis(12) {
            bad.push(format!("{name}: {best:?} without a look"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// Probe (run in release): the longest blind stretch of each hostile shape at 6 MB.
#[test]
#[ignore = "probe: prints the front end's longest blind stretches"]
fn res_front_probe_blind_stretches() {
    let mb: usize = std::env::var("RES_MB").ok().and_then(|s| s.parse().ok()).unwrap_or(6);
    for (name, src) in hostile_sources(mb << 20) {
        let (gap, total, bt) = longest_blind_stretch(&src);
        println!("{name:>20}: longest stretch {gap:>10.2?} of {total:>10.2?}");
        if std::env::var("RES_BACKTRACE").is_ok() {
            if std::env::var("RES_RAW").is_ok() {
                println!("{bt}");
                continue;
            }
            let keep: Vec<&str> = bt
                .lines()
                .filter(|l| {
                    (l.contains("remedy_lua::") || l.contains("-----"))
                        && !l.contains("residuals")
                        && !l.contains("Watch")
                        && !l.contains("compile_tick")
                        && !l.contains("LocalKey")
                })
                .filter(|l| {
                    !l.contains("vm::Lua>::call")
                        && !l.contains("vm::Lua>::enter")
                        && !l.contains("execute")
                        && !l.contains("vm::Lua>::run")
                        && !l.contains("base::load")
                })
                .collect();
            println!("{}", keep.join("\n"));
        }
    }
}

/// The S5B-OPEN-2 worst layout (`sweep5c_open2::probe_far`: every node of both chains a
/// cache line or more from the last) in 1,999 tables of 2^9 slots: the setup for a key
/// (`k` is the aimed key or a plain absent one), the tables either reached through one
/// `__index` chain (`U`) or kept in an array (`T`).
pub(super) fn far_layout(key_aimed: bool) -> (String, f64) {
    use super::sweep5b_open::{PAIR_FULL, aimed_floats, homes, list, table_seeds};
    let bits = 9u32;
    let ss = table_seeds(super::common::TEST_SEED, 1);
    let mask = (1u64 << bits) - 1;
    let n = 1usize << bits;
    let plain = n * 3 / 10;
    let mut used = vec![false; n];
    for i in 1..=plain {
        let (x, y) = homes(i as f64 + 0.25, ss[0], bits);
        used[x as usize] = true;
        used[y as usize] = true;
    }
    let clear = |h: u64| !used[h as usize] && !used[(h ^ mask) as usize];
    let mut pairs: Vec<u64> = Vec::new();
    let mut h = 0u64;
    while pairs.len() < 4 {
        if h + 9 < n as u64 / 2 && clear(h) && pairs.iter().all(|&p| p.abs_diff(h) > 20) {
            pairs.push(h);
        }
        h += 1;
    }
    let mut fill = Vec::new();
    for &p in &pairs {
        for d in 1..=8u64 {
            fill.extend(aimed_floats(&ss, bits, (p + d) & mask, 1));
            fill.extend(aimed_floats(&ss, bits, ((p ^ mask) + d) & mask, 1));
        }
    }
    let keys = PAIR_FULL;
    let per: Vec<Vec<f64>> = pairs.iter().map(|&p| aimed_floats(&ss, bits, p, keys + 1)).collect();
    let k = if key_aimed { per[0][keys] } else { -0.375 };
    let mut late = Vec::new();
    for r in 0..keys {
        for c in &per {
            late.push(c[r]);
        }
    }
    let setup = format!(
        "local F, L = {{{}}}, {{{}}} T = {{}} local last = {{}} \
         for i = 1, 1999 do local x = {{}} for i = 1, {plain} do x[i + 0.25] = i end \
         for _, f in ipairs(F) do x[f] = true end for _, f in ipairs(L) do x[f] = true end \
         T[i] = x setmetatable(x, {{__index = last}}) last = x end U = last \
         k = {k:?} collectgarbage() collectgarbage('stop')",
        list(&fill),
        list(&late)
    );
    (setup, k)
}

/// S5B-OPEN-2's residual: with a known hash seed, keys aimed at the worst layout (every
/// node of both chains a cache line or more from the last, `far_layout`) in 1,999 tables
/// of 2^9 slots reached through one `__index` chain ran at 8.3x plain code per unit of fuel
/// with the fast loop and 6.5x without here (up to 13x and 7x in sweep 5c, 9x on slower
/// memory at 2 units a link): memory latency is not fuel, and a link was 1 unit for about
/// two instructions' work and another table's loads, a cache miss or more each when the
/// chain is long. Now each link is 2 units, and each past the 4th of one walk 8
/// (`vm::link_cost`): about 1.5x here. Plain keys in the same tables: well under 1x.
/// Measured best of 5, plain code just before the chain, each outside memory-heavy tests
/// (`ns_per_unit` holds `common::measuring()`; held here too, a heavy test waiting between
/// the two reads deadlocked the suite).
#[test]
fn res_open2_worst_layout_through_a_chain_is_bounded() {
    use super::sweep5b_open::{baseline, ns_per_unit};
    let (setup, _) = far_layout(true);
    for (fast, bound) in [(true, 7.5), (false, 5.5)] {
        let mut r = f64::MAX;
        for _ in 0..5 {
            let base = baseline(fast);
            r = r.min(ns_per_unit(&setup, "local x = U[k]", 3_000_000, fast) / base);
        }
        println!("fast {fast}: {r:.1}x plain code");
        assert!(r < bound, "fast {fast}: aimed keys through a chain at {r:.1}x plain code per unit of fuel");
    }
}

/// Probe: the worst layout's cost per unit of fuel against plain code, through an `__index`
/// chain and by direct lookups, aimed and plain keys, fast loop and full path.
#[test]
#[ignore = "probe: prints S5B-OPEN-2's worst-layout ratios"]
fn res_open2_probe_far() {
    use super::sweep5b_open::{baseline, ns_per_unit};
    for fast in [true, false] {
        for aimed in [true, false] {
            let (setup, _) = far_layout(aimed);
            for (what, body) in
                [("chain", "local x = U[k]"), ("direct", "for j = 1, 1999 do local x = T[j][k] end")]
            {
                let r = (0..3)
                    .map(|_| ns_per_unit(&setup, body, 3_000_000, fast) / baseline(fast))
                    .fold(f64::MAX, f64::min);
                println!("fast {fast} aimed {aimed} {what}: {r:.1}x plain code");
            }
        }
    }
}

/// The memory a compile charges is the program's: the scope map and the constant maps are
/// sharded past 16,384 keys (`shardmap`), and the shards' own sizes follow a random hasher
/// (as do a map's tombstones after removals), so the charge is by the number of keys, never
/// the tables' `capacity()`. A load with 40,000 locals and 40,000 string constants, stopped
/// at many points by the fuel limit inside a call (what it built is then deferred and
/// charged): the same fuel and memory on every run, under any seed, on both paths.
#[test]
fn res_front_compile_memory_is_the_programs() {
    let locals: String = (0..400)
        .map(|j| {
            format!("local {}\n", (0..100).map(|i| format!("v{}", j * 100 + i)).collect::<Vec<_>>().join(","))
        })
        .collect();
    let consts = format!("local t = {{{}}}", (0..40_000).map(|i| format!("'c{i}',")).collect::<String>());
    // Names that leave scope (tombstones in the scope map), then a long tail to stop in.
    let scopes: String = (0..3000)
        .map(|j| {
            format!("do local {} end\n", (0..12).map(|i| format!("s{j}_{i}")).collect::<Vec<_>>().join(","))
        })
        .collect::<String>()
        + &(0..20_000).map(|i| format!("local w{i} = {i}\n")).collect::<String>();
    for src in [locals, consts, scopes] {
        let measure = |seed: u64, fast: bool, fuel: u64| {
            let mut lua = state(Limits::default().fuel(1 << 40).memory(1 << 30).hash_seed(seed), fast);
            let s = lua.create_string(src.as_str()).unwrap();
            lua.set_global("SRC", s).unwrap();
            lua.set_limits(lua.limits().fuel(fuel));
            let r = lua.exec("local f = load(SRC) return f ~= nil", "=t").map_err(|e| e.to_string());
            (r, lua.fuel_left(), lua.memory_used())
        };
        for fuel in [40_000, 150_000, 400_000, 1_000_000, 4_000_000, 1 << 30] {
            let first = measure(1, true, fuel);
            for (seed, fast) in [(1, true), (1, false), (2, true), (0xdead_beef, false), (1, true)] {
                assert_eq!(measure(seed, fast, fuel), first, "seed {seed} fast {fast} fuel {fuel}");
            }
        }
    }
}

/// How many times one call of `body` (after `setup`) reads the time limit's clock (as
/// `sec_metering::clock_reads`): looks at the deadline, counted, so the test does not
/// depend on the machine's speed.
fn clock_reads(setup: &str, body: &str) -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), true);
    lua.exec(setup, "=setup").unwrap();
    let f = lua.load(body, "=body").unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let r2 = Arc::clone(&reads);
    let clock: remedy_lua::Clock = Arc::new(move || {
        r2.fetch_add(1, Ordering::Relaxed);
        std::time::Duration::ZERO
    });
    lua.set_time_limit(Some((clock, std::time::Duration::from_secs(3600))));
    lua.call(f.value(), &[]).unwrap();
    reads.load(Ordering::Relaxed)
}

/// Bulk natives over a 16 MiB string copied or scanned it in one go between looks at the
/// host's interrupt and deadline: `string.pack('s')` ran 21 ms past a 1 ms time limit
/// (buffer doublings, then a copy into the string), `rep(s, 1)` 9 ms (the first piece in
/// one copy), `format('%s')` 8 ms, plain `find` 8 ms (one scan for the first byte),
/// `unpack('s')` 7 ms, `load`'s copy and scan of its source several ms. Each looks every
/// megabyte now (`Lua::extend_watched`, `new_copied`, `COPY_PIECE`): 0.2 to 0.6 ms past
/// the limit. Checked by the looks, and the results are the same strings.
#[test]
fn res_bulk_natives_look_at_the_deadline_every_megabyte() {
    // Floods memory bandwidth: never beside a measurement.
    let _heavy = super::common::memory_heavy();
    let setup = "S = ('x'):rep((16 << 20) - 64) P = string.pack('s', S) SRC = ('local a = 1\\n'):rep(300000)";
    let base = clock_reads(setup, "local n = #S");
    for body in [
        "local r = string.pack('s', S)",
        "local r = string.pack('z', S)",
        "local r = string.pack('c' .. #S, S)",
        "local r = string.rep(S, 1)",
        "local r = string.rep(S, 1, 'sep')",
        "local r = string.unpack('s', P)",
        "local r = string.format('%s', S)",
        "local r = S:find('z', 1, true)",
        "local f = load(SRC)",
    ] {
        let reads = clock_reads(setup, body);
        assert!(reads >= base + 8, "{body}: the clock was read {reads} times ({base} for #S)");
    }
    let mut lua = state(Limits::default().memory(512 << 20).fuel(1 << 40), true);
    let r = lua
        .exec(
            "local s = {} for i = 1, 300000 do s[i] = string.char(65 + i % 26, 97 + i % 7, 48 + i % 10) .. i end s = table.concat(s) \
             assert(#s > (2 << 20)) \
             assert(string.unpack('s', string.pack('s', s)) == s) \
             assert(string.unpack('z', string.pack('z', s)) == s) \
             assert(string.pack('c' .. #s, s) == s) \
             assert(string.rep(s, 1) == s and string.rep(s, 2, '|') == s .. '|' .. s and #string.rep(s, 3) == 3 * #s) \
             assert(string.format('%s', s) == s and string.format('<%s>', s) == '<' .. s .. '>') \
             assert(s:find(s:sub(-20), 1, true) == #s - 19 and s:find('#', 1, true) == nil) \
             assert(s:find(s:sub(2000001, 2000010), 1, true) == 2000001) \
             return #s",
            "=t",
        )
        .unwrap();
    assert!(matches!(r.as_slice(), [remedy_lua::Value::Int(n)] if *n > 2 << 20), "{r:?}");
    // `rep` builds a long result by copying what it has written so far, capped per step:
    // the copy must start at a whole number of units (a result over 16 MiB came out wrong
    // on the base, every result over 1 MiB with the first piece copied in pieces).
    let mut lua = state(Limits::default().memory(1 << 30).string_len(64 << 20).fuel(1 << 40), true);
    let check = "local function check(s, n, sep) local r = string.rep(s, n, sep) local u = s .. (sep or '')                  if #r ~= #s * n + #(sep or '') * (n - 1) then return false end                  for i = 1, #r, 99991 do if r:byte(i) ~= u:byte((i - 1) % #u + 1) then return false end end return true end                  local big = {} for i = 1, 200000 do big[i] = string.char(65 + i % 26) .. i end big = table.concat(big)                  return check('abc', 7000000), check('abcd', 3000000, ','), check(big, 4), check(big, 3, '|'), check('x', 20000000)";
    let r = lua.exec(check, "=rep").unwrap();
    assert_eq!(r, [remedy_lua::Value::Bool(true); 5], "string.rep built a wrong string");
}

/// The longest stretch between two looks at the clock while `body` runs (after `setup`),
/// in a call with a time limit (the clock is read at each look).
pub(super) fn longest_run_stretch(setup: &str, body: &str, fast: bool) -> std::time::Duration {
    use std::time::{Duration, Instant};
    let mut lua = state(Limits::default().memory(4 << 30).fuel(1 << 40), fast);
    lua.exec(setup, "=setup").unwrap();
    let f = lua.load(body, "=body").unwrap();
    let rec: Arc<Mutex<(Option<Instant>, Duration)>> = Arc::new(Mutex::new((None, Duration::ZERO)));
    let r2 = Arc::clone(&rec);
    let origin = Instant::now();
    lua.set_time_limit(Some((
        Arc::new(move || {
            let now = Instant::now();
            let mut g = r2.lock().unwrap();
            if let Some(last) = g.0 {
                g.1 = g.1.max(now - last);
            }
            g.0 = Some(now);
            now - origin
        }),
        Duration::from_secs(3600),
    )));
    rec.lock().unwrap().0 = Some(Instant::now());
    lua.call(f.value(), &[]).unwrap();
    let end = Instant::now();
    let g = rec.lock().unwrap();
    g.1.max(end - g.0.unwrap_or(end))
}

/// Probe: the longest stretch without a look while tables grow big.
#[test]
#[ignore = "probe: prints the longest stretches while big tables grow"]
fn res_table_growth_probe() {
    for (name, body) in [
        ("array 2^21", "local t = {} for i = 1, 1 << 21 do t[i] = i end"),
        ("array 2^22", "local t = {} for i = 1, 1 << 22 do t[i] = i end"),
        ("hash 2^20", "local t = {} for i = 1, 1 << 20 do t[i + 0.5] = i end"),
        ("ints 2^20", "local t = {} for i = 1, 1 << 20 do t[i * 3] = i end"),
        ("strings 2^19", "local t = {} for i = 1, 1 << 19 do t['k' .. i] = i end"),
        ("mixed 2^21", "local t = {x = 1} for i = 1, 1 << 21 do t[i] = i end"),
    ] {
        for fast in [true, false] {
            let d = (0..3).map(|_| longest_run_stretch("", body, fast)).min().unwrap_or_default();
            println!("{name:>14} fast {fast}: {d:?}");
        }
    }
}

/// Big tables, each one key short of a resize (`T`), and the write that resizes it (`K`):
/// the array part (2^21 slots, to 2^22), the integer part (2^20 keys), `nodes` (float keys)
/// and a table with a string key whose array part grows (the other branch of `resize`).
fn one_short_of_a_resize() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("array", "T = {} for i = 1, 1 << 21 do T[i] = i end", "T[(1 << 21) + 1] = 1"),
        ("ints", "T = {} for i = 1, 1 << 20 do T[i * 3] = i end", "T[-1] = 1"),
        ("nodes", "T = {} for i = 1, 3 << 19 do T[i + 0.5] = i end", "for i = 1, 4 do T[-i - 0.5] = i end"),
        ("mixed", "T = {x = 1} for i = 1, 1 << 21 do T[i] = i end", "T[(1 << 21) + 1] = 1"),
        // LR-23: moving a million-slot array up one grows its array part (63 ms late).
        ("move", "T = {} for i = 1, 1 << 20 do T[i] = i end", "table.move(T, 1, 1 << 20, 2)"),
    ]
}

/// A table resize looks at the host's interrupt and deadline between pieces of its work
/// (`Mem::stopped`, `table::RESIZE_PIECE`): growing a table of a million keys was one
/// operation of 15 to 64 ms with no look (the array part doubling at 2^21 slots 15 ms, a
/// hash part rebuilt 64 ms); now at most about 2 ms between looks. Counted by the clock's
/// reads, so the test does not depend on the machine.
#[test]
fn res_table_resizes_look_at_the_deadline() {
    let _heavy = super::common::memory_heavy();
    for (name, setup, write) in one_short_of_a_resize() {
        let base = clock_reads(setup, "local x = 1");
        let reads = clock_reads(setup, write);
        assert!(reads >= base + 16, "{name}: the clock was read {reads} times ({base} without the resize)");
    }
}

/// A table resize stopped by the deadline between pieces gives up: the table is exactly as
/// it was (the same entries in the same traversal order, the same border), the write raises
/// the time limit, and the same write afterwards gives the same table, memory and fuel as
/// in a twin state that was never stopped. Stopped at several points of each resize. (What
/// it made is kept for the next one, `Table::partial`, beside the old parts: memory in
/// between counts both, never more than the old parts and the finished resize together.)
///
/// SEC7-T3 (bound change): this bound was the finished resize's
/// alone, which only an undercount could meet: the kept state holds the old parts and the
/// new one. The ints case (anon RSS against `memory_used`, MB):
///
/// | stopped at read (of 321) | RSS | counted before SEC7-T3 | counted with it |
/// |---|---|---|---|
/// | (before the write) | 37 | 36 | 36 |
/// | 160 | 53 | 72 | 108 |
/// | 250 | 98 | 72 | 108 |
/// | 300, 318 | 102 | 72 | 108 |
/// | (finished) | 70 | 72 | 72 |
///
/// Stopped late, 30 MB were resident past the account under the old bound; the new part
/// is now counted whole while kept (allocated, so counted before its pages are written).
#[test]
fn res_table_stopped_resize_leaves_the_table_unchanged() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let _heavy = super::common::memory_heavy();
    let fresh = |setup: &str, fast: bool| {
        let mut lua =
            state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(super::common::TEST_SEED), fast);
        lua.exec(setup, "=setup").unwrap();
        lua
    };
    let snapshot = |lua: &mut Lua| {
        // (An entry first: it frees what a compile left (`Lua::load`'s, freed at the next
        // entry, as the stopped call's own entry does), so `memory_used` below counts the
        // same things in every state. `get_global` entered for its name until SEC8-F3; it
        // reads without entering now.)
        lua.create_string("T").unwrap();
        let t = match lua.get_global("T").unwrap() {
            remedy_lua::Value::Table(t) => t,
            v => panic!("{v:?}"),
        };
        // (Strings by their bytes: handles differ between states.)
        let show = |lua: &Lua, v: &remedy_lua::Value| match v {
            remedy_lua::Value::Str(_) => format!("{:?}", lua.str_bytes(*v)),
            v => format!("{v:?}"),
        };
        let pairs: Vec<String> =
            lua.table_pairs(t).iter().map(|(k, v)| format!("{}={}", show(lua, k), show(lua, v))).collect();
        (pairs, lua.table_len(t), lua.memory_used())
    };
    for (name, setup, write) in one_short_of_a_resize() {
        let total = clock_reads(setup, write);
        // (Found once, on the first path: the other's stop there is checked all the same.)
        let mut last = None;
        for fast in [true, false] {
            let mut twin = fresh(setup, fast);
            let f = twin.load(write, "=write").unwrap();
            let before = snapshot(&mut twin);
            twin.call(f.value(), &[]).unwrap();
            let fuel = (1u64 << 40) - twin.fuel_left();
            let after = snapshot(&mut twin);
            let stop_at = |at: usize| {
                let mut lua = fresh(setup, fast);
                let f = lua.load(write, "=write").unwrap();
                let reads = Arc::new(AtomicUsize::new(0));
                let r2 = Arc::clone(&reads);
                // The deadline passes at the `at`-th read of the clock.
                let clock: remedy_lua::Clock = Arc::new(move || {
                    let n = r2.fetch_add(1, Ordering::Relaxed);
                    if n + 1 >= at { std::time::Duration::from_secs(7200) } else { std::time::Duration::ZERO }
                });
                lua.set_time_limit(Some((clock, std::time::Duration::from_secs(3600))));
                let r = lua.call(f.value(), &[]);
                (lua, f, r)
            };
            // Also one piece before the end: the last read a stop at leaves the table as it
            // was (the reads after the resize's are the write's own).
            let inside = |at: usize| {
                let (mut lua, _, r) = stop_at(at);
                lua.set_time_limit(None);
                let now = snapshot(&mut lua);
                r.is_err() && (now.0.len(), now.1) == (before.0.len(), before.1)
            };
            // (Not for the nodes and move cases: they write before the resize.)
            let last = *last.get_or_insert_with(|| {
                let (mut lo, mut hi) = (2, total);
                while !matches!(name, "nodes" | "move") && lo + 1 < hi {
                    let mid = (lo + hi) / 2;
                    if inside(mid) { lo = mid } else { hi = mid }
                }
                Some(lo).filter(|&at| at > total / 2)
            });
            // (What a stop at the second read costs: the dispatch, no resize work done.)
            let mut again = None;
            for at in [Some(2), Some(total / 2), last].into_iter().flatten() {
                let (mut lua, f, r) = stop_at(at);
                assert!(
                    matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Time))),
                    "{name} at {at}: {r:?}"
                );
                let stopped = (1u64 << 40) - lua.fuel_left();
                let again = *again.get_or_insert(stopped);
                lua.set_time_limit(None);
                // (The nodes and move cases write before the resize: compare with what the
                // twin holds only when nothing was written.)
                let now = snapshot(&mut lua);
                if !matches!(name, "nodes" | "move") {
                    assert_eq!(
                        (now.0.len(), now.1),
                        (before.0.len(), before.1),
                        "{name} fast {fast}: stopped at read {at} of {total}, the table changed"
                    );
                    assert!(
                        (before.2..=before.2 + after.2).contains(&now.2),
                        "{name} fast {fast}: stopped at read {at} of {total}, memory {} (before {}, done {})",
                        now.2,
                        before.2,
                        after.2
                    );
                    let diff = now.0.iter().zip(&before.0).position(|(a, b)| a != b);
                    assert!(
                        now.0 == before.0,
                        "{name} fast {fast}: stopped at read {at} of {total}, the entries changed at {diff:?}: {:?} vs {:?}",
                        diff.map(|d| &now.0[d]),
                        diff.map(|d| &before.0[d])
                    );
                }
                // Done again, unstopped: the twin's table and fuel.
                if !matches!(name, "nodes" | "move") {
                    lua.set_limits(lua.limits().fuel(1 << 40));
                    lua.call(f.value(), &[]).unwrap();
                    // The work charged once in all: what the stopped call did, the rest when
                    // it is done (LR-29d).
                    assert_eq!(
                        stopped + (1u64 << 40) - lua.fuel_left(),
                        fuel + again,
                        "{name} fast {fast}: fuel after a stopped resize at read {at}"
                    );
                    assert_eq!(
                        snapshot(&mut lua),
                        after,
                        "{name} fast {fast}: the table after a stopped resize"
                    );
                }
            }
        }
    }
}

/// The fuel of growing big tables is the program's: the same on both paths and under any
/// seed (the looks between pieces cost nothing).
#[test]
fn res_table_resize_fuel_is_the_programs() {
    let _heavy = super::common::memory_heavy();
    for (name, setup, write) in one_short_of_a_resize() {
        let src = format!("{setup} {write}");
        let fuel = |seed: u64, fast: bool| {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
            lua.exec(&src, "=t").unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        let first = fuel(1, true);
        for (seed, fast) in [(1, false), (2, true), (0xdead_beef, false), (super::common::TEST_SEED, true)] {
            assert_eq!(fuel(seed, fast), first, "{name}: seed {seed} fast {fast}");
        }
    }
}

/// A resize given up at the deadline goes on in the next call from the host from where it
/// stopped (`Table::partial`), at least a piece of it in each: a script run in slices
/// shorter than a resize still gets past it (given up every time and begun again, it
/// never did; `sweep6_tables::s6t_regrow_stopped_mid_move_and_swept_mid_move` caught it).
/// A slice here is three reads of the clock: about a piece.
#[test]
fn res_table_resize_progresses_in_short_slices() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let _heavy = super::common::memory_heavy();
    // (Not `move`: its own million writes take longer than a slice, a new call each time.)
    for (name, setup, write) in one_short_of_a_resize().into_iter().filter(|c| c.0 != "move") {
        for fast in [true, false] {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
            lua.exec(setup, "=setup").unwrap();
            let f = lua.load(format!("{write} DONE = true"), "=write").unwrap();
            let mut tries = 0;
            loop {
                tries += 1;
                let reads = Arc::new(AtomicU64::new(0));
                let r = Arc::clone(&reads);
                let clock: remedy_lua::Clock =
                    Arc::new(move || std::time::Duration::from_nanos(r.fetch_add(1, Ordering::Relaxed)));
                lua.set_time_limit(Some((clock, std::time::Duration::from_nanos(3))));
                if lua.call(f.value(), &[]).is_ok() {
                    break;
                }
                // (Every pass of the biggest resize here in pieces: under 1,024.)
                assert!(tries < 1024, "{name} fast {fast}: no progress after {tries} slices");
            }
            lua.set_time_limit(None);
            assert_eq!(lua.get_global("DONE").unwrap(), remedy_lua::Value::Bool(true), "{name}");
        }
    }
}

/// The toggle of cost_fuzz_quick (fixed #4): a string key put into a table whose hash
/// keys are all integers moves them to `nodes`, 418,000 units for 2^17 keys. It was
/// charged after it ran, so a call with 100,000 units left did all of it (10x plain code
/// per unit of fuel under load). Now a move past the call's fuel is refused before it
/// touches the table, on both paths and under any seed.
#[test]
fn res_table_key_move_fuel_is_the_programs() {
    let setup = "T = {} for i = 1, (1 << 17) - 1 do T[i * 3] = i end";
    let mut seen = None;
    for seed in [1u64, 2, 0x5eed_7e57] {
        for fast in [true, false] {
            let mut lua = state(Limits::default().fuel(1 << 30).memory(1 << 30).hash_seed(seed), fast);
            lua.exec(setup, "=setup").unwrap();
            lua.set_limits(lua.limits().fuel(100_000));
            let r = lua.exec("T.zz = 1", "=toggle");
            assert!(
                matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))),
                "seed {seed} fast {fast}: {r:?}"
            );
            lua.set_limits(lua.limits().fuel(1 << 30));
            let left = lua.exec("return rawget(T, 'zz'), #T", "=check").unwrap();
            assert!(
                matches!(left[0], remedy_lua::Value::Nil),
                "seed {seed} fast {fast}: the key went in past the fuel"
            );
            // With the fuel for it, the move is done and costs the same everywhere.
            lua.set_limits(lua.limits().fuel(1_000_000));
            lua.exec("T.zz = 1", "=toggle").unwrap();
            let used = 1_000_000 - lua.fuel_left();
            assert_eq!(*seen.get_or_insert(used), used, "seed {seed} fast {fast}");
        }
    }
}

/// A resize given up at the deadline resumes in the next call from where it stopped, under
/// the watch (sweep6's stop-sweep review of 2903b95). It was run to the end unwatched
/// instead, so a host slicing a script into 1 ms calls saw one call of the whole resize:
/// 19 to 185 ms here. Now every call ends within a piece of its deadline (its thread's CPU
/// time, so a loaded machine's preemption does not count), and the resize still finishes.
#[test]
fn res_table_resize_resumes_in_1ms_slices() {
    let _alone = super::common::measuring();
    // (Not `move`: its own million writes take longer than a slice, a new call each time.)
    for (name, setup, write) in one_short_of_a_resize().into_iter().filter(|c| c.0 != "move") {
        for fast in [true, false] {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
            lua.exec(setup, "=setup").unwrap();
            let want = lua
                .exec(
                    format!("{setup} {write} local n = 0 for _ in pairs(T) do n = n + 1 end return n, #T"),
                    "=want",
                )
                .unwrap();
            lua.exec(setup, "=setup").unwrap();
            let f = lua.load(format!("{write} DONE = true"), "=write").unwrap();
            let clock: remedy_lua::Clock = {
                let t0 = std::time::Instant::now();
                Arc::new(move || t0.elapsed())
            };
            lua.set_time_limit(Some((clock, std::time::Duration::from_millis(1))));
            let (mut calls, mut longest) = (0, 0);
            loop {
                calls += 1;
                let t = remedy_lua_oracle::thread_cpu();
                let r = lua.call(f.value(), &[]);
                longest = longest.max(remedy_lua_oracle::thread_cpu().saturating_sub(t));
                match r {
                    Ok(_) => break,
                    Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Time)) => {}
                    Err(e) => panic!("{name} fast {fast}: {e:?}"),
                }
                assert!(calls < 5000, "{name} fast {fast}: no end after {calls} slices");
            }
            lua.set_time_limit(None);
            assert_eq!(lua.get_global("DONE").unwrap(), remedy_lua::Value::Bool(true));
            let got = lua.exec("local n = 0 for _ in pairs(T) do n = n + 1 end return n, #T", "=n").unwrap();
            assert_eq!(got, want, "{name} fast {fast}");
            // (A piece is about 0.2 to 2.8 ms here; up to 64 ms before.)
            assert!(
                longest < 12_000_000,
                "{name} fast {fast}: a slice of {} ms ({calls} calls)",
                longest as f64 / 1e6
            );
        }
    }
}

/// SEC7-N5 (low): the in-place plain `find` searched to its budget, past its cap, and gave
/// the call to the full path, which searched again: a 1 MiB miss took 2x the CPU per unit
/// of fuel with the fast loop (2.01 s against 0.97 s). Now one whose scan passes the cap
/// goes to the full path first, and the rest stop at the cap. The same fuel, and about the
/// same CPU, in both loops (200 misses: 143 ms against 76 before, 73 against 71 now).
#[test]
fn sec7_n5_an_in_place_find_does_not_search_twice() {
    let _alone = super::common::measuring();
    let prog = "local h = ('a'):rep(1 << 20) for i = 1, 200 do h:find('b', 1, true) end";
    let run = |fast: bool| {
        let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
        let f = lua.load(prog, "=n5").unwrap();
        let t = remedy_lua_oracle::thread_cpu();
        lua.call(f.value(), &[]).unwrap();
        (remedy_lua_oracle::thread_cpu().saturating_sub(t), lua.fuel_left())
    };
    // The best of three each, alternating: one run of a pair slowed by the machine was 1.3
    // to 1.5x the other now and then, on either side (80 pairs beside a full suite: medians
    // 1.00, the slowest runs on the full side).
    let (_, _) = (run(true), run(false));
    let (mut fast, mut full) = (run(true), run(false));
    for _ in 0..2 {
        (fast, full) = (fast.min(run(true)), full.min(run(false)));
    }
    assert_eq!(fast.1, full.1, "fuel");
    assert!(fast.0 * 4 < full.0 * 5, "fast {} ms, full {} ms", fast.0 as f64 / 1e6, full.0 as f64 / 1e6);
}

/// SEC7-H3: a full collection cut a slab's free tail under a quarter of it (which the
/// sweep leaves) in one go after the sweep, with no look at the deadline: 12 to 16 ms for
/// 720,000 dead tables after 2,280,000 live ones. It is cut a piece at a time now, with a
/// look between pieces: more reads than with the same dead tables at the front (none more
/// before).
#[test]
fn sec7_h3_a_full_collection_trims_between_looks() {
    let (live, dead) = (
        "K = {} for i = 1, 200000 do K[i] = {} end",
        "local d = {} for i = 1, 60000 do d[i] = {} end d = nil",
    );
    let tail = clock_reads(&format!("collectgarbage('stop') {live} {dead}"), "collectgarbage()");
    let front = clock_reads(&format!("collectgarbage('stop') {dead} {live}"), "collectgarbage()");
    assert!(tail >= front + 10, "{tail} reads with a dead tail, {front} with the dead at the front");
}

/// SEC7-T2 (high): a resize given up at the deadline went on only while the table was
/// unwritten, so a script writing a field before its resizing write (`T.x = T.x + 1
/// T[n + 1] = 1`) threw the kept part away and started over in every 1 ms call: no end
/// after 3,000 calls, against 95 without the earlier write. Now a value written over a value
/// keeps the part and is written into what it made too (the new array buffer being copied,
/// the new `nodes`), for any number of keys.
#[test]
fn sec7_t2_a_written_table_resumes_its_resize() {
    let _alone = super::common::measuring();
    // Setup, the writes before the resizing one, the resizing write, the key call `i` wrote
    // over (once: rewritten each call, an earlier value lost would be put back), the keys
    // and the border after. `L[i]`: call `i` got to its write.
    let cases = [
        // (32 keys fill the hash part: the resizing write rehashes.)
        (
            "T = {x = 0} for i = 1, 31 do T['f' .. i] = 0 end for i = 1, 1 << 21 do T[i] = 0 end",
            "for i = 1, 31 do T['f' .. i] = C end T.x = C T[C] = C",
            "T[(1 << 21) + 1] = 1",
            "i",
            (1 << 21) + 33,
            (1 << 21) + 1,
        ),
        // (A 16 MiB array buffer, copied a piece at a time.)
        (
            "T = {} for i = 1, 1 << 20 do T[i] = 0 end",
            "T[C] = C",
            "T[(1 << 20) + 1] = 1",
            "i",
            (1 << 20) + 1,
            (1 << 20) + 1,
        ),
        // (Odd keys: the integer part rebuilt a piece at a time.)
        (
            "T = {} for i = 1, 1 << 20 do T[2 * i + 1] = 0 end",
            "T[2 * C + 1] = C",
            "T[-1] = 1",
            "2 * i + 1",
            (1 << 20) + 1,
            0,
        ),
        // (`nodes` at 3/4 of 2^20 slots, Lua's node array not full: rebuilt a piece at a time.)
        (
            "T = {} for i = 1, 3 << 18 do T['k' .. i] = 0 end",
            "T['k' .. C] = C",
            "T.new = 1",
            "'k' .. i",
            (3 << 18) + 1,
            0,
        ),
    ];
    for (setup, first, grow, check, n, border) in cases {
        for fast in [true, false] {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
            lua.exec(format!("C = 0 L = {{}} for i = 1, 1000 do L[i] = false end {setup}"), "=setup")
                .unwrap();
            // (`L[C]` set once its writes are done: a call can be stopped before them, at a
            // collector step's look.)
            let f = lua.load(format!("C = C + 1 {first} L[C] = true {grow} DONE = true"), "=write").unwrap();
            let clock: remedy_lua::Clock = {
                let t0 = std::time::Instant::now();
                Arc::new(move || t0.elapsed())
            };
            lua.set_time_limit(Some((clock, std::time::Duration::from_millis(1))));
            let mut calls = 0;
            loop {
                calls += 1;
                match lua.call(f.value(), &[]) {
                    Ok(_) => break,
                    Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Time)) => {}
                    Err(e) => panic!("{first} fast {fast}: {e:?}"),
                }
                assert!(calls < 1000, "{first} fast {fast}: no end after {calls} slices");
            }
            lua.set_time_limit(None);
            assert!(calls > 3, "{first} fast {fast}: {calls} calls (no resize given up)");
            // The last call's writes are what the table holds, not an earlier call's.
            let check = format!(
                "(function() for i = 1, C do if L[i] and T[{check}] ~= i then return false end end return true end)()"
            );
            let got = lua
                .exec(format!("local n = 0 for _ in pairs(T) do n = n + 1 end return n, #T, {check}"), "=n")
                .unwrap();
            let want =
                [remedy_lua::Value::Int(n), remedy_lua::Value::Int(border), remedy_lua::Value::Bool(true)];
            assert_eq!(got, want, "{first} fast {fast} ({calls} calls)");
        }
    }
}

/// The clock's reads in `write` after `setup` (a deadline that never comes), and the state.
fn reads_of(setup: &str, write: &str, expire_at: usize) -> (usize, Lua) {
    let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let r2 = Arc::clone(&reads);
    let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), true);
    lua.exec(setup, "=setup").unwrap();
    lua.set_time_limit(Some((
        Arc::new(move || {
            let n = r2.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n >= expire_at { std::time::Duration::from_secs(7200) } else { std::time::Duration::ZERO }
        }),
        std::time::Duration::from_secs(3600),
    )));
    reads.store(0, std::sync::atomic::Ordering::Relaxed);
    let _ = lua.exec(write, "=w");
    (reads.load(std::sync::atomic::Ordering::Relaxed), lua)
}

/// LR-29b: a resize's old part of 2^21 nodes (64 MiB) was freed in one go after its last
/// look, 2 to 3 ms (the kernel's unmapping: 26 ms at 512 MiB, 12 ms past a 1 ms slice once
/// in a loaded full run). Now it is freed a piece at a time with a look after each
/// (`table::unpark`), so the clock is read at least once per 512 KiB freed; stopped part-way,
/// the rest stays in the account until a later look frees it.
#[test]
fn lr29b_a_big_old_part_is_freed_between_looks() {
    let setup = "T = {} for i = 1, 3 << 19 do T[i + 0.5] = i end";
    let write = "for i = 1, 4 do T[-i - 0.5] = i end";
    let (small, _) = reads_of("T = {} for i = 1, 3 << 17 do T[i + 0.5] = i end", write, usize::MAX);
    let (reads, _) = reads_of(setup, write, usize::MAX);
    // (A quarter of the keys: the resize's own looks, a quarter as many; the part freed is
    // 128 pieces.)
    assert!(reads >= 4 * small + 100, "{reads} reads ({small} for a quarter of the keys)");
    // Stopped half-way through the free: the half not freed yet is still counted, and goes
    // when a later look frees it.
    let (_, mut lua) = reads_of(setup, write, reads - 64);
    let stopped = lua.memory_used();
    lua.set_time_limit(None);
    lua.exec("local x = 0 for i = 1, 100000 do x = x + i end", "=later").unwrap();
    let later = lua.memory_used();
    assert!(stopped >= later + (24 << 20), "{stopped} bytes counted when stopped, {later} once freed");
}

/// LR-29d: a charge past the call's slice was dropped when the call stopped before the
/// reserve paid it (`fuel_left` counted the slice's overdraft as nothing): a resize
/// finished by a call stopped at its next look was reported as the slice alone (16,384
/// units of 3,670,016 for a 2^22 -> 2^23 growth), so a host slicing finely got resizes
/// nearly free (18,433 units in all for 1,025 calls). Now the work is charged once
/// whatever the slicing: every schedule costs the uninterrupted call plus the chunk's own
/// dispatch for each call it is run again, on both paths and under any seed.
#[test]
fn lr29d_a_resize_is_charged_once_however_the_call_is_sliced() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    for seed in [1, 7, 0xdead_beef] {
        for fast in [true, false] {
            let run = |every: usize| {
                let (reads, stop) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(every)));
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec("T = {} for i = 1, 1 << 20 do T[i] = i end", "=setup").unwrap();
                let (r, s) = (Arc::clone(&reads), Arc::clone(&stop));
                lua.set_time_limit(Some((
                    Arc::new(move || {
                        let n = r.fetch_add(1, Relaxed) + 1;
                        let at = s.load(Relaxed);
                        std::time::Duration::from_secs(if at != 0 && n >= at { 7200 } else { 0 })
                    }),
                    std::time::Duration::from_secs(3600),
                )));
                let f = lua.load("T[(1 << 20) + 1] = 1", "=grow").unwrap();
                let (mut fuel, mut calls) = (Vec::new(), 0);
                loop {
                    calls += 1;
                    reads.store(0, Relaxed);
                    let r = lua.call(f.value(), &[]);
                    fuel.push((1u64 << 40) - lua.fuel_left());
                    if r.is_ok() {
                        return fuel;
                    }
                    assert!(calls < 10_000, "seed {seed} fast {fast} every {every}: no end");
                }
            };
            let whole = run(0)[0];
            // The chunk's dispatch before its store: what a call stopped at its first look costs.
            let again = run(2)[0];
            for every in [2, 4, 18] {
                let fuel = run(every);
                let total: u64 = fuel.iter().sum();
                assert!(fuel.len() > 3, "seed {seed} fast {fast} every {every}: {} calls", fuel.len());
                assert_eq!(
                    total,
                    whole + again * (fuel.len() as u64 - 1),
                    "seed {seed} fast {fast} every {every}: {} calls",
                    fuel.len()
                );
            }
        }
    }
}

/// LR-29d: work charged after it was done ran past the call's fuel and the rest of its
/// charge was dropped: a new key into a full hash part (6,619,144 units for 2^21 slots in a
/// call of 20,000), the room a rehash makes in `nodes`, the rebuild a string key into an
/// integer part makes, a constructor's list grown, and the marking of any growth while the
/// collector marks. No call gets past the growth with less than the fuel it costs now, the
/// table as it was until then, on both paths and under any seed: the list grown is refused
/// whole (a constructor's table is the call's own), the rest are made a piece at a time and
/// kept between calls, in once the calls together have paid for them (SEC8-F4).
/// The fuel a call of `lr29d_a_table_grows_only_with_the_fuel_for_it` may pay past the
/// growth's own: its write's steps again, and a piece of the growth's work it could not pay
/// for (SEC8-F4).
const LR29D_SLACK: u64 = 100;

#[test]
fn lr29d_a_table_grows_only_with_the_fuel_for_it() {
    let cases = [
        // (`kept`: made a piece at a time within each call's fuel and kept between calls,
        // SEC8-F4; else refused whole.)
        (
            "nodes",
            "T = {} for i = 1, 3 << 18 do T[i + 0.5] = i end",
            "T[-0.5] = 1",
            "return rawget(T, -0.5)",
            true,
        ),
        (
            "room",
            "T = {x = 1} for i = 1, 1 << 20 do T[i] = i end for i = 1, (1 << 20) - 4096 do T[i] = nil end",
            "for i = 1, 64 do T['s' .. i] = i end",
            "return rawget(T, 's64'), #T",
            true,
        ),
        (
            "to_counting",
            "T = {} for i = 1, 1 << 17 do T[i * 7] = i end",
            "T.s = 1",
            "return rawget(T, 's')",
            true,
        ),
        ("set_list", "L = {} for i = 1, 1 << 20 do L[i] = i end", "T = {table.unpack(L)}", "return T", false),
        (
            "marking",
            "T = {} for i = 1, 1 << 20 do T[i] = i end collectgarbage() collectgarbage('step', 1)",
            "T[(1 << 20) + 1] = 1",
            "return rawget(T, (1 << 20) + 1)",
            true,
        ),
    ];
    for (name, setup, write, look, kept) in cases {
        for seed in [1, 7, 0xdead_beef] {
            for fast in [true, false] {
                let fresh = || {
                    let mut lua =
                        state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                    lua.exec(setup, "=setup").unwrap();
                    let f = lua.load(write, "=write").unwrap();
                    (lua, f)
                };
                let (mut lua, f) = fresh();
                lua.call(f.value(), &[]).unwrap();
                let whole = (1u64 << 40) - lua.fuel_left();
                let (mut lua, f) = fresh();
                let before = lua.exec(look, "=look").unwrap();
                let mut paid = 0;
                for (call, budget) in [whole / 8, whole / 4, whole / 2, whole * 7 / 8].into_iter().enumerate()
                {
                    lua.set_limits(lua.limits().fuel(budget));
                    let r = lua.call(f.value(), &[]);
                    paid += budget - lua.fuel_left();
                    // Kept, it gets past once the calls together have paid for it, and in that
                    // call: their fuel the growth's and a piece a call at most.
                    if kept && paid >= whole {
                        assert!(
                            r.is_ok(),
                            "{name} seed {seed} fast {fast}: {paid} paid of {whole}, still refused"
                        );
                        assert!(
                            paid <= whole + (call as u64 + 1) * LR29D_SLACK,
                            "{name} seed {seed} fast {fast}: {paid} paid for {whole}"
                        );
                        break;
                    }
                    assert!(
                        r.is_err(),
                        "{name} seed {seed} fast {fast}: a call of {budget} got past a growth of {whole}, {paid} paid"
                    );
                    lua.set_limits(lua.limits().fuel(1 << 40));
                    assert_eq!(
                        lua.exec(look, "=look").unwrap(),
                        before,
                        "{name} seed {seed} fast {fast}: the table changed at {budget} of {whole}"
                    );
                }
                if !kept {
                    lua.set_limits(lua.limits().fuel(whole * 2));
                    assert!(
                        lua.call(f.value(), &[]).is_ok(),
                        "{name} seed {seed} fast {fast}: refused at {} for {whole}",
                        whole * 2
                    );
                }
            }
        }
    }
}

/// LR-29d (reseed): an insert that made a chain longer than `CHAIN_FREE + 1` (keys aimed at
/// the seed) reseeded the part after it was in, its charge unchecked. Now the next insert
/// does it, refused before anything when the call cannot pay for it: the seed (the order
/// `next` goes in) and the table stay as they were until a call has the reseed's cost, and
/// that call reseeds. On both paths and under any seed.
#[test]
fn lr29d_a_reseed_waits_for_an_insert_that_can_pay_for_it() {
    use super::sweep5b_open::{PAIR_FULL, aimed_floats, homes, list, table_seeds};
    let (n, bits) = (1usize << 14, 15u32);
    let mask = (1u64 << bits) - 1;
    let look = "local s, k = '', nil for i = 1, 16 do k = next(T, k) s = s .. tostring(k) .. ',' end S = S or s return s == S, rawget(T, 'x')";
    for seed in [1, 7, 0xdead_beef] {
        let ts = table_seeds(seed, 1);
        // A pair of homes no plain key has, and keys aimed at it: the last makes a chain of
        // `CHAIN_FREE + 2`.
        let home = (0..mask / 2)
            .find(|h| {
                (1..=n).all(|i| {
                    let (a, b) = homes(i as f64 + 0.25, ts[0], bits);
                    ![a, b].contains(h) && ![a, b].contains(&(h ^ mask))
                })
            })
            .unwrap();
        let aimed = aimed_floats(&ts, bits, home, PAIR_FULL + 1);
        let setup = format!(
            "T = {{}} for i = 1, {n} do T[i + 0.25] = i end for _, f in ipairs({{{}}}) do T[f] = f end",
            list(&aimed)
        );
        for fast in [true, false] {
            let fresh = || {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec(&setup, "=setup").unwrap();
                let f = lua.load("T.x = 1", "=write").unwrap();
                (lua, f)
            };
            let (mut lua, f) = fresh();
            lua.call(f.value(), &[]).unwrap();
            let whole = (1u64 << 40) - lua.fuel_left();
            let g = lua.load("T.y = 1", "=plain").unwrap();
            lua.call(g.value(), &[]).unwrap();
            let plain = (1u64 << 40) - lua.fuel_left();
            // (Its placements at least: a slot of the part a unit of `resized`.)
            assert!(
                whole >= plain + (1 << bits) / 16,
                "seed {seed} fast {fast}: the insert after the aimed keys cost {whole}, a plain one {plain}"
            );
            let (mut lua, f) = fresh();
            let before = lua.exec(look, "=look").unwrap();
            for budget in [whole / 8, whole / 4, whole / 2, whole * 7 / 8] {
                lua.set_limits(lua.limits().fuel(budget));
                assert!(
                    lua.call(f.value(), &[]).is_err(),
                    "seed {seed} fast {fast}: a call of {budget} reseeded for {whole}"
                );
                lua.set_limits(lua.limits().fuel(1 << 40));
                assert_eq!(
                    lua.exec(look, "=look").unwrap(),
                    before,
                    "seed {seed} fast {fast}: the table changed at {budget} of {whole}"
                );
            }
            lua.set_limits(lua.limits().fuel(whole * 2));
            assert!(
                lua.call(f.value(), &[]).is_ok(),
                "seed {seed} fast {fast}: refused at {} for {whole}",
                whole * 2
            );
            let after = lua.exec(look, "=look").unwrap();
            assert!(
                matches!(after[..], [remedy_lua::Value::Bool(false), remedy_lua::Value::Int(1)]),
                "seed {seed} fast {fast}: not reseeded"
            );
        }
    }
}

/// LR-29d: a resize given up between pieces (the interrupt or deadline) was refunded, so
/// one thrown away by a write before it was taken up again was never paid: a script could
/// have a rehash or a rebuild of a big part done again and again for the chunk's dispatch.
/// What the stopped call did stays charged now (the count of a rehash; the new part made,
/// and the old part's slots read and placed so far, of a rebuild of `nodes`), the write
/// that throws it away costs what it costs on a fresh state, and the redo costs the
/// uninterrupted resize again. On both paths and under any seed.
#[test]
fn lr29d_a_resize_thrown_away_stays_paid() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let shapes = [
        ("T = {} for i = 1, 1 << 20 do T[i] = i end", "T[(1 << 20) + 1] = 1", "T[1] = nil", [70, 100]),
        ("T = {} for i = 1, 3 << 18 do T[i + 0.5] = i end", "T[-0.5] = 1", "T[1.5] = nil", [200, 240]),
    ];
    for (setup, grow, write, stops) in shapes {
        for seed in [1, 7, 0xdead_beef] {
            for fast in [true, false] {
                let fresh = |stop: usize| {
                    let mut lua =
                        state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                    lua.exec(setup, "=setup").unwrap();
                    let reads = Arc::new(AtomicUsize::new(0));
                    let r = Arc::clone(&reads);
                    lua.set_time_limit(Some((
                        Arc::new(move || {
                            let n = r.fetch_add(1, Relaxed) + 1;
                            std::time::Duration::from_secs(if stop != 0 && n >= stop { 7200 } else { 0 })
                        }),
                        std::time::Duration::from_secs(3600),
                    )));
                    lua
                };
                let cost = |lua: &mut Lua, src: &str| {
                    let f = lua.load(src, "=x").unwrap();
                    let r = lua.call(f.value(), &[]);
                    ((1u64 << 40) - lua.fuel_left(), r.is_ok())
                };
                let mut lua = fresh(0);
                let (w0, r0) = (cost(&mut lua, write).0, cost(&mut lua, grow).0);
                let again = cost(&mut fresh(2), grow).0;
                for stop in stops {
                    let mut lua = fresh(stop);
                    let (s, done) = cost(&mut lua, grow);
                    lua.set_time_limit(None);
                    let (w, r) = (cost(&mut lua, write).0, cost(&mut lua, grow));
                    let at = format!("{grow} seed {seed} fast {fast} stopped at {stop}");
                    assert!(!done && r.1, "{at}");
                    assert!(
                        s > again + 1 && s - again <= r0,
                        "{at}: the stopped call cost {s}, the chunk's dispatch {again}, the resize {r0}"
                    );
                    assert_eq!((w, r.0), (w0, r0), "{at}: the write and the redo");
                }
            }
        }
    }
}

/// A resize given up while it grows the array part cut its new tail off, and the next one
/// filled it again with no look before going on (the gate's 1 ms slices: one of 18.8 ms at
/// 2^21 slots, the slices but one spent filling again what the last had made). The tail
/// stays now, hidden past the array part, and is gone on with in place: every new slot is
/// made once however the resize is sliced, each key is seen once after, the account holds
/// the new buffer from the first stop, and fuel is the same on both paths and every seed.
#[test]
fn res_table_a_growth_given_up_goes_on_in_place() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let n = 1usize << 21;
    let mut fuels = Vec::new();
    for seed in [1, 7, 0xdead_beef] {
        for fast in [true, false] {
            for every in [2, 9] {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec(&format!("T = {{}} for i = 1, {n} do T[i] = i end"), "=setup").unwrap();
                let held = lua.memory_used();
                let reads = Arc::new(AtomicUsize::new(0));
                let r = Arc::clone(&reads);
                lua.set_time_limit(Some((
                    Arc::new(move || {
                        std::time::Duration::from_secs(if r.fetch_add(1, Relaxed) + 1 >= every {
                            7200
                        } else {
                            0
                        })
                    }),
                    std::time::Duration::from_secs(3600),
                )));
                let f = lua.load(&format!("T[{n} + 1] = 1"), "=grow").unwrap();
                lua.table_slots_staged();
                let (mut fuel, mut calls, mut made) = (0, 0, 0);
                loop {
                    calls += 1;
                    reads.store(0, Relaxed);
                    let r = lua.call(f.value(), &[]);
                    fuel += (1u64 << 40) - lua.fuel_left();
                    made += lua.table_slots_staged();
                    if r.is_ok() {
                        break;
                    }
                    // (Stopped: the new tail made so far is in the account, hidden or not.)
                    assert!(
                        lua.memory_used() >= held + made as usize * 16,
                        "seed {seed} fast {fast}: {} bytes held, {held} before, {made} slots made",
                        lua.memory_used()
                    );
                    assert!(calls < 10_000, "seed {seed} fast {fast} every {every}: no end");
                }
                let at = format!("seed {seed} fast {fast} every {every} ({calls} calls)");
                assert!(calls > 3, "{at}");
                assert_eq!(made, n as u64, "{at}: slots of the new tail made");
                lua.set_time_limit(None);
                let got = lua.exec(&format!("local c, s = 0, 0 for k, v in pairs(T) do c = c + 1 s = s + v end return c, s, #T, T[{n} + 1]"), "=look").unwrap();
                let want = lua
                    .exec(&format!("return {n} + 1, {n} * ({n} + 1) // 2 + 1, {n} + 1, 1"), "=want")
                    .unwrap();
                assert_eq!(got, want, "{at}: each key once");
                fuels.push((every, fuel));
            }
        }
    }
    for every in [2, 9] {
        let f: Vec<u64> = fuels.iter().filter(|x| x.0 == every).map(|x| x.1).collect();
        assert!(f.iter().all(|&x| x == f[0]), "every {every}: fuel {f:?}");
    }
}

/// The resize behaviour commit's safety net: a table whose growth is given up (a hidden new
/// array tail, then gone on with, `ArrPart`) at each of its reads in turn, with integer
/// keys already in the hash part inside the range it grows into, and the host writing such
/// keys at the stop (over their values: the resize kept, and written into its tail too; or
/// nil: thrown away), ends as C Lua's does with the same writes: every key's value, what
/// `pairs` visits, the border and a constructor's copy. Integer keys only, with a string
/// key (the other build), and with holes in the array; on both paths, the seed varying.
#[test]
fn res_table_stopped_resizes_match_c_lua() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let n = 1usize << 17;
    let evens = format!("for i = {n} + 2, {n} + 2 * 16384, 2 do T[i] = -i end");
    let shapes = [
        ("ints", format!("T = {{}} for i = 1, {n} do T[i] = i end {evens}")),
        ("string", format!("T = {{x = 'x'}} for i = 1, {n} do T[i] = i end {evens}")),
        (
            "holes",
            format!("T = {{}} for i = 1, {n} do T[i] = i end for i = 1, {n}, 7 do T[i] = nil end {evens}"),
        ),
    ];
    // (Gone on with from where the call stopped: `P`.) Its resizes include the array part
    // doubling to 2n, which takes in the keys above.
    let grow = format!("local i = P or {n} + 1 while i <= 2 * {n} + 1 do T[i] = i i = i + 2 P = i end");
    let keys = [n + 2, n + 4002, n + 32768];
    let actions = [
        ("over", keys.iter().map(|k| format!("T[{k}] = 'w' ")).collect::<String>()),
        ("nil", keys.iter().map(|k| format!("T[{k}] = nil ")).collect::<String>()),
    ];
    let digest = format!(
        "local h, c, ks = 0, 0, 0 \
         for i = 1, 2 * {n} + 2 * 16384 + 4 do local v = T[i] if v ~= nil then \
           h = (h * 31 + i * 7 + (type(v) == 'number' and v % 1000003 or #v)) % 1000000007 end end \
         for k, v in pairs(T) do c = c + 1 if math.type(k) == 'integer' then ks = (ks + k) % 1000000007 end end \
         local U = {{table.unpack(T, {n} - 8, {n} + 40)}} local u = 0 \
         for i = 1, 49 do if U[i] ~= nil then u = u + i end end \
         return h .. ' ' .. c .. ' ' .. ks .. ' ' .. #T .. ' ' .. u"
    );
    for (name, setup) in &shapes {
        let total = clock_reads(setup, &grow);
        assert!(total > 30, "{name}: {total} reads");
        for (act, w) in &actions {
            let c = remedy_lua_oracle::run(&format!("{setup} {w} {grow} {digest}"), 1 << 40).unwrap();
            for at in 2..=total {
                for fast in [true, false] {
                    let seed = [1, 7, 0xdead_beef][at % 3];
                    let mut lua =
                        state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                    lua.exec(setup, "=setup").unwrap();
                    let reads = Arc::new(AtomicUsize::new(0));
                    let r = Arc::clone(&reads);
                    lua.set_time_limit(Some((
                        Arc::new(move || {
                            std::time::Duration::from_secs(if r.fetch_add(1, Relaxed) + 1 >= at {
                                7200
                            } else {
                                0
                            })
                        }),
                        std::time::Duration::from_secs(3600),
                    )));
                    let f = lua.load(grow.as_str(), "=grow").unwrap();
                    let stopped = lua.call(f.value(), &[]).is_err();
                    lua.set_time_limit(None);
                    lua.exec(w.as_str(), "=host").unwrap();
                    lua.call(f.value(), &[]).unwrap();
                    let ours = lua.exec(digest.as_str(), "=digest").unwrap();
                    let ours = lua
                        .str_bytes(ours[0])
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default();
                    assert!(stopped, "{name} at {at}");
                    assert_eq!(
                        ours, c,
                        "{name} {act}, stopped at read {at} of {total}, seed {seed} fast {fast}"
                    );
                }
            }
        }
    }
}

/// Probe (the resize behaviour commit's measurements, recorded apart): a 2^20 -> 2^21 and
/// a 2^21 -> 2^22 growth of an array part (16 MiB to 32 MiB, copied a piece at a time;
/// 32 MiB to 64 MiB, `realloc`), uninterrupted and stopped every
/// 2 or 9 reads, on the trace of cef08a8 (`TraceOp::Reserve`: the new buffer reserved;
/// `TraceOp::Resize`: each piece of the new tail made): peak RSS (VmHWM, reset first),
/// peak accounted bytes (at every stop and the end), the reserve's time and the longest
/// tail piece. `cargo test --release --test all res_table_growth_measures -- --ignored
/// --nocapture`.
#[test]
#[ignore]
fn res_table_growth_measures() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let _alone = super::common::measuring();
    let kib = |key: &str| {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        s.lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0)
    };
    for n in [1usize << 20, 1 << 21] {
        for every in [0, 2, 9] {
            for fast in [true, false] {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
                lua.exec(&format!("T = {{}} for i = 1, {n} do T[i] = i end"), "=setup").unwrap();
                let held = lua.memory_used();
                let t0 = std::time::Instant::now();
                lua.gc_trace(Some(Arc::new(move || t0.elapsed())), 1 << 16);
                let reads = Arc::new(AtomicUsize::new(0));
                let r = Arc::clone(&reads);
                lua.set_time_limit(Some((
                    Arc::new(move || {
                        std::time::Duration::from_secs(
                            if every != 0 && r.fetch_add(1, Relaxed) + 1 >= every { 7200 } else { 0 },
                        )
                    }),
                    std::time::Duration::from_secs(3600),
                )));
                let f = lua.load(&format!("T[{n} + 1] = 1"), "=grow").unwrap();
                let _ = std::fs::write("/proc/self/clear_refs", "5");
                let rss0 = kib("VmRSS:");
                let (mut calls, mut peak) = (0, held);
                loop {
                    calls += 1;
                    reads.store(0, Relaxed);
                    let r = lua.call(f.value(), &[]);
                    peak = peak.max(lua.memory_used());
                    if r.is_ok() {
                        break;
                    }
                    assert!(calls < 10_000);
                }
                let hwm = kib("VmHWM:");
                let (recs, dropped) = lua.gc_trace_take();
                let of = |op: remedy_lua::TraceOp| recs.iter().filter(move |r| r.op == op);
                let reserve: std::time::Duration = of(remedy_lua::TraceOp::Reserve).map(|r| r.wall()).sum();
                let reserve_max =
                    of(remedy_lua::TraceOp::Reserve).map(|r| r.wall()).max().unwrap_or_default();
                let pieces = of(remedy_lua::TraceOp::Resize).count();
                let piece_max = of(remedy_lua::TraceOp::Resize).map(|r| r.wall()).max().unwrap_or_default();
                println!(
                    "2^{} every {every:>2} fast {fast:>5}: {calls:>3} calls | peak RSS +{} MiB (VmHWM {} MiB from RSS {} MiB) | peak accounted +{} MiB | reserve {:.2} ms in all, {:.2} ms at most ({} records) | tail {pieces} pieces, longest {:.3} ms | dropped {dropped}",
                    n.trailing_zeros(),
                    hwm.saturating_sub(rss0) / 1024,
                    hwm / 1024,
                    rss0 / 1024,
                    (peak - held) >> 20,
                    reserve.as_secs_f64() * 1e3,
                    reserve_max.as_secs_f64() * 1e3,
                    of(remedy_lua::TraceOp::Reserve).count(),
                    piece_max.as_secs_f64() * 1e3,
                );
            }
        }
    }
}

/// LR-29f: an array part of 1 to 32 MiB grown in one call is copied into its new buffer
/// a piece at a time, the old buffer beside it until the copy ends, uncounted: 16 MiB grown
/// to 32 held 32 MiB more against an account of 16 more, so a script near its cap went
/// past it. The old buffer is counted while it is there now: a growth the cap has room for
/// only without it is refused with a memory error, every key kept, and one with room for
/// both is done. On both paths and three seeds.
#[test]
fn lr29f_a_copied_array_part_counts_its_old_buffer() {
    let n = 1usize << 20;
    for seed in [1, 7, 0xdead_beef] {
        for fast in [true, false] {
            for (room, fits) in [(24usize, false), (40, true)] {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec(
                    &format!(
                        "T = {{}} for i = 1, {n} do T[i] = i end collectgarbage() collectgarbage('stop')"
                    ),
                    "=setup",
                )
                .unwrap();
                let used = lua.memory_used();
                lua.set_limits(lua.limits().memory(used + (room << 20)));
                lua.table_slots_copied();
                let r = lua.exec(&format!("T[{n} + 1] = 1"), "=grow");
                let at = format!("seed {seed} fast {fast}, {room} MiB of room");
                // (Segmented, nothing is copied and no old buffer is beside a new one.)
                let fits = fits || cfg!(feature = "segmented-arrays");
                assert_eq!(r.is_ok(), fits, "{at}: {r:?}");
                if cfg!(feature = "segmented-arrays") {
                    assert_eq!(lua.table_slots_copied().0, 0, "{at}: slots copied");
                }
                lua.set_limits(lua.limits().memory(1 << 30));
                let want = if fits { n + 1 } else { n };
                let got = lua
                    .exec(
                        "local c, s = 0, 0 for k, v in pairs(T) do c = c + 1 s = s + v end return c, s, #T",
                        "=look",
                    )
                    .unwrap();
                let s = (n * (n + 1) / 2 + usize::from(fits)) as i64;
                assert_eq!(
                    got,
                    vec![
                        remedy_lua::Value::Int(want as i64),
                        remedy_lua::Value::Int(s),
                        remedy_lua::Value::Int(want as i64)
                    ],
                    "{at}: every key kept"
                );
            }
        }
    }
}

/// The array part's storage changes no fuel: growths, a constructor's list, a shrink and
/// `table.insert` on big tables cost what `tests/fuel_matrix.txt` says, on the
/// contiguous build and the segmented one (`--features segmented-arrays`) alike, on both
/// paths and three seeds. Stopped every 3 and every 17 clock reads and gone on with, each
/// costs the same on every seed and ends with the table the unstopped run makes. (Not
/// compared across storages or paths: they read the clock at different points, so a
/// schedule of reads stops them at different places; a segmented growth copies nothing,
/// so it looks less often.) `REMEDYLUA_BLESS=1` writes the file (after a change of fuel).
#[test]
fn res_table_fuel_matrix_is_the_same_on_both_storages() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let shapes = [
        ("grow", "T = {}", "local i = P or 1 while i <= 300000 do T[i] = i i = i + 1 P = i end"),
        (
            "mixed",
            "T = {x = 'x'} for i = 70002, 70002 + 2 * 8192, 2 do T[i] = -i end",
            "local i = P or 1 while i <= 140001 do T[i] = i i = i + 1 P = i end",
        ),
        ("ctor", "L = {} for i = 1, 120000 do L[i] = i end", "T = {table.unpack(L)}"),
        (
            "shrink",
            "T = {} for i = 1, 1 << 17 do T[i] = i end",
            "local i = P or 1 while i <= 1 << 17 do if i % 3 ~= 0 then T[i] = nil end i = i + 1 P = i end for k = 1, 3000 do T['s' .. k] = k end",
        ),
        // (Done once whatever the stops: `#T`.)
        (
            "insert",
            "T = {}",
            "local i = P or 1 while i <= 200000 do if #T < i then table.insert(T, i) end i = i + 1 P = i end",
        ),
    ];
    let digest = "local c, s = 0, 0 for k, v in pairs(T) do c = c + 1 if math.type(v) == 'integer' then s = (s + v) % 1000000007 end end return c .. ' ' .. s .. ' ' .. #T";
    let (mut lines, mut unstopped) = (Vec::new(), Vec::new());
    for (name, setup, chunk) in shapes {
        // (The constructor unstopped only: stopped, it starts over in every call.)
        for every in [0usize, 3, 17].into_iter().filter(|&e| e == 0 || name != "ctor") {
            for fast in [true, false] {
                let mut seen: Option<(u64, String)> = None;
                for seed in [1, 7, 0xdead_beef] {
                    let mut lua =
                        state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                    lua.exec(setup, "=setup").unwrap();
                    let reads = Arc::new(AtomicUsize::new(0));
                    let r = Arc::clone(&reads);
                    lua.set_time_limit(Some((
                        Arc::new(move || {
                            std::time::Duration::from_secs(
                                if every != 0 && r.fetch_add(1, Relaxed) + 1 >= every { 7200 } else { 0 },
                            )
                        }),
                        std::time::Duration::from_secs(3600),
                    )));
                    let f = lua.load(chunk, "=chunk").unwrap();
                    let (mut fuel, mut calls) = (0u64, 0);
                    loop {
                        calls += 1;
                        reads.store(0, Relaxed);
                        let r = lua.call(f.value(), &[]);
                        fuel += (1u64 << 40) - lua.fuel_left();
                        if r.is_ok() {
                            break;
                        }
                        assert!(calls < 100_000, "{name} every {every}: no end");
                    }
                    lua.set_time_limit(None);
                    let d = lua.exec(digest, "=digest").unwrap();
                    let d = lua
                        .str_bytes(d[0])
                        .map(|b| String::from_utf8_lossy(b).into_owned())
                        .unwrap_or_default();
                    let got = (fuel, format!("{calls} calls, {d}"));
                    assert_eq!(
                        *seen.get_or_insert_with(|| got.clone()),
                        got,
                        "{name} every {every}: seed {seed} fast {fast}"
                    );
                }
                let (fuel, rest) = seen.unwrap_or_default();
                let table = rest.split_once(", ").map(|x| x.1.to_string()).unwrap_or_default();
                if every == 0 {
                    lines.push(format!("{name} fast {fast}: fuel {fuel}, {rest}"));
                    unstopped.push((name, fast, table));
                } else {
                    assert!(
                        unstopped.contains(&(name, fast, table.clone())),
                        "{name} every {every} fast {fast}: ended with {table}"
                    );
                }
            }
        }
    }
    let got = lines.join("\n") + "\n";
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fuel_matrix.txt");
    if std::env::var_os("REMEDYLUA_BLESS").is_some() {
        std::fs::write(path, &got).unwrap();
        return;
    }
    let want = std::fs::read_to_string(path).unwrap_or_default();
    assert_eq!(got, want, "the fuel matrix (segmented: {})", cfg!(feature = "segmented-arrays"));
}

/// Segmented storage (wasm; here the `segmented-arrays` build, at 4,096 slots): an array
/// part that reaches `seg::FROM` and grows on becomes segmented once, its buffer kept as
/// block 0, and no slot of what it held is ever copied again, however far it grows and
/// however the growth is sliced; every key is where it was, and the account holds the
/// blocks made at every stop. Both paths, three seeds.
#[cfg(feature = "segmented-arrays")]
#[test]
fn seg_a_segmented_array_part_grows_without_copying() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    for seed in [1, 7, 0xdead_beef] {
        for fast in [true, false] {
            for every in [0usize, 2] {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec("T = {} for i = 1, 4096 do T[i] = i end", "=setup").unwrap();
                let held = lua.memory_used();
                let reads = Arc::new(AtomicUsize::new(0));
                let r = Arc::clone(&reads);
                lua.set_time_limit(Some((
                    Arc::new(move || {
                        std::time::Duration::from_secs(
                            if every != 0 && r.fetch_add(1, Relaxed) + 1 >= every { 7200 } else { 0 },
                        )
                    }),
                    std::time::Duration::from_secs(3600),
                )));
                lua.table_slots_copied();
                lua.table_slots_staged();
                let f = lua
                    .load("local i = P or 4097 while i <= 1 << 19 do T[i] = i i = i + 1 P = i end", "=grow")
                    .unwrap();
                let (mut calls, mut made) = (0, 0u64);
                loop {
                    calls += 1;
                    reads.store(0, Relaxed);
                    let r = lua.call(f.value(), &[]);
                    made += lua.table_slots_staged();
                    if r.is_ok() {
                        break;
                    }
                    assert!(
                        lua.memory_used() >= held + made as usize * 16,
                        "seed {seed} fast {fast}: {} held, {held} before, {made} slots made",
                        lua.memory_used()
                    );
                    assert!(calls < 100_000);
                }
                let at = format!("seed {seed} fast {fast} every {every} ({calls} calls)");
                assert_eq!(
                    lua.table_slots_copied(),
                    (0, 1),
                    "{at}: slots copied, array parts made segmented"
                );
                lua.set_time_limit(None);
                let got = lua.exec("local c, s = 0, 0 for k, v in pairs(T) do c = c + 1 s = s + v end local ok = true for i = 1, #T do ok = ok and T[i] == i end return c, s, #T, ok", "=look").unwrap();
                let n = 1i64 << 19;
                assert_eq!(
                    got,
                    vec![
                        remedy_lua::Value::Int(n),
                        remedy_lua::Value::Int(n * (n + 1) / 2),
                        remedy_lua::Value::Int(n),
                        remedy_lua::Value::Bool(true)
                    ],
                    "{at}"
                );
            }
        }
    }
}

/// An array part cut back within its storage and grown again into what it held (on a
/// segmented one, within a block: its block 0 a constructor's 5,000 slots, so its blocks
/// do not end where the array's powers of two do): the slots taken in again are nil, as in
/// C Lua, not what they held before. Every key, what `pairs` visits and the border, against
/// C Lua; on both paths.
#[test]
fn res_table_regrowth_matches_c_lua() {
    let digest = "local h, c = 0, 0 for i = 1, 1 << 17 do local v = T[i] if v ~= nil then h = (h * 31 + i + v % 1000003) % 1000000007 end end \
                  for k, v in pairs(T) do c = c + 1 end return h .. ' ' .. c .. ' ' .. #T";
    for setup in [
        "T = {} for i = 1, 3 << 16 do T[i] = i end for i = (2 << 16) + 1, 3 << 16 do T[i] = nil end \
         for k = 1, 5000 do T['s' .. k] = k end for i = (2 << 16) + 1, (2 << 16) + 40000 do T[i] = -i end",
        // (The keys left above 2^14 go to the hash part when it shrinks there, their slots
        // keeping what they held, and are removed; the array grows over them again.)
        "L = {} for i = 1, 5000 do L[i] = i end T = {table.unpack(L)} for i = 5001, 1 << 15 do T[i] = i end \
         for i = 8193, 1 << 14, 2 do T[i] = nil end \
         for i = (1 << 14) + 1, 1 << 15 do if i % 5 ~= 0 then T[i] = nil end end \
         for k = 1, 5000 do T['s' .. k] = k end \
         for i = (1 << 14) + 1, 1 << 15 do if i % 5 == 0 then T[i] = nil end end \
         for i = 8193, 1 << 14, 2 do T[i] = i end \
         for i = (1 << 14) + 1, (1 << 14) + 12000 do if i % 5 ~= 0 then T[i] = -i end end",
    ] {
        let src = format!("{setup} {digest}");
        let c = remedy_lua_oracle::run(&src, 1 << 40).unwrap();
        for fast in [true, false] {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
            let ours = lua.exec(&src, "=regrow").unwrap();
            let ours =
                lua.str_bytes(ours[0]).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            assert_eq!(ours, c, "{setup} fast {fast}");
        }
    }
}

/// SEC8-F1: a call stopped at its deadline reports the fuel it has left, not 0. Each call
/// below stops at its clock's 3rd read and is called again until it finishes: a shrink
/// of the array part, then string keys. (The stop came inside a resumed atomic phase,
/// which zeroed the fuel as a stop at the fuel limit does.)
#[test]
fn sec8_f1_a_call_stopped_at_its_deadline_reports_its_fuel_left() {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
        lua.exec("T = {} for i = 1, 1 << 17 do T[i] = i end", "=setup").unwrap();
        let reads = Arc::new(AtomicU64::new(0));
        let r = Arc::clone(&reads);
        lua.set_time_limit(Some((
            Arc::new(move || std::time::Duration::from_secs(r.fetch_add(1, Relaxed))),
            std::time::Duration::from_secs(2),
        )));
        let body =
            "local i = P or 1 while i <= 1 << 17 do if i % 3 ~= 0 then T[i] = nil end i = i + 1 P = i end
            for k = 1, 3000 do T['s' .. k] = k end";
        let (mut stops, mut zero) = (0, Vec::new());
        for call in 0.. {
            assert!(call < 10_000, "fast {fast}: no end");
            reads.store(0, Relaxed);
            match lua.exec(body, "=f1") {
                Ok(_) => break,
                Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Time)) => {
                    stops += 1;
                    if lua.fuel_left() == 0 {
                        zero.push(call);
                    }
                }
                Err(e) => panic!("fast {fast}: call {call}: {e}"),
            }
        }
        assert!(stops > 10, "fast {fast}: {stops} stops");
        assert!(
            zero.is_empty(),
            "fast {fast}: calls stopped at the deadline with no fuel left: {zero:?} of {stops}"
        );
    }
}

/// SEC8-F4 (medium): a rehash bigger than a call's fuel was refused before it began and
/// kept nothing, so under a fixed fuel a call of `F` could never add a key to a table of
/// more than about `2F` keys (about half a unit of fuel a key): 2^16 keys at 20,000 never.
/// It is made a piece at a time within each call's fuel now, kept between calls and charged
/// once: the key lands after a bounded number of calls, the calls' fuel together the
/// growth's whole cost and a piece a call at most, the same under every seed and on both
/// paths; and its passes read each part once (a call stopped again goes on where the last
/// one stopped, SEC7-T4): at 2^18 keys, under a fuel watch from its first call.
#[test]
fn sec8_f4_a_rehash_bigger_than_a_call_is_made_across_calls() {
    const FUEL: u64 = 20_000;
    // (A piece a call: the write's own steps and one piece of the scan, 512 slots of 3
    // units, past what a call could pay.)
    const SLACK: u64 = 100;
    for bits in [16, 18] {
        let setup = format!("collectgarbage('stop') T = {{}} for i = 1, 1 << {bits} do T['k' .. i] = i end");
        let mut seen = None;
        for seed in [1u64, 7, 0xdead_beef] {
            for fast in [true, false] {
                let fresh = || {
                    let mut lua =
                        state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                    lua.exec(&setup, "=setup").unwrap();
                    let f = lua.load("T.new = 1", "=add").unwrap();
                    (lua, f)
                };
                let (mut lua, f) = fresh();
                lua.call(f.value(), &[]).unwrap();
                let whole = (1u64 << 40) - lua.fuel_left();
                assert!(whole > FUEL, "2^{bits} seed {seed} fast {fast}: the growth costs {whole}");
                let (mut lua, f) = fresh();
                let Ok(remedy_lua::Value::Table(t)) = lua.get_global("T") else { panic!("T") };
                let key = lua.create_string("new").unwrap();
                // (Its hash part: 2^(bits + 1) slots for 2^bits keys.)
                let slots = 2u64 << bits;
                lua.set_limits(lua.limits().fuel(FUEL));
                let _ = lua.table_slots_watched();
                let mut used = Vec::new();
                loop {
                    assert!(
                        (used.len() as u64) < whole / FUEL + 2,
                        "2^{bits} seed {seed} fast {fast}: not in after {used:?} of {whole}"
                    );
                    let r = lua.call(f.value(), &[]);
                    used.push(FUEL - lua.fuel_left());
                    if r.is_ok() {
                        break;
                    }
                    assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
                    assert!(
                        lua.table_get(t, key).is_nil(),
                        "2^{bits} seed {seed} fast {fast}: in before the growth was paid"
                    );
                }
                assert!(!lua.table_get(t, key).is_nil());
                let (calls, total) = (used.len() as u64, used.iter().sum::<u64>());
                assert!(
                    (whole..=whole + calls * SLACK).contains(&total),
                    "2^{bits} seed {seed} fast {fast}: {total} in {used:?} for {whole}"
                );
                // The census and the scan, each part once, and a piece of each a call.
                let read = lua.table_slots_watched();
                assert!(
                    read <= 2 * slots + calls * 2 * 512,
                    "2^{bits} seed {seed} fast {fast}: {read} slots read of {slots}"
                );
                match &seen {
                    None => seen = Some(used.clone()),
                    Some(s) => assert_eq!(s, &used, "2^{bits} seed {seed} fast {fast}: the calls' fuel"),
                }
            }
        }
    }
    // Thrown away by a write between calls (a key removed), what was made is not refunded and
    // the growth is made again from the start, charged whole as new work.
    for fast in [true, false] {
        let fresh = |removed: bool| {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(1), fast);
            lua.exec("collectgarbage('stop') T = {} for i = 1, 1 << 18 do T['k' .. i] = i end", "=setup")
                .unwrap();
            if removed {
                lua.exec("T.k1 = nil", "=remove").unwrap();
            }
            let f = lua.load("T.new = 1", "=add").unwrap();
            (lua, f)
        };
        let (mut lua, f) = fresh(true);
        lua.call(f.value(), &[]).unwrap();
        let again = (1u64 << 40) - lua.fuel_left();
        let (mut lua, f) = fresh(false);
        lua.set_limits(lua.limits().fuel(FUEL));
        assert!(lua.call(f.value(), &[]).is_err());
        assert!(lua.fuel_left() < SLACK, "fast {fast}: the first call left {}", lua.fuel_left());
        lua.set_limits(lua.limits().fuel(1 << 40));
        lua.exec("T.k1 = nil", "=remove").unwrap();
        lua.set_limits(lua.limits().fuel(FUEL));
        let mut used = Vec::new();
        while lua.call(f.value(), &[]).is_err() {
            used.push(FUEL);
            assert!((used.len() as u64) < again / FUEL + 2, "fast {fast}: not in after {used:?} of {again}");
        }
        used.push(FUEL - lua.fuel_left());
        let (calls, total) = (used.len() as u64, used.iter().sum::<u64>());
        assert!(
            (again..=again + calls * SLACK).contains(&total),
            "fast {fast}: made again for {total} in {used:?}, whole {again}"
        );
    }
}

/// SEC8-F4, the nodes build: a full `nodes` grown for a new key (`grow_nodes`, a table not
/// counting) was refused whole the same way. Made a piece at a time within each call's
/// fuel now (the new part made and the keys placed in pieces of 512), kept between calls
/// and charged once: in after a bounded number of calls, their fuel the growth's and a
/// piece a call at most, the same under every seed and on both paths; the old part placed
/// once; and while it is kept the new part is counted whole beside the old one (SEC7-T3).
#[test]
fn sec8_f4_a_nodes_growth_bigger_than_a_call_is_made_across_calls() {
    const FUEL: u64 = 20_000;
    // (The write's own steps and a piece of placements past what the call could pay.)
    const SLACK: u64 = 100;
    let setup = "collectgarbage('stop') T = {} for i = 1, 3 << 15 do T[i + 0.5] = i end";
    let mut seen = None;
    for seed in [1u64, 7, 0xdead_beef] {
        for fast in [true, false] {
            let fresh = || {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec(setup, "=setup").unwrap();
                let f = lua.load("T[-0.5] = 1", "=add").unwrap();
                (lua, f)
            };
            let (mut lua, f) = fresh();
            lua.call(f.value(), &[]).unwrap();
            let (whole, landed) = ((1u64 << 40) - lua.fuel_left(), lua.memory_used());
            assert!(whole > FUEL, "seed {seed} fast {fast}: the growth costs {whole}");
            let (mut lua, f) = fresh();
            let Ok(remedy_lua::Value::Table(t)) = lua.get_global("T") else { panic!("T") };
            lua.set_limits(lua.limits().fuel(FUEL));
            let _ = lua.table_slots_watched();
            let mut used = Vec::new();
            loop {
                assert!(
                    (used.len() as u64) < whole / FUEL + 2,
                    "seed {seed} fast {fast}: not in after {used:?} of {whole}"
                );
                let r = lua.call(f.value(), &[]);
                used.push(FUEL - lua.fuel_left());
                if r.is_ok() {
                    break;
                }
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
                assert!(
                    lua.table_get(t, remedy_lua::Value::Num(-0.5)).is_nil(),
                    "seed {seed} fast {fast}: in before the growth was paid"
                );
                assert!(
                    lua.memory_used() >= landed,
                    "seed {seed} fast {fast}: {} counted while kept, {landed} once in",
                    lua.memory_used()
                );
            }
            let (calls, total) = (used.len() as u64, used.iter().sum::<u64>());
            assert!(
                (whole..=whole + calls * SLACK).contains(&total),
                "seed {seed} fast {fast}: {total} in {used:?} for {whole}"
            );
            // The old part's keys placed once, and a piece a call.
            let read = lua.table_slots_watched();
            assert!(read <= (4u64 << 15) + calls * 512, "seed {seed} fast {fast}: {read} slots placed");
            match &seen {
                None => seen = Some(used.clone()),
                Some(s) => assert_eq!(s, &used, "seed {seed} fast {fast}: the calls' fuel"),
            }
        }
    }
}

/// SEC8-F4, the key move: a string key into a table of integer keys in their own part
/// (`to_counting`) moved them all into `nodes` in one go, refused whole the same way. With
/// the move needing a new `nodes`, it is made under the fuel watch now: the integer keys
/// placed into the new part after the old one's, a piece at a time within each call's
/// fuel, the table as it was until the new part is in, and charged once: in after a
/// bounded number of calls, their fuel the growth's and a piece a call at most, the same
/// under every seed and on both paths, each key placed once.
#[test]
fn sec8_f4_a_key_move_bigger_than_a_call_is_made_across_calls() {
    const FUEL: u64 = 20_000;
    const SLACK: u64 = 100;
    let setup = "collectgarbage('stop') T = {} for i = 1, 1 << 15 do T[i * 7] = i end";
    let mut seen = None;
    for seed in [1u64, 7, 0xdead_beef] {
        for fast in [true, false] {
            let fresh = || {
                let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
                lua.exec(setup, "=setup").unwrap();
                let f = lua.load("T.s = 1", "=add").unwrap();
                (lua, f)
            };
            let (mut lua, f) = fresh();
            lua.call(f.value(), &[]).unwrap();
            let whole = (1u64 << 40) - lua.fuel_left();
            assert!(whole > FUEL, "seed {seed} fast {fast}: the move costs {whole}");
            let (mut lua, f) = fresh();
            let Ok(remedy_lua::Value::Table(t)) = lua.get_global("T") else { panic!("T") };
            let key = lua.create_string("s").unwrap();
            lua.set_limits(lua.limits().fuel(FUEL));
            let _ = lua.table_slots_watched();
            let mut used = Vec::new();
            loop {
                assert!(
                    (used.len() as u64) < whole / FUEL + 2,
                    "seed {seed} fast {fast}: not in after {used:?} of {whole}"
                );
                let r = lua.call(f.value(), &[]);
                used.push(FUEL - lua.fuel_left());
                if r.is_ok() {
                    break;
                }
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
                assert!(
                    lua.table_get(t, key).is_nil(),
                    "seed {seed} fast {fast}: in before the move was paid"
                );
                assert_eq!(
                    lua.table_get(t, remedy_lua::Value::Int(7)),
                    remedy_lua::Value::Int(1),
                    "seed {seed} fast {fast}: a key moved early"
                );
            }
            let (calls, total) = (used.len() as u64, used.iter().sum::<u64>());
            assert!(
                (whole..=whole + calls * SLACK).contains(&total),
                "seed {seed} fast {fast}: {total} in {used:?} for {whole}"
            );
            // Each integer key placed once, and the census of the rehash after it once, a piece
            // a call. (At four units a slot the rehash after the move is watched; its count
            // gets one fixed allowance of 65,536 slots across the whole run, and retrying does
            // not grant another full pass.)
            let read = lua.table_slots_watched();
            assert!(read <= (2u64 << 16) + calls * 512, "seed {seed} fast {fast}: {read} slots read");
            match &seen {
                None => seen = Some(used.clone()),
                Some(s) => assert_eq!(s, &used, "seed {seed} fast {fast}: the calls' fuel"),
            }
        }
    }
}

/// SEC8-F4, R1: a growth while the collector marks is charged the
/// marking of its new slots once it is in (SEC7-G1, `take_resized`), whole: an integer
/// array of 2^22 grown by one key while marking (1,835,011 fuel uninterrupted, against
/// 786,435 not marking) never went in at 20,000 a call. That marking is paid ahead now,
/// a call's fuel at a time with the rest: in after a bounded number of calls, their fuel
/// the growth's whole cost and a piece a call at most; and at 2^20 the calls' fuel the same
/// under every seed and on both paths.
#[test]
fn sec8_f4_a_growth_while_the_collector_marks_is_made_across_calls() {
    const FUEL: u64 = 20_000;
    const SLACK: u64 = 100;
    let mut seen = None;
    for (bits, seed, fast) in [
        (22, 1u64, true),
        (20, 1, true),
        (20, 1, false),
        (20, 7, true),
        (20, 7, false),
        (20, 0xdead_beef, true),
        (20, 0xdead_beef, false),
    ] {
        let fresh = || {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
            lua.exec(&format!("T = {{}} for i = 1, 1 << {bits} do T[i] = i end collectgarbage() collectgarbage('step', 1)"), "=setup").unwrap();
            let f = lua.load(format!("T[(1 << {bits}) + 1] = 1"), "=add").unwrap();
            (lua, f)
        };
        let (mut lua, f) = fresh();
        lua.call(f.value(), &[]).unwrap();
        let whole = (1u64 << 40) - lua.fuel_left();
        let (mut lua, f) = fresh();
        let Ok(remedy_lua::Value::Table(t)) = lua.get_global("T") else { panic!("T") };
        lua.set_limits(lua.limits().fuel(FUEL));
        let mut used = Vec::new();
        loop {
            assert!(
                (used.len() as u64) < whole / FUEL + 2,
                "2^{bits} seed {seed} fast {fast}: not in after {} calls of {whole}",
                used.len()
            );
            let r = lua.call(f.value(), &[]);
            used.push(FUEL - lua.fuel_left());
            if r.is_ok() {
                break;
            }
            assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
        }
        assert_eq!(lua.table_get(t, remedy_lua::Value::Int((1 << bits) + 1)), remedy_lua::Value::Int(1));
        let (calls, total) = (used.len() as u64, used.iter().sum::<u64>());
        assert!(
            (whole..=whole + calls * SLACK).contains(&total),
            "2^{bits} seed {seed} fast {fast}: {total} in {calls} calls for {whole}"
        );
        if bits == 20 {
            match &seen {
                None => seen = Some(used.clone()),
                Some(s) => assert_eq!(s, &used, "seed {seed} fast {fast}: the calls' fuel"),
            }
        }
    }
}

/// SEC8-F4, retention: a resize given up and then thrown away by a write
/// (`RESUMABLE` cleared) kept what it made, counted, until the table's next resize found it
/// stale: an array growth from 2^17 stopped by the clock in its copy held its new part past
/// the deletion that ended it. It is released at that write now.
#[test]
fn sec8_f4_a_resize_thrown_away_by_a_write_is_released_there() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    for fast in [true, false] {
        let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
        lua.exec("T = {} for i = 1, 1 << 17 do T[i] = i end", "=setup").unwrap();
        let f = lua.load("T[(1 << 17) + 1] = 1", "=grow").unwrap();
        let before = lua.memory_used();
        let reads = Arc::new(AtomicUsize::new(0));
        let r = Arc::clone(&reads);
        lua.set_time_limit(Some((
            Arc::new(move || {
                std::time::Duration::from_secs(if r.fetch_add(1, Relaxed) + 1 >= 3 { 7200 } else { 0 })
            }),
            std::time::Duration::from_secs(3600),
        )));
        let mut held = 0;
        for _ in 0..50 {
            reads.store(0, Relaxed);
            assert!(lua.call(f.value(), &[]).is_err(), "fast {fast}: in before its copy was stopped");
            held = lua.memory_used().saturating_sub(before);
            if held > 1 << 20 {
                break;
            }
        }
        assert!(held > 1 << 20, "fast {fast}: no copy held ({held} bytes)");
        lua.set_time_limit(None);
        lua.exec("T[5] = nil", "=delete").unwrap();
        let after = lua.memory_used().saturating_sub(before);
        assert!(after < 1 << 16, "fast {fast}: {after} bytes held after the write ({held} before it)");
        let ok = lua.exec("T[(1 << 17) + 1] = 1 for i = 1, (1 << 17) + 1 do local want = i if i == 5 then want = nil elseif i > 1 << 17 then want = 1 end if T[i] ~= want then return false end end return true", "=check").unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}");
    }
}

/// SEC8-F4: a rehash made across calls that ends while the collector
/// traverses its table in chunks (`heap.part`, S6GC-2e): every value is still there after
/// the cycle, on both paths and three seeds. (Its calls step the collector by the least:
/// the rehash lands within T's traversal, in 4 calls.)
#[test]
fn sec8_f4_a_partial_ending_while_its_table_is_traversed_in_chunks() {
    for (fast, seed) in
        [(true, 1u64), (false, 1), (true, 7), (false, 7), (true, 0xdead_beef), (false, 0xdead_beef)]
    {
        let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
        lua.exec(
            "T = {} for i = 1, 1 << 17 do T['k' .. i] = {i} end collectgarbage() collectgarbage('step', 1)",
            "=setup",
        )
        .unwrap();
        let f = lua.load("collectgarbage('step', 0) T.extra = {0}", "=grow").unwrap();
        lua.set_limits(lua.limits().fuel(20_000));
        let mut calls = 0;
        while lua.call(f.value(), &[]).is_err() {
            calls += 1;
            assert!(calls < 200, "seed {seed} fast {fast}: not in");
        }
        assert!(calls > 1, "seed {seed} fast {fast}: in in one call");
        lua.set_limits(lua.limits().fuel(1 << 40));
        let ok = lua
            .exec("collectgarbage() collectgarbage() if T.extra[1] ~= 0 then return false end for i = 1, 1 << 17 do local v = T['k' .. i] if type(v) ~= 'table' or v[1] ~= i then return false end end return true", "=check")
            .unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "seed {seed} fast {fast}");
    }
}

/// SEC8-F4, R5: a call's first piece of a growth under a fuel watch is
/// made before its first look, so under a call's fuel below a piece's cost (512 slots: 96
/// fuel scanning 2^17 nodes, about 2,300 placing 2^21 slots) it went past the call's fuel,
/// counted paid but charged only the call's fuel: a rehash of 2^21 string keys (1,048,579
/// fuel) went in for 49,155 at 3 fuel a call. A stop now counts paid what it charged and no
/// more: the total is the growth's cost, and the chunk's own instructions again each call.
#[test]
fn sec8_f4_a_growth_under_a_piece_a_call_is_charged_whole() {
    const FUEL: u64 = 40;
    for fast in [true, false] {
        let fresh = || {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
            lua.exec(
                "T = {} for i = 1, 1 << 16 do T['k' .. i] = i end collectgarbage() collectgarbage('stop')",
                "=setup",
            )
            .unwrap();
            let f = lua.load("T.extra = 1", "=add").unwrap();
            (lua, f)
        };
        let (mut lua, f) = fresh();
        lua.call(f.value(), &[]).unwrap();
        let whole = (1u64 << 40) - lua.fuel_left();
        let (mut lua, f) = fresh();
        lua.set_limits(lua.limits().fuel(FUEL));
        let (mut calls, mut total) = (0u64, 0u64);
        loop {
            calls += 1;
            assert!(calls < 2 * whole, "fast {fast}: not in after {calls} calls of {whole}");
            let r = lua.call(f.value(), &[]);
            total += FUEL - lua.fuel_left();
            if r.is_ok() {
                break;
            }
        }
        // (Each call's instructions before the write, 3 fuel at most, again in each.)
        assert!(
            (whole..=whole + calls * 3).contains(&total),
            "fast {fast}: {total} in {calls} calls for {whole}"
        );
        lua.set_limits(lua.limits().fuel(1 << 40));
        let ok = lua.exec("if T.extra ~= 1 then return false end for i = 1, 1 << 16 do if T['k' .. i] ~= i then return false end end return true", "=check").unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}");
    }
}

/// SEC8-F4, R1 across cycles: the marking of a growth paid ahead in one
/// cycle (c), the growth in while the next (c + 1) marks. The calls' fuel is at most the
/// growth's whole cost while marking (the same table, the same marking in either cycle) and
/// a piece a call: the marking is not paid twice.
#[test]
fn sec8_f4_a_growth_paid_ahead_in_one_cycle_lands_in_the_next() {
    const FUEL: u64 = 20_000;
    for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (false, 7)] {
        let fresh = || {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(seed), fast);
            lua.exec(
                "T = {} for i = 1, 1 << 20 do T[i] = i end collectgarbage() collectgarbage('step', 1)",
                "=setup",
            )
            .unwrap();
            let f = lua.load("T[(1 << 20) + 1] = 1", "=add").unwrap();
            (lua, f)
        };
        let (mut lua, f) = fresh();
        lua.call(f.value(), &[]).unwrap();
        let whole = (1u64 << 40) - lua.fuel_left();
        let (mut lua, f) = fresh();
        let mut used = Vec::new();
        let mut call = |lua: &mut Lua| {
            lua.set_limits(lua.limits().fuel(FUEL));
            let r = lua.call(f.value(), &[]);
            used.push(FUEL - lua.fuel_left());
            r.is_ok()
        };
        // Cycle c: all but its last 2 calls' worth (the passes and the marking paid ahead).
        for _ in 0..whole / FUEL - 2 {
            assert!(!call(&mut lua), "seed {seed} fast {fast}: in early, of {whole}");
        }
        // Cycle c ends, c + 1 starts and marks (the host's fuel, not the calls').
        lua.set_limits(lua.limits().fuel(1 << 40));
        lua.exec("collectgarbage() collectgarbage('step', 1)", "=cycle").unwrap();
        let mut n = 0;
        while !call(&mut lua) {
            n += 1;
            assert!(n < 100, "seed {seed} fast {fast}: not in");
        }
        let total: u64 = used.iter().sum();
        assert!(
            total <= whole + used.len() as u64 * 100,
            "seed {seed} fast {fast}: {total} in {} calls for {whole}",
            used.len()
        );
        lua.set_limits(lua.limits().fuel(1 << 40));
        let ok = lua
            .exec(
                "for i = 1, 1 << 20 do if T[i] ~= i then return false end end return T[(1 << 20) + 1] == 1",
                "=check",
            )
            .unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "seed {seed} fast {fast}");
    }
}

/// SEC8-F6: a resize to other sizes after one given up drops the new tail that one left past
/// the array part. Segmented, that frees its blocks, but their bytes stayed counted, and the
/// new part was charged again over them, until the table died: 1,310,720 bytes over at 2^17.
/// Counted as the same writes unstopped now, on both storages.
#[test]
fn sec8_f6_a_dropped_tail_is_not_counted() {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    for fast in [true, false] {
        let mut used = Vec::new();
        for stop in [false, true] {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40), fast);
            lua.exec("T = {} for i = 1, 1 << 17 do T[i] = i end", "=setup").unwrap();
            // (Loaded in both: its function counted alike.)
            let f = lua.load("T[(1 << 17) + 1] = 1", "=grow").unwrap();
            if stop {
                let reads = Arc::new(AtomicUsize::new(0));
                let r = Arc::clone(&reads);
                lua.set_time_limit(Some((
                    Arc::new(move || {
                        std::time::Duration::from_secs(if r.fetch_add(1, Relaxed) + 1 >= 3 {
                            7200
                        } else {
                            0
                        })
                    }),
                    std::time::Duration::from_secs(3600),
                )));
                for _ in 0..5 {
                    reads.store(0, Relaxed);
                    assert!(lua.call(f.value(), &[]).is_err(), "fast {fast}: in before its stops");
                }
                lua.set_time_limit(None);
            }
            // (A string key: the resize is to other sizes than the one given up.)
            lua.exec("T.s = 1 T[(1 << 17) + 1] = 1 collectgarbage() collectgarbage()", "=rest").unwrap();
            let ok = lua.exec("for i = 1, (1 << 17) + 1 do if T[i] ~= (i <= 1 << 17 and i or 1) then return false end end return T.s == 1", "=check").unwrap();
            assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}, stop {stop}");
            used.push(lua.memory_used());
        }
        assert_eq!(used[1], used[0], "fast {fast}: counted after a given-up resize, against unstopped");
    }
}

/// SEC8-F4, the gate runner's case 1 (SEC7-T4's key move, 2^21 integer keys at `i * 3`, then
/// `T.s = 1`): it never went in at 1,000, 2,000 or 20,000 a call, each call spending its fuel
/// for nothing. A stop before a stage's first piece (the marking paid ahead past what the
/// call had left; at 1,000 a call, the first piece placing 2^22 nodes) charged no more than
/// the attempts before had paid, and `resumed` took it for a refusal: nothing kept paid, the
/// same stop in every call. Such a stop is paid ahead with the call's fuel now: in after a
/// bounded number of calls, their fuel the move's whole cost and each call's instructions
/// again, the same on both loops.
#[test]
fn sec8_f4_a_key_move_whose_pieces_are_past_a_call_is_made_across_calls() {
    for fuel in [1_000u64, 20_000] {
        let mut seen = None;
        for fast in [true, false] {
            let fresh = || {
                let mut lua = state(Limits::default().memory(1 << 31).fuel(1 << 40).hash_seed(1), fast);
                lua.exec("T = {} for i = 1, 1 << 21 do T[i * 3] = i end", "=setup").unwrap();
                let f = lua.load("T.s = 1", "=add").unwrap();
                (lua, f)
            };
            let (mut lua, f) = fresh();
            lua.call(f.value(), &[]).unwrap();
            let whole = (1u64 << 40) - lua.fuel_left();
            assert!(whole > 100 * fuel, "fast {fast}: the move costs {whole}");
            let (mut lua, f) = fresh();
            lua.set_limits(lua.limits().fuel(fuel));
            let (mut calls, mut total) = (0u64, 0u64);
            loop {
                calls += 1;
                assert!(
                    calls <= 2 * whole / fuel + 20,
                    "{fuel} a call, fast {fast}: not in after {calls} calls, {total} paid of {whole}"
                );
                let r = lua.call(f.value(), &[]);
                total += fuel - lua.fuel_left();
                if r.is_ok() {
                    break;
                }
                assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{r:?}");
            }
            assert!(
                (whole..=whole + 100 * calls).contains(&total),
                "{fuel} a call, fast {fast}: {total} in {calls} calls for {whole}"
            );
            lua.set_limits(lua.limits().fuel(1 << 40));
            let ok = lua.exec("if T.s ~= 1 then return false end for i = 1, 1 << 21 do if T[i * 3] ~= i then return false end end return true", "=check").unwrap();
            assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "{fuel} a call, fast {fast}");
            let got = (whole, calls, total);
            assert_eq!(*seen.get_or_insert(got), got, "{fuel} a call, fast {fast}: by loop");
        }
    }
}

/// SEC8-F4, the marking paid ahead alone (the lead's request): a key move of 2^17 integer
/// keys at `i * 3` while a cycle marks, at 20,000 a call. Its count goes through in a call;
/// then the marking of its growth, paid ahead, is past what any call has left. That stop
/// charged no more than the attempt before had paid, and was taken for a refusal: from the
/// second call on, the same stop every call, never in (2,000 calls, 40,000,000 fuel for a
/// move of 499,715). Paid ahead with the call's fuel now: in, the calls' fuel the move's
/// whole cost and the chunk's instructions again each call, the same on both loops.
#[test]
fn sec8_f4_a_marking_paid_ahead_past_a_call_is_paid_across_calls() {
    const FUEL: u64 = 20_000;
    let setup = "T = {} for i = 1, 1 << 17 do T[i * 3] = i end collectgarbage() collectgarbage('step', 1)";
    let mut seen = None;
    for fast in [true, false] {
        let fresh = || {
            let mut lua = state(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(1), fast);
            lua.exec(setup, "=setup").unwrap();
            let f = lua.load("T.s = 1", "=add").unwrap();
            (lua, f)
        };
        let (mut lua, f) = fresh();
        lua.call(f.value(), &[]).unwrap();
        let whole = (1u64 << 40) - lua.fuel_left();
        let (mut lua, f) = fresh();
        lua.set_limits(lua.limits().fuel(FUEL));
        let (mut calls, mut total) = (0u64, 0u64);
        loop {
            calls += 1;
            assert!(
                calls <= 2 * whole / FUEL + 20,
                "fast {fast}: not in after {calls} calls, {total} paid of {whole}"
            );
            let r = lua.call(f.value(), &[]);
            total += FUEL - lua.fuel_left();
            if r.is_ok() {
                break;
            }
        }
        assert!(
            (whole..=whole + 100 * calls).contains(&total),
            "fast {fast}: {total} in {calls} calls for {whole}"
        );
        lua.set_limits(lua.limits().fuel(1 << 40));
        let ok = lua.exec("if T.s ~= 1 then return false end for i = 1, 1 << 17 do if T[i * 3] ~= i then return false end end return true", "=check").unwrap();
        assert_eq!(ok.first(), Some(&remedy_lua::Value::Bool(true)), "fast {fast}");
        let got = (whole, calls, total);
        assert_eq!(*seen.get_or_insert(got), got, "fast {fast}: by loop");
    }
}
