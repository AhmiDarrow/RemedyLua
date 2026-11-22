//! Sweep 6, tables and the interner: regression tests named `s6t_<n>_<what>` after their
//! findings (S6T-2, S6T-3, the S5B-OPEN-2 and rehash residuals, the interner's regrow),
//! each run with the fast loop on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Error, LimitKind, Libs, Limits, Lua};

use super::common::{TEST_SEED, small_stack};
use super::sweep5b_open::{aimed_floats, baseline, list, measured, ns_per_unit, table_seeds};

/// The best of three ratios of `body`'s cost per unit of fuel to plain code's.
fn ratio(setup: &str, body: &str, fuel: u64, fast: bool) -> f64 {
    (0..3).map(|_| ns_per_unit(setup, body, fuel, fast) / baseline(fast)).fold(f64::MAX, f64::min)
}

/// Fuel, result and memory of `src` are the same under every seed and with the fast loop
/// on and off.
fn seed_independent(src: &str) {
    let first = measured(src, TEST_SEED, true);
    println!("fuel {}", first.1);
    for seed in [TEST_SEED, 0, 1, 2, 3, 0xdead_beef, u64::MAX, 0x1234_5678_9abc_def0, 77] {
        for fast in [true, false] {
            assert_eq!(measured(src, seed, fast), first, "seed {seed:#x} fast {fast}:\n{src}");
        }
    }
}

/// S6T-2 (medium): `next` over the hash part charged a unit per 16 slots of a gap past 64
/// empty slots, and where the gaps fall follows the hash seed: a traversal of a hash part
/// with most of its keys removed used 30,150 to 30,164 fuel across seeds. The part now
/// keeps a bitmap of the nodes that may hold a value (set by every write that gives a node
/// one, cleared by `next` when it finds the value gone), so a gap costs a word per 64
/// slots and is free; the fuel is the program's (30,042 under every seed at dbed73c, the bitmap's
/// bytes charged with the part).
#[test]
fn s6t_2_next_fuel_does_not_follow_the_seed() {
    small_stack(|| {
        seed_independent(
            "local t = {} for i = 1, 3000 do t[i + 0.5] = i end \
             for i = 1, 3000 do if i % 50 ~= 0 then t[i + 0.5] = nil end end \
             local c = 0 for k, v in pairs(t) do c = c + v end return c",
        );
        // Partial traversals, a key revived after its value went (found again by later
        // traversals), clearing while traversing, and string keys beside the integer part.
        seed_independent(
            "local t = {} for i = 1, 20000 do t['k' .. i] = i end \
             for i = 1, 20000 do if i % 97 ~= 0 then t['k' .. i] = nil end end \
             local c, n = 0, 0 for r = 1, 30 do local k, v = next(t) \
               for j = 1, r do if k == nil then break end c = c + 1 k, v = next(t, k) end end \
             t.k5 = 5 t.k7 = 7 for k, v in pairs(t) do n = n + 1 if v % 2 == 0 then t[k] = nil end end \
             local m = 0 for k in pairs(t) do m = m + 1 end \
             local u = {x = 1} for i = 1, 5000 do u[i * 7] = i end for i = 1, 5000 do if i % 9 ~= 0 then u[i * 7] = nil end end \
             local s = 0 for k, v in pairs(u) do s = s + v end return c, n, m, s",
        );
    });
}

/// S6T-2: what `next` does now costs about what plain code does per unit of fuel: whole
/// and partial traversals of a big hash part with one key in 1,000 left, with clearing
/// and reviving keys between them.
#[test]
fn s6t_2_sparse_traversals_are_paid() {
    small_stack(|| {
        let setup = "t = {} K = {} for i = 1, 200000 do t[i + 0.5] = i end \
                     for i = 1, 200000 do if i % 1000 ~= 0 then t[i + 0.5] = nil end end \
                     for i = 1, 1000 do K[i] = i * 200 + 0.5 end";
        let bodies = [
            "for k, v in pairs(t) do end",
            "local k = next(t) for i = 1, 5 do k = next(t, k) end",
            "for i = 1, 1000 do t[K[i]] = nil end local k = next(t) for i = 1, 1000 do t[K[i]] = i end",
        ];
        for fast in [true, false] {
            for body in bodies {
                let r = ratio(setup, body, 2_000_000, fast);
                println!("fast {fast}: {r:.1}x  {body}");
                assert!(r < 3.0, "fast {fast}: {r:.1}x plain code per unit: {body}");
            }
        }
    });
}

