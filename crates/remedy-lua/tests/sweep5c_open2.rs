//! Sweep 5c: S5B-OPEN-2, closed. Each test is named `s5c_open2_<what>` and compares with C
//! Lua (`common::oracle`) and between the fast loop and the full path (`common::remedy`).
//!
//! With a known `Limits::hash_seed` (every table of a state shares the seed), keys aimed at
//! one home put into many tables reached through `__index` chains ran at 7-20x plain code
//! per unit of fuel: a lookup walked up to `CHAIN_FREE` = 16 chain steps free (random keys
//! reach chains of 9-11, so fuel had to leave them free to stay seed-independent), each a
//! dependent load on memory that is not in the caches. Now every key has two homes and
//! joins the shorter chain (two-choice): random keys never make a chain past 4 (the tail is
//! doubly exponential), so only 4 steps past each home are free, and a lookup walks its two
//! chains in step (their loads overlap). See `table::CHAIN_FREE`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common::{TEST_SEED, oracle, remedy};
use super::sweep5b_open::{CHAIN_FREE, PAIR_FULL, aimed_floats, baseline, homes, list, measured, ns_per_unit, table_seeds};

/// `m` `__index` chains of 1,999 distinct tables, each holding the same `n` float keys (all
/// tables have the state's seed): keys aimed at both homes of one pair of 32 slots
/// (`aimed_floats`, so also of every smaller size; the first `PAIR_FULL` fill the two
/// chains in turn, any more reseed the table), or plain keys; and an absent key `k` aimed
/// the same way (or plain). The body looks `k` up through each chain in turn (a link is a unit of
/// fuel), so with many chains the tables do not stay in the caches.
fn tables_with_keys(m: usize, n: usize, aimed: bool) -> (String, String) {
    let ss = table_seeds(TEST_SEED, 1);
    let (keys, k) = if aimed {
        let mut keys = aimed_floats(&ss, 5, 3, n + 1);
        let k = keys.pop().unwrap_or_default();
        (keys, k)
    } else {
        ((0..n).map(|i| i as f64 + 0.25).collect(), -0.375)
    };
    let setup = format!(
        "local a = {{{}}} U = {{}} for j = 1, {m} do local last = {{}} \
         for i = 1, 1999 do local x = {{}} for _, f in ipairs(a) do x[f] = true end \
         setmetatable(x, {{__index = last}}) last = x end U[j] = last end \
         k = {k:?} collectgarbage() collectgarbage('stop')",
        list(&keys)
    );
    (setup, format!("for j = 1, {m} do local x = U[j][k] end"))
}

/// The best of up to `tries` ratios of `setup`/`body`'s ns per unit of fuel to plain code
/// measured just before (a noisy machine, tests running beside), stopping once under `bound`.
fn best_ratio(setup: &str, body: &str, fast: bool, bound: f64, tries: usize) -> f64 {
    let mut ratio = f64::MAX;
    for _ in 0..tries {
        let base = baseline(fast);
        ratio = ratio.min(ns_per_unit(setup, body, 1_000_000, fast) / base);
        if ratio <= bound {
            break;
        }
    }
    ratio
}

/// S5B-OPEN-2: keys aimed at one pair of homes (a known seed) put into 1,999 tables reached
/// through an `__index` chain cost at most 4x plain code per unit of fuel, fast loop and
/// full path: `PAIR_FULL` keys (two chains of `CHAIN_FREE + 1`, the most an insert leaves
/// without reseeding: the worst now) and 17 (one chain of 17 under the single-choice
/// layout, the worst before: 7-12x; now the table reseeds). Plain keys in the same shape
/// are about 1x.
#[test]
fn s5c_open2_many_tables_aimed_keys_are_bounded() {
    let cases = [(PAIR_FULL, true), (17, true), (PAIR_FULL, false)];
    for fast in [true, false] {
        let mut bad = Vec::new();
        for &(n, aimed) in &cases {
            let (setup, body) = tables_with_keys(1, n, aimed);
            let ratio = best_ratio(&setup, &body, fast, 4.0, 5);
            println!("fast {fast}: {n} keys aimed {aimed}: {ratio:.1}x plain code");
            if ratio > 4.0 {
                bad.push(format!("{n} keys aimed {aimed}: {ratio:.1}x plain code"));
            }
        }
        assert!(bad.is_empty(), "fast {fast}:\n{}", bad.join("\n"));
    }
}