/// A table `t` of 1.5 million plain float keys (2^21 slots) and `R`: `rounds` lists of 11
/// keys aimed at both homes of one pair under the seed the table has at that round (each
/// round's keys reseed it).
fn reseed_storm(rounds: usize) -> String {
    storm(21, 1_500_000, rounds)
}

/// `reseed_storm` in a part of 2^`bits` slots holding `n` plain keys.
fn storm(bits: u32, n: usize, rounds: usize) -> String {
    let seeds = table_seeds(TEST_SEED, rounds);
    let lists: Vec<String> = seeds.iter().map(|&s| format!("{{{}}}", list(&aimed_floats(&[s], bits, 12345 % (1 << (bits - 1)), 11)))).collect();
    format!(
        "t = {{}} for i = 1, {n} do t[i + 0.25] = i end R = {{{}}} J = 0 collectgarbage() collectgarbage('stop')",
        lists.join(",")
    )
}

/// Probe (not a test): reseeds and rehashes at several sizes, as ratios to plain code.
/// `cargo test -p remedy-lua --test all sweep6_tables::probe_sizes -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_sizes() {
    small_stack(|| {
        let fast = std::env::var("FAST").map_or(true, |v| v != "0");
        for bits in [14u32, 16, 18, 19, 20, 21] {
            let n = (7usize << bits) / 10;
            let setup = storm(bits, n, 400);
            let body = "J = J + 1 local a = R[J] for i = 1, 11 do t[a[i]] = i end for i = 1, 11 do t[a[i]] = nil end";
            let fuel = (n as u64).max(1 << 16) * 18;
            println!("reseed 2^{bits}: {:.1}x", ratio(&setup, body, fuel, fast));
        }
        for bits in [14u32, 16, 18, 19, 20] {
            let setup = format!("t = {{}} for i = 1, (1 << {bits}) - 1 do t[i * 3] = i end J = 0 collectgarbage() collectgarbage('stop')");
            let fuel = (1u64 << bits) * 3;
            println!("rehash 2^{bits}: {:.1}x", ratio(&setup, "J = J + 1 t[-J] = 1 t[-J] = nil", fuel, fast));
        }
    });
}

/// S6T-3 (medium, a known seed): 11 keys aimed at one pair of homes reseed the part, and
/// the reseed was charged `PLACE_COST` (half a unit) per key placed, but placing a key in
/// a 64 MB part is a cache miss or two: a round of 11 aimed keys in and out of a table of
/// 1.5 million keys ran at 8 to 11x plain code per unit of fuel (6.0x with the fast loop
/// here, 3.8x without; 3.1x at 2^18 slots, 2.4x at 2^16). A placement in a part of 2^16,
/// 2^18 or 2^20 slots or more is charged 2, 4 or 12 times as much now (the new part too) (`table::place_cost`,
/// by the part's size, which the seed never changes): 1.7 to 1.8x and 0.8 to 1.1x at 2^21,
/// at most 1.8x at every size from 2^14 (`probe_sizes`).
#[test]
fn s6t_3_reseeding_a_big_part_is_paid() {
    small_stack(|| {
        let setup = reseed_storm(40);
        let body = "J = J + 1 local a = R[J] for i = 1, 11 do t[a[i]] = i end for i = 1, 11 do t[a[i]] = nil end";
        for fast in [true, false] {
            let r = ratio(&setup, body, 25_000_000, fast);
            println!("fast {fast}: {r:.1}x");
            assert!(r < 3.0, "fast {fast}: a reseed storm at {r:.1}x plain code per unit");
        }
    });
}

/// The rehash residual (S5B-COST-2): a table of a million integer keys one short of a power
/// of two rehashes at every new key (as C Lua's does), and each rehash first listed every
/// key it moved in a vector grown by doubling (32 bytes a key, mapped afresh each time):
/// 3.4 to 3.6x plain code per unit of fuel with the fast loop here (3.7 to 3.9x by
/// `sweep5b_cost::cost_probe`, 4.5 to 4.9x elsewhere), a rehash taking 53 ms against C
/// Lua's 21 ms. The keys are placed straight from the old parts now: 1.9 to 2.2x (1.1x
/// without the fast loop; 2.4x by `cost_probe`), 34 ms a rehash. On a machine with slower
/// memory that was still 3.0x: the placements are charged by the part's size now
/// (`table::int_place_cost`), 1.3x here (0.8x without the fast loop).
#[test]
fn s6t_rehash_at_a_million_keys_is_paid() {
    small_stack(|| {
        let setup = "t = {} for i = 1, (1 << 20) - 1 do t[i * 3] = i end J = 0 collectgarbage() collectgarbage('stop')";
        let body = "J = J + 1 t[-J] = 1 t[-J] = nil";
        for fast in [true, false] {
            let r = ratio(setup, body, 3_000_000, fast);
            println!("fast {fast}: {r:.1}x");
            assert!(r < 1.9, "fast {fast}: rehash at a million keys at {r:.1}x plain code per unit");
        }
    });
}

/// The interner's regrow (low): it placed every interned string again (a cache miss or two
/// each once the interner is big) and charged only walks past the bound, so making strings
/// that die beside two million live ones ran at 2.7x plain code per unit of fuel with the
/// fast loop and 3.2x without (`sweep5b_cost::cost_probe`; 2.2 to 2.7x beside a million and
/// 3.2 to 3.9x beside three million elsewhere), a new string costing the same fuel (4.0)
/// beside 1.2 million live strings as beside a thousand (4.6). Regrows follow the seed, so
/// each new string now pays its share by the interner's count (`heap::regrow_share`: 2 at
/// 2^18 strings, 4 at 2^20): 8.0 beside 1.2 million, the same under every seed; 1.8x and
/// 2.1x by `cost_probe`, 1.5x and 1.0x here (1.8 to 2.3x and 1.5x before).
#[test]
fn s6t_intern_new_strings_pay_for_regrows() {
    small_stack(|| {
        let src = |live: usize, n: usize| {
            format!(
                "local L = {{}} for i = 1, {live} do L[i] = 'live' .. i end collectgarbage() \
                 for i = 1, {n} do local s = 'g' .. i end return #L"
            )
        };
        let per_string = |live: usize| {
            let (a, b) = (measured(&src(live, 0), TEST_SEED, true), measured(&src(live, 300_000), TEST_SEED, true));
            (b.1 - a.1) as f64 / 300_000.0
        };
        let (small, big) = (per_string(1000), per_string(1_200_000));
        println!("a new string: {small:.2} fuel beside a thousand live strings, {big:.2} beside 1.2 million");
        assert!(big >= small + 3.0, "a new string beside 1.2 million live ones costs {big:.2}, beside a thousand {small:.2}");
        // The share follows the count only: the same fuel under every seed, with regrows on
        // the way (and strings dying, whose slots new ones take).
        seed_independent(&src(300_000, 600_000));
        let setup = "L = {} for i = 1, 2000000 do L[i] = 'live' .. i end J = 0 collectgarbage()";
        let body = "J = J + 1 local s = 'g' .. J";
        for fast in [true, false] {
            let r = ratio(setup, body, 20_000_000, fast);
            println!("fast {fast}: {r:.1}x");
            assert!(r < 2.5, "fast {fast}: garbage strings beside two million live ones at {r:.1}x plain code per unit");
        }
    });
}