/// S5B-OPEN-2: the same attack in tables that do not stay in the caches (40 chains of 1,999
/// tables, about 80,000 tables): plain keys in that shape already cost several times plain
/// code (each link touches a table, its metatable and their nodes, all cold), so the bound
/// here is against them: aimed keys cost at most 2.25x what plain keys cost in the same
/// tables (2.8-3x before, 16 steps free; now about 1.6x).
#[test]
fn s5c_open2_cold_tables_aimed_keys_cost_little_more_than_plain_keys() {
    let (ps, pb) = tables_with_keys(40, PAIR_FULL, false);
    let mut bad = Vec::new();
    for fast in [true, false] {
        for n in [PAIR_FULL, 17] {
            let (setup, body) = tables_with_keys(40, n, true);
            let mut ratio = f64::MAX;
            for _ in 0..5 {
                let plain = ns_per_unit(&ps, &pb, 1_000_000, fast);
                ratio = ratio.min(ns_per_unit(&setup, &body, 1_000_000, fast) / plain);
                if ratio <= 2.25 {
                    break;
                }
            }
            println!("fast {fast}: {n} aimed keys: {ratio:.2}x plain keys");
            if ratio > 2.25 {
                bad.push(format!("fast {fast}: {n} aimed keys: {ratio:.2}x plain keys"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// S5B-OPEN-2: many tables holding keys aimed at one pair of homes (and keys aimed at the
/// first home only, which spread over their second homes), with lookups hit and miss,
/// through `__index`, removals, `next` after removal, reinsertion, `#`, and method calls:
/// C Lua's results, and the same results and fuel with the fast loop on and off.
#[test]
fn s5c_open2_aimed_keys_match_c_lua_and_the_full_path() {
    let ss = table_seeds(TEST_SEED, 2);
    let pair = aimed_floats(&ss[..1], 4, 3, 2 * PAIR_FULL);
    let over = aimed_floats(&ss, 4, 5, PAIR_FULL + 2);
    let mask = 15u64;
    let mut first_only = Vec::new();
    let mut x = 0.5f64;
    while first_only.len() < 24 {
        x += 1.0;
        if super::sweep5b_open::float_home(x, ss[0]) & mask == 9 {
            first_only.push(x);
        }
    }
    for keys in [&pair[..PAIR_FULL], &pair[..], &over[..], &first_only[..]] {
        let p = format!(
            "local a = {{{}}} local T = {{}} local last = {{}} \
             for j = 1, 60 do local x = {{}} for i = 1, #a - 1 do x[a[i]] = i * j end \
             setmetatable(x, {{__index = last}}) last = x T[j] = x end \
             local k, n, s = a[#a], 0, 0 \
             for r = 1, 3 do for j = 1, 60 do if T[j][k] == nil then n = n + 1 end s = s + (T[j][a[1]] or 0) end end \
             for i = 1, #a - 1 do s = s + (last[a[i]] or 0) end \
             local t = T[30] local ks = {{}} for kk in pairs(t) do ks[#ks + 1] = kk end \
             for i = 1, #ks, 3 do t[ks[i]] = nil end \
             local c = 0 for kk, v in pairs(t) do c = c + 1 t[kk] = nil end \
             for i = 1, #ks, 2 do t[ks[i]] = i end for i = 1, 40 do t[i] = i end \
             T[1][k] = function(self, v) return v + 1 end for i = 1, 50 do n = last[k](last, n) end \
             T[2].m = T[1][k] local o = setmetatable({{}}, {{__index = last}}) for i = 1, 50 do n = o:m(n) end \
             for kk, v in pairs(t) do if type(v) == 'number' then s = s + v end end \
             return n, c, #t, s",
            list(keys)
        );
        let ours = remedy(&p, 1 << 40);
        assert_eq!(ours, oracle(&p, 1 << 40), "{} keys", keys.len());
        assert!(ours.is_ok(), "{ours:?}");
    }
}

/// S5B-OPEN-2: fuel and memory never depend on the hash seed with two homes per key:
/// many small tables (4 to 16 slots, where a key's two homes are closest and chains of
/// random keys are longest), big ones, misses and hits through `__index`, removals and
/// new keys (churn), `next` while fields are cleared, and a full collection. Eight seeds,
/// fast loop on and off, with the incremental collector running (S5C-1 made its fuel the
/// same under every seed too).
#[test]
fn s5c_open2_fuel_is_seed_independent() {
    let src = "local T, s = {}, 0 \
        for j = 1, 6000 do local x = {} for i = 1, j % 13 do x['f' .. i] = i x[i + j * 0.5] = i end T[j] = x end \
        for j = 2, 6000 do if j % 50 ~= 1 then setmetatable(T[j], {__index = T[j - 1]}) end end \
        for r = 1, 3 do for j = 1, 6000, 7 do local x = T[j] s = s + (x.f1 or 0) + (x[-0.5] or 0) + (x['g' .. r] or 1) end end \
        local big = {} for i = 1, 30000 do big[i + 0.125] = i big['b' .. i] = i end \
        for i = 1, 30000, 3 do big[i + 0.125] = nil big['b' .. (i + 1)] = nil big[-i - 0.5] = i end \
        local c = 0 for k, v in pairs(big) do c = c + 1 if v % 4 == 0 then big[k] = nil end end \
        for j = 1, 6000, 5 do local x = T[j] for k in pairs(x) do x[k] = nil end x.z = j end \
        for j = 1, 6000 do local v = T[j].z if v then s = s + v end end \
        collectgarbage() local w = 0 for j = 1, 6000 do if next(T[j]) then w = w + 1 end end \
        return s, c, w";
    let mut seen = Vec::new();
    for seed in [TEST_SEED, 0, 1, 2, 0xdead_beef, u64::MAX, 0x1234_5678_9abc_def0, 77] {
        for fast in [true, false] {
            seen.push((seed, fast, measured(src, seed, fast)));
        }
    }
    let first = &seen[0].2;
    for (seed, fast, m) in &seen {
        assert_eq!(m, first, "seed {seed:#x} fast {fast}");
    }
}

/// S5B-OPEN-2: keys aimed at a pair of homes under the table's seed and the next one
/// (chains of `CHAIN_FREE + 2` and `CHAIN_FREE + 1` after the reseed) in many tables: each
/// lookup of the absent aimed key pays for its step past `CHAIN_FREE` (the fast loop and
/// the full path alike), so it costs more fuel than a plain absent key.
#[test]
fn s5c_open2_steps_past_the_bound_are_fuel_in_every_table() {
    let ss = table_seeds(TEST_SEED, 2);
    let mut keys = aimed_floats(&ss, 4, 2, PAIR_FULL + 2);
    let k = keys.pop().unwrap_or_default();
    let prog = |key: String| {
        format!(
            "local a = {{{}}} local T = {{}} for j = 1, 200 do local x = {{}} for _, f in ipairs(a) do x[f] = j end T[j] = x end \
             local q, n = {key}, 0 for r = 1, 20 do for j = 1, 200 do if T[j][q] == nil then n = n + 1 end end end return n",
            list(&keys)
        )
    };
    for fast in [true, false] {
        let (r1, f_aimed, _) = measured(&prog(format!("{k:?}")), TEST_SEED, fast);
        let (r2, f_plain, _) = measured(&prog("0.125".into()), TEST_SEED, fast);
        assert_eq!((r1.as_str(), r2.as_str()), ("4000", "4000"));
        assert!(f_aimed >= f_plain + 4000, "fast {fast}: aimed {f_aimed}, plain {f_plain} ({CHAIN_FREE} steps free)");
    }
}

/// Probe (not a test): the attacker's best layout in big tables. 1,999 tables of 2^`BITS`
/// slots (`M` chains of them), each holding a pair of full chains (`KEYS` = `PAIR_FULL` keys
/// aimed at homes `a` and `a ^ mask`; on the single-choice layout, `KEYS=17` is one chain) whose nodes are far from their homes and from each other: the
/// 8 slots after each home (where a chain looks for a free node first) are taken by keys
/// aimed there, and between the chain's keys come keys of three other such pairs, so each
/// chain node comes from the top of the part, 8 nodes from the one before (a cache line or
/// more): every step of the walk is its own load. Plain keys fill the rest. Against the
/// same tables looked up with a plain absent key.
/// `BITS=9 M=1 cargo test -p remedy-lua --test all sweep5c_open2::probe_far -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_far() {
    let env = |k: &str, d: usize| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
    let bits = env("BITS", 9) as u32;
    let m = env("M", 1);
    let keys = env("KEYS", PAIR_FULL);
    let ss = table_seeds(TEST_SEED, 1);
    let mask = (1u64 << bits) - 1;
    let n = 1usize << bits;
    let plain = n * 3 / 10;
    // Homes of the pairs (the chain's and three spacers'), clear of plain keys' homes.
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
        // Room for its window and its partner's, apart from the others'.
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
    let per: Vec<Vec<f64>> = pairs.iter().map(|&p| aimed_floats(&ss, bits, p, keys + 1)).collect();
    let k = per[0][keys];
    let mut late = Vec::new();
    for r in 0..keys {
        for c in &per {
            late.push(c[r]);
        }
    }
    let setup = |key: f64| {
        format!(
            "local F, L = {{{}}}, {{{}}} U = {{}} for j = 1, {m} do local last = {{}} \
             for i = 1, 1999 do local x = {{}} for i = 1, {plain} do x[i + 0.25] = i end \
             for _, f in ipairs(F) do x[f] = true end for _, f in ipairs(L) do x[f] = true end \
             setmetatable(x, {{__index = last}}) last = x end U[j] = last end \
             k = {key:?} collectgarbage() collectgarbage('stop')",
            list(&fill),
            list(&late)
        )
    };
    let body = format!("for j = 1, {m} do local x = U[j][k] end");
    for fast in [true, false] {
        let base = baseline(fast);
        let p = ns_per_unit(&setup(-0.375), &body, 3_000_000, fast);
        let a = ns_per_unit(&setup(k), &body, 3_000_000, fast);
        println!("fast {fast} 2^{bits} x{m}: plain {:.1}x ({p:.1} ns/unit), aimed far {:.1}x ({a:.1} ns/unit)", p / base, a / base);
    }
}