/// What one run of `regrow_watch` saw: the deadline reads from the first sight of the
/// interner's slots for 2M strings moving to the time all had moved, how often the probe
/// saw them moving, all the deadline reads, and the fuel.
#[derive(Debug, PartialEq, Eq)]
struct RegrowWatch {
    reads_while_moving: u64,
    probes_while_moving: u64,
    reads: u64,
    fuel: u64,
}

/// Makes 2M strings that stay live under a clock that counts its reads (each a look at the
/// deadline, as at the host's interrupt), with `probe()` every 1,024 of them noting whether the
/// interner's slots are moving to the 2^22 it grows to at 1.6 million.
fn regrow_watch(fast: bool) -> RegrowWatch {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    let size = |lua: &Lua, key: &str| {
        lua.internal_sizes().into_iter().find(|(k, _)| *k == key).map(|(_, v)| v).unwrap()
    };
    let mut lua = Lua::new(Limits::default().memory(2 << 30).fuel(1 << 40).hash_seed(TEST_SEED), Libs::ALL);
    lua.set_fast_loop(fast);
    // The table's array part grows first: the interner's work alone is watched.
    lua.exec("L = {} for i = 1, 2000000 do L[i] = true end", "=setup").unwrap();
    let reads = Arc::new(AtomicU64::new(0));
    let r = Arc::clone(&reads);
    let clock: remedy_lua::Clock =
        Arc::new(move || std::time::Duration::from_nanos(r.fetch_add(1, Ordering::Relaxed)));
    lua.set_time_limit(Some((clock, std::time::Duration::from_secs(1 << 40))));
    // (first read seen moving, reads when done, probes seen moving)
    let seen = Arc::new(Mutex::new((None::<u64>, None::<u64>, 0u64)));
    let (s, r) = (Arc::clone(&seen), Arc::clone(&reads));
    lua.register("probe", move |lua, _| {
        let (moving, slots) = (size(lua, "interner_moving"), size(lua, "interner_slots"));
        let now = r.load(Ordering::Relaxed);
        let mut s = s.lock().unwrap();
        if slots == 1 << 22 && moving > 0 {
            s.0.get_or_insert(now);
            s.2 += 1;
        } else if s.0.is_some() && moving == 0 {
            s.1.get_or_insert(now);
        }
        Ok(vec![])
    })
    .unwrap();
    let r = lua
        .exec(
            "for i = 1, 2000000 do L[i] = 'r' .. i if i % 1024 == 0 then probe() end end return L[2000000]",
            "=t",
        )
        .unwrap();
    assert_eq!(super::common::show(&lua, r[0]), "r2000000");
    let (first, done, probes) = *seen.lock().unwrap();
    let (Some(first), Some(done)) = (first, done) else {
        panic!("fast {fast}: the slots were never seen moving to 2^22 ({first:?}, {done:?})")
    };
    RegrowWatch {
        reads_while_moving: done - first,
        probes_while_moving: probes,
        reads: reads.load(Ordering::Relaxed),
        // (Each host call starts with the whole budget: its fuel is the budget less what
        // it left.)
        fuel: lua.limits().fuel - lua.fuel_left(),
    }
}

/// S6T-REGROW (high): the interner placed every string again in one insert when it grew,
/// with no look at the host's interrupt or deadline: 15 to 17 ms at 600,000 strings in a
/// script or a host `load`, 52 to 58 ms at 1.6 million (making 2M strings went 51 to 82 ms
/// between looks at the deadline). The slots now move a stretch at a time, each new string
/// moving a few while lookups walk both: across the move to 2^22 slots the deadline is
/// read dozens of times, the same reads at every run, and the fuel is the same with the
/// fast loop on and off (it follows the count of strings only, as before).
#[test]
fn s6t_regrow_moves_between_looks_at_the_deadline() {
    small_stack(|| {
        let fast = regrow_watch(true);
        println!("{fast:?}");
        // (A call of `probe` may read the clock too: the reads past those are the VM's.)
        assert!(fast.probes_while_moving >= 8, "the slots moved in {} probes", fast.probes_while_moving);
        assert!(
            fast.reads_while_moving >= fast.probes_while_moving + 16,
            "the deadline was read {} times while the slots moved",
            fast.reads_while_moving
        );
        assert_eq!(regrow_watch(true), fast, "a second run");
        let full = regrow_watch(false);
        assert_eq!(full.fuel, fast.fuel, "fuel with the fast loop off");
        assert!(full.reads_while_moving >= full.probes_while_moving + 16, "fast off: {full:?}");
        assert_eq!(regrow_watch(false), full, "a second run, fast off");
    });
}

/// S6T-REGROW: a stop at the deadline in the middle of the move leaves an interner that
/// works: every string is found again (one object for one content), strings that died
/// mid-move are taken out of whichever slots hold them, and the interned count comes back
/// to where it started.
#[test]
fn s6t_regrow_stopped_mid_move_and_swept_mid_move() {
    let _heavy = super::common::memory_heavy();
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    small_stack(|| {
        let size = |lua: &Lua, key: &str| {
            lua.internal_sizes().into_iter().find(|(k, _)| *k == key).map(|(_, v)| v).unwrap()
        };
        for fast in [true, false] {
            let mut lua =
                Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(TEST_SEED), Libs::ALL);
            lua.set_fast_loop(fast);
            let base = size(&lua, "interned");
            // Stops at deadline reads until one lands while the slots move.
            let mut stopped_moving = false;
            lua.exec("L = {} N = 0", "=s").unwrap();
            for _ in 0..4000 {
                let reads = Arc::new(AtomicU64::new(0));
                let r = Arc::clone(&reads);
                let clock: remedy_lua::Clock =
                    Arc::new(move || std::time::Duration::from_nanos(r.fetch_add(1, Ordering::Relaxed)));
                lua.set_time_limit(Some((clock, std::time::Duration::from_nanos(3))));
                let r = lua.exec("while N < 900000 do L[N + 1] = 'm' .. (N + 1) N = N + 1 end", "=t");
                if r.is_ok() {
                    break;
                }
                assert!(matches!(r, Err(Error::Limit(LimitKind::Time))), "{r:?}");
                stopped_moving |= size(&lua, "interner_moving") > 0;
            }
            assert!(stopped_moving, "fast {fast}: no stop landed while the slots moved");
            lua.set_time_limit(None);
            // Strings die while the slots move (every third), are made again and looked up.
            let r = lua
                .exec(
                    "local T, bad = {}, 0 \
                     for i = 1, 900000 do if i % 3 ~= 1 then T[L[i]] = i end end \
                     for i = 1, 900000, 3 do L[i] = false end \
                     for i = 1, 800000 do local s = 'n' .. i T[s] = -i \
                       if i % 4096 == 0 then collectgarbage('step', 200) end \
                       local j = (i * 7919) % 900000 + 1 \
                       if T['m' .. j] ~= (j % 3 ~= 1 and j or nil) then bad = bad + 1 end \
                       if T['n' .. (i // 2 + 1)] ~= -(i // 2 + 1) then bad = bad + 1 end end \
                     for i = 1, 900000 do if L[i] and L[i] ~= 'm' .. i then bad = bad + 1 end end \
                     return bad",
                    "=t",
                )
                .unwrap();
            assert_eq!(super::common::show(&lua, r[0]), "0", "fast {fast}");
            lua.exec("L = nil collectgarbage() collectgarbage()", "=g").unwrap();
            assert!(
                size(&lua, "interned") <= base + 64,
                "fast {fast}: {} interned, {base} at the start",
                size(&lua, "interned")
            );
        }
    });
}

/// S6T-REGROW: with the slots moving a stretch at a time, strings dying and found again
/// while they move, the fuel, results and memory are the same under every seed and with
/// the fast loop on and off.
#[test]
fn s6t_regrow_moves_do_not_follow_the_seed() {
    small_stack(|| {
        seed_independent(
            "local L, T, c = {}, {}, 0 \
             for i = 1, 120000 do local s = 'k' .. i L[i] = s T[s] = i \
               if i % 3 == 0 then L[i // 2] = false end \
               if i % 2048 == 0 then collectgarbage('step', 100) end \
               if T['k' .. (i // 3 + 1)] == i // 3 + 1 then c = c + 1 end end \
             collectgarbage() local n = 0 for k, v in pairs(T) do n = n + 1 end return c, n",
        );
    });
}

/// One call of `body` after `setup` (hash seed `seed`, fast loop `fast`) under a clock that
/// counts its reads (each a look at the deadline): the reads, the fuel and the result.
fn key_reads(setup: &str, body: &str, seed: u64, fast: bool) -> (usize, u64, String) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut lua = Lua::new(Limits::default().memory(1 << 30).string_len(64 << 20).fuel(1 << 40).hash_seed(seed), Libs::ALL);
    lua.set_fast_loop(fast);
    lua.exec(setup, "=setup").unwrap();
    let f = lua.load(body, "=body").unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let r = Arc::clone(&reads);
    let clock: remedy_lua::Clock = Arc::new(move || {
        r.fetch_add(1, Ordering::Relaxed);
        std::time::Duration::ZERO
    });
    lua.set_time_limit(Some((clock, std::time::Duration::from_secs(3600))));
    let v = lua.call(f.value(), &[]).unwrap();
    let shown = v.iter().map(|x| super::common::show(&lua, *x)).collect::<Vec<_>>().join(" ");
    (reads.load(Ordering::Relaxed), (1u64 << 40) - lua.fuel_left(), shown)
}

/// S6T-KEY (medium): the first lookup of a 16 MiB string as a table key hashed all of it in
/// one go, deep in the interner, 6.4 ms past a 1 ms time limit (and a lookup of an equal
/// copy of an interned key compared all its bytes as well). The hash and the comparison
/// are done a megabyte at a time now, each piece a look at the deadline: on every way a
/// key reaches a table (a read, a write, a constructor, `__index`, `rawget`, `rawset`,
/// `next`), the reads are the same at every run, and the fuel is the same with the fast
/// loop on and off and under every seed (the hash and comparison are free, as before).
#[test]
fn s6t_key_long_keys_look_at_the_deadline_every_megabyte() {
    let _heavy = super::common::memory_heavy();
    small_stack(|| {
        let setup = "L = string.rep('a', 16 << 20) S = 'short' L2 = string.rep('b', 16 << 20) \
                     T = {[string.rep('b', 16 << 20)] = 1} M = setmetatable({}, {__index = function() return 2 end})";
        // (Each body uses key `K`, `L` first used; `K2` is `L2`, equal to `T`'s interned
        // key: compared with it too.)
        let bodies = [
            ("local t = {} t[K] = 1 return t[K]", 15),
            ("local t = {} return t[K]", 15),
            ("local t = {[K] = 3} return t[K]", 15),
            ("return M[K]", 15),
            ("return rawget({}, K)", 15),
            ("local t = {} rawset(t, K, 4) return t[K]", 15),
            ("return pcall(next, {}, K)", 15),
            ("return T[K2]", 30),
        ];
        for (body, at_least) in bodies {
            let long = format!("local K, K2 = L, L2 {body}");
            let short = format!("local K, K2 = S, S {body}");
            let first = key_reads(setup, &long, TEST_SEED, true);
            let plain = key_reads(setup, &short, TEST_SEED, true);
            println!("{body}: {} reads ({} with a short key), fuel {}", first.0, plain.0, first.1);
            assert!(first.0 >= plain.0 + at_least, "{body}: {} deadline reads, {} with a short key", first.0, plain.0);
            assert_eq!(key_reads(setup, &long, TEST_SEED, true), first, "{body}: a second run");
            let full = key_reads(setup, &long, TEST_SEED, false);
            assert_eq!((full.1, &full.2), (first.1, &first.2), "{body}: fast loop off");
            for seed in [0, 7, u64::MAX] {
                let other = key_reads(setup, &long, seed, true);
                assert_eq!((other.1, &other.2), (first.1, &first.2), "{body}: seed {seed:#x}");
            }
        }
    });
}

/// S6T-EQ: `==` of two different 16 MiB strings of equal bytes compared them in one go
/// (1.5 ms past the deadline); now a megabyte at a time, each piece a look at it, the
/// same fuel with the fast loop on and off and under every seed (the bytes are charged
/// as before).
#[test]
fn s6t_eq_long_strings_compare_a_megabyte_at_a_time() {
    let _heavy = super::common::memory_heavy();
    small_stack(|| {
        let setup = "A = string.rep('a', 16 << 20) B = string.rep('a', 16 << 20) C = string.rep('a', (16 << 20) - 1) .. 'b' \
                     S, U = 'short' .. 1, 'short' .. 1";
        for (body, at_least) in [("return A == B", 15), ("return A == C, A ~= C", 30)] {
            let long = key_reads(setup, body, TEST_SEED, true);
            let short = key_reads(setup, "return S == U", TEST_SEED, true);
            println!("{body}: {} reads ({} for short strings), fuel {}, {}", long.0, short.0, long.1, long.2);
            assert!(long.0 >= short.0 + at_least, "{body}: {} deadline reads", long.0);
            assert_eq!(key_reads(setup, body, TEST_SEED, true), long, "{body}: a second run");
            let full = key_reads(setup, body, TEST_SEED, false);
            assert_eq!((full.1, &full.2), (long.1, &long.2), "{body}: fast loop off");
            for seed in [0, 7, u64::MAX] {
                let other = key_reads(setup, body, seed, true);
                assert_eq!((other.1, &other.2), (long.1, &long.2), "{body}: seed {seed:#x}");
            }
        }
    });
}

/// `heap::mix_seed` (the state's hash seed from `Limits::hash_seed`, as `sweep5_gc` has
/// it).
fn state_seed(seed: u64) -> u64 {
    let mut h = seed ^ 1u64.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

fn fold_mul(a: u64, b: u64) -> u64 {
    let r = u128::from(a) * u128::from(b);
    (r as u64) ^ ((r >> 64) as u64)
}

/// Setup code making `n + 1` strings of `len` bytes (`len - 16` of `x`, then two words),
/// all of one whole hash under hash seed `seed`, the second word cancelling the first:
/// `K[i]` for i = 1..=n+1, the first n interned as keys of `T` (`T[K[i]] = i`).
fn colliding_long_keys(seed: u64, n: usize, len: usize) -> String {
    let s = state_seed(seed);
    let (k1, k2) = (s ^ 0xa076_1d64_78bd_642f, s.rotate_left(32) ^ 0xe703_7ed1_a0b4_28db);
    let mut h = s ^ (len as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let x = u64::from_le_bytes([b'x'; 8]);
    for _ in 0..(len - 16) / 8 {
        h = fold_mul(h ^ x ^ k1, k2);
    }
    let lit = |w: u64| w.to_le_bytes().iter().map(|b| format!("\\x{b:02x}")).collect::<String>();
    let tails: Vec<String> = (0..=n as u64)
        .map(|i| {
            let w1 = 0x4141_4141_0000_0000 | i;
            let w2 = fold_mul(h ^ w1 ^ k1, k2) ^ 0x1234_5678_9abc_def0;
            format!("\"{}{}\"", lit(w1), lit(w2))
        })
        .collect();
    format!(
        "local F = string.rep('x', {}) local tails = {{{}}} K = {{}} T = {{}} \
         for i = 1, #tails do K[i] = F .. tails[i] end for i = 1, {n} do T[K[i]] = i end \
         F2 = F tails2 = tails",
        len - 16,
        tails.join(",")
    )
}

/// S6T-EQ (security): with the hash seed known, a program can make any number of long
/// strings of one hash and length, and a lookup of another compared its bytes with each
/// in turn: any number of keys' worth of bytes, free, in one instruction (with 16
/// interned, `T[k]` looked at the deadline 2 times a string compared, 32 or more). Now the
/// strings of one hash and length are told apart by a crit-bit tree, and a lookup
/// compares its key with one of them at most, once (the lookup after `settle_key` uses
/// what it found): the deadline reads do not grow with the strings of that hash, found
/// or not, and the results are right.
#[test]
fn s6t_eq_keys_of_one_hash_compare_one_key_at_most() {
    let _heavy = super::common::memory_heavy();
    small_stack(|| {
        let len = 2 << 20;
        let mut reads = Vec::new();
        for n in [2, 16] {
            let setup = colliding_long_keys(TEST_SEED, n, len);
            // A copy of the last interned key (found), the one not interned (not found), and
            // an interned key's copy made a key (found, nothing interned again).
            let bodies = [
                (format!("local k = string.sub(K[{n}], 1) return T[k]"), format!("{n}")),
                (format!("local k = string.sub(K[{}], 1) return T[k]", n + 1), "nil".to_string()),
                (format!("local k = string.sub(K[1], 1) T[k] = 'x' return T[K[1]]"), "x".to_string()),
            ];
            let mut row = Vec::new();
            for (body, want) in &bodies {
                let r = key_reads(&setup, body, TEST_SEED, true);
                assert_eq!(&r.2, want, "n {n}: {body}");
                let full = key_reads(&setup, body, TEST_SEED, false);
                assert_eq!((full.1, &full.2), (r.1, &r.2), "n {n}: {body}: fast loop off");
                row.push(r.0);
            }
            println!("{n} keys of one hash: deadline reads {row:?}");
            reads.push(row);
        }
        assert_eq!(reads[0], reads[1], "the reads grow with the keys of one hash");
        assert!(reads[1].iter().all(|&r| r <= 8), "reads {:?}", reads[1]);
    });
}

/// S6T-LOAD: `load` of a 16 MiB string literal interned the constant with its hash
/// computed in one go (6 ms between looks at the deadline). Now hashed a megabyte at a
/// time, each piece a look: 15 more reads than a comment of the same length (the
/// lexer's own looks are the same), the same at every run, the fuel the same fast and
/// full and under every seed.
#[test]
fn s6t_load_long_constants_look_at_the_deadline_every_megabyte() {
    let _heavy = super::common::memory_heavy();
    small_stack(|| {
        let setup = "S = 'return \"' .. string.rep('a', 16 << 20) .. '\"' C = '--[[' .. string.rep('a', 16 << 20) .. ']] return 1'";
        let (body, comment) = ("return #load(S)()", "return load(C)()");
        let long = key_reads(setup, body, TEST_SEED, true);
        let plain = key_reads(setup, comment, TEST_SEED, true);
        println!("literal {long:?}, comment {plain:?}");
        assert_eq!(long.2, "16777216");
        assert!(long.0 >= plain.0 + 15, "{} deadline reads, {} for a comment", long.0, plain.0);
        assert_eq!(key_reads(setup, body, TEST_SEED, true), long, "a second run");
        let full = key_reads(setup, body, TEST_SEED, false);
        assert_eq!((full.1, &full.2), (long.1, &long.2), "fast loop off");
        for seed in [0, 7, u64::MAX] {
            let other = key_reads(setup, body, seed, true);
            assert_eq!((other.1, &other.2), (long.1, &long.2), "seed {seed:#x}");
        }
    });
}
