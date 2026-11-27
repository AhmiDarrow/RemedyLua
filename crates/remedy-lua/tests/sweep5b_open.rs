//! Sweep 5b: the two items sweep 5 left open, closed. Each test is named
//! `s5b_open_<what>` and compares with C Lua (`common::oracle`) and between the fast loop
//! and the full path (`common::remedy`).
//!
//! - S5GC-5: lookups among non-integer keys walked up to `disp_bound` slots (a few
//!   hundred) uncharged, so keys aimed at one run (with a known `Limits::hash_seed`) ran
//!   at 21x plain code per unit of fuel through an `__index` loop. The `nodes` part is now
//!   Lua's chained scatter table: a lookup walks only the keys with its home, random keys
//!   keep that under `CHAIN_FREE` steps at any size, steps past it are fuel, and an
//!   insert that makes a longer chain reseeds the part. (Since S5B-OPEN-2, `sweep5c_open2`:
//!   every key has two homes and joins the shorter chain, so `CHAIN_FREE` is 4 in each.)
//! - S5COR-6b: a yield inside a protected call of a protected call (`pcall(pcall, f)`)
//!   was "attempt to yield across a C-call boundary"; C Lua allows it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::time::Instant;

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua};

use super::common::{TEST_SEED, oracle, remedy};

/// Runs `setup`, then `body` in a loop until `fuel` is spent: nanoseconds per unit of fuel.
pub(crate) fn ns_per_unit(setup: &str, body: &str, fuel: u64, fast: bool) -> f64 {
    let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(TEST_SEED), Libs::ALL);
    lua.set_fast_loop(fast);
    lua.set_print(|_| {});
    lua.exec(setup, "=setup").unwrap();
    lua.set_limits(lua.limits().fuel(fuel));
    let f = lua.load(format!("while true do {body}\nend"), "=body").unwrap();
    let _measuring = super::common::measuring();
    let start = Instant::now();
    let r = lua.call(f.value(), &[]);
    let t = start.elapsed();
    assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{body}: {r:?}");
    t.as_nanos() as f64 / fuel as f64
}

/// A typical library call's cost per unit on this machine and build (as sweep 5's).
pub(crate) fn baseline(fast: bool) -> f64 {
    ns_per_unit("t = {} s = 'abc'", "local x = tostring(#s) .. s t[1] = x", 300_000, fast)
}

/// `table::CHAIN_FREE`: the chain steps a lookup takes free in each of a key's two chains
/// (a chain of `CHAIN_FREE + 1` keys is the longest an insert leaves without reseeding).
pub(crate) const CHAIN_FREE: usize = 4;

/// Keys a pair of chains of `CHAIN_FREE + 1` holds: the most keys aimed at both homes of
/// one pair an insert leaves without reseeding.
pub(crate) const PAIR_FULL: usize = 2 * (CHAIN_FREE + 1);

/// `heap::mix_seed`.
fn mix_seed(base: u64, n: u64) -> u64 {
    let mut h = base ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// The murmur3 finalizer `value::hash_key` mixes with.
fn fmix(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// `fmix` run backwards.
fn unfmix(mut h: u64) -> u64 {
    let inv = |a: u64| {
        let mut x = a;
        for _ in 0..6 {
            x = x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)));
        }
        x
    };
    h ^= h >> 33;
    h = h.wrapping_mul(inv(0xc4ce_b9fe_1a85_ec53));
    h ^= h >> 33;
    h = h.wrapping_mul(inv(0xff51_afd7_ed55_8ccd));
    h ^ (h >> 33)
}

/// A table's key hash (`value::hash_key`) for a float key under table seed `seed`.
pub(crate) fn float_home(f: f64, seed: u64) -> u64 {
    fmix((f.to_bits() ^ 0x5555) ^ seed)
}

/// A float key's two homes (`table::second_home`) in 2^`bits` slots under table seed `seed`.
pub(crate) fn homes(f: f64, seed: u64, bits: u32) -> (u64, u64) {
    let mask = (1u64 << bits) - 1;
    let h = float_home(f, seed);
    let a = h & mask;
    (a, a ^ (((h >> 32) & mask) | 1))
}

/// The seeds a table's hash part goes through: the state's table seed, then each reseed.
pub(crate) fn table_seeds(seed: u64, n: usize) -> Vec<u64> {
    let mut v = vec![mix_seed(seed, 2)];
    while v.len() < n {
        let last = *v.last().unwrap();
        v.push(mix_seed(last, 0x007e_5eed));
    }
    v
}

/// Float keys (not of the form `i + 0.25`) whose two homes are `home` and the slot
/// farthest from it (`home ^ mask`) of 2^`bits` (and so of every smaller size) under each
/// of `seeds`: they fill the two chains in turn. One seed: made by running the mixer
/// backwards; more: searched for (keep `bits` small).
pub(crate) fn aimed_floats(seeds: &[u64], bits: u32, home: u64, n: usize) -> Vec<f64> {
    let mask = (1u64 << bits) - 1;
    let mut out = Vec::new();
    if let [seed] = seeds {
        let mut j = 0u64;
        while out.len() < n {
            j += 1;
            let h = home | (mask << 32) | (j << (32 + bits)) | ((j & 0xff) << 24);
            let f = f64::from_bits(unfmix(h) ^ seed ^ 0x5555);
            if f.is_finite() && f.fract() != 0.0 && (f - 0.25).fract() != 0.0 && f.abs() < 1e15 {
                assert_eq!(homes(f, *seed, bits), (home, home ^ mask));
                out.push(f);
            }
        }
        return out;
    }
    let mut x = 0.5f64;
    while out.len() < n {
        x += 1.0;
        if seeds.iter().all(|&s| homes(x, s, bits) == (home, home ^ mask)) {
            out.push(x);
        }
    }
    out
}

/// A Lua list of floats.
pub(crate) fn list(fs: &[f64]) -> String {
    fs.iter().map(|f| format!("{f:?}")).collect::<Vec<_>>().join(",")
}

/// The S5GC-5 shape: a table `t` with `fill` plain float keys and then `run` float keys
/// whose home is one of the first 4 of its 2^`bits` slots, and `k`: an absent key with
/// the same home. Under open addressing they made one run that `k`'s lookup walked.
fn run_of_floats(bits: u32, fill: usize, run: usize) -> String {
    let ts = mix_seed(TEST_SEED, 2);
    let mask = (1u64 << bits) - 1;
    let mut aimed = Vec::new();
    let mut x = 0.5f64;
    while aimed.len() < run + 1 {
        x += 1.0;
        if float_home(x, ts) & mask < 4 {
            aimed.push(x);
        }
    }
    let k = aimed.pop().unwrap_or_default();
    format!(
        "t = {{x = 1}} for i = 1, {fill} do t[i + 0.25] = i end for _, f in ipairs({{{}}}) do t[f] = f end k = {k:?}",
        list(&aimed)
    )
}

/// The longest free walk now: `fill` plain keys, then keys aimed at both homes of one pair
/// under the first `seeds` seeds the table goes through, as many as the two chains take
/// (`CHAIN_FREE + 1` each, plus one per extra seed), and `k`: an absent key aimed the same
/// way, whose lookup walks both chains.
fn chain_of_floats(bits: u32, fill: usize, seeds: usize) -> String {
    let ss = table_seeds(TEST_SEED, seeds);
    let mask = (1u64 << bits) - 1;
    // A pair of homes no plain key has.
    let home = (0..mask)
        .find(|h| {
            (1..=fill).all(|i| {
                let (a, b) = homes(i as f64 + 0.25, ss[0], bits);
                ![a, b].contains(h) && ![a, b].contains(&(h ^ mask))
            })
        })
        .unwrap();
    let n = PAIR_FULL + seeds - 1;
    let mut aimed = aimed_floats(&ss, bits, home, n + 1);
    let k = aimed.pop().unwrap_or_default();
    format!(
        "t = {{}} for i = 1, {fill} do t[i + 0.25] = i end for _, f in ipairs({{{}}}) do t[f] = f end k = {k:?}",
        list(&aimed)
    )
}

/// `m` `__index` chains of 1,999 distinct small tables (16 slots) each holding the same
/// `PAIR_FULL` keys (aimed at both homes of one pair, every table having the state's seed,
/// or plain keys), and `k` (absent: aimed the same way, or plain): a body that looks `k`
/// up through each chain in turn, so with many chains the tables do not stay in the caches.
pub(crate) fn index_chains(m: usize, aimed: bool) -> (String, String) {
    let ss = table_seeds(TEST_SEED, 1);
    let mut keys = aimed_floats(&ss, 4, 3, PAIR_FULL + 1);
    let mut k = keys.pop().unwrap_or_default();
    if !aimed {
        keys = (0..PAIR_FULL).map(|i| i as f64 + 0.25).collect();
        k = -0.375;
    }
    let setup = format!(
        "local a = {{{}}} U = {{}} for j = 1, {m} do local last = {{}} \
         for i = 1, 1999 do local x = {{}} for _, f in ipairs(a) do x[f] = true end \
         setmetatable(x, {{__index = last}}) last = x end U[j] = last end \
         k = {k:?} collectgarbage() collectgarbage('stop')",
        list(&keys)
    );
    (setup, format!("for j = 1, {m} do local x = U[j][k] end"))
}

/// S5GC-5: keys aimed at one home with a known seed cost at most about 4x plain code per
/// unit of fuel, as a plain lookup and through an `__index` self-loop (a link is a unit:
/// 2,000 lookups of one key per call), in the old shape (a run of 150 under open
/// addressing: 5.4x and 21x before) and at the longest pair of chains random keys never
/// reach (`PAIR_FULL` keys aimed at both homes). (Through chains of distinct tables that do
/// not stay in the caches: `sweep5c_open2`, `probe_index_dram`, `probe_index_big`;
/// S5B-OPEN-2.)
#[test]
fn s5b_open_aimed_keys_are_metered() {
    let cases = [
        (run_of_floats(14, 6200, 150), "local x = t[k]"),
        (
            format!("{} setmetatable(t, {{__index = t}})", run_of_floats(14, 6200, 150)),
            "pcall(function() return t[k] end)",
        ),
        (chain_of_floats(14, 6200, 1), "local x = t[k]"),
        (
            format!("{} setmetatable(t, {{__index = t}})", chain_of_floats(14, 6200, 1)),
            "pcall(function() return t[k] end)",
        ),
        (
            format!("{} setmetatable(t, {{__index = t}})", chain_of_floats(5, 2, 1)),
            "pcall(function() return t[k] end)",
        ),
    ];
    for fast in [true, false] {
        let mut bad = Vec::new();
        for (setup, body) in &cases {
            // Noisy machine (and tests running beside): the best of five ratios, each
            // against plain code measured just before.
            let mut ratio = f64::MAX;
            for _ in 0..5 {
                let base = baseline(fast);
                ratio = ratio.min(ns_per_unit(setup, body, 300_000, fast) / base);
                if ratio <= 4.0 {
                    break;
                }
            }
            println!("fast {fast}: {ratio:.1}x  {body}");
            if ratio > 4.0 {
                bad.push(format!("{body}: {ratio:.1}x plain code"));
            }
        }
        assert!(bad.is_empty(), "fast {fast}:\n{}", bad.join("\n"));
    }
}

/// Fuel of `src` (run with `seed`, fast loop on or off), its result and memory held.
pub(crate) fn measured(src: &str, seed: u64, fast: bool) -> (String, u64, usize) {
    let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(seed), Libs::ALL);
    lua.set_fast_loop(fast);
    let r = lua.exec(src, "=t").unwrap();
    let r: Vec<String> = r.iter().map(|v| super::common::show(&lua, *v)).collect();
    (r.join(" "), (1u64 << 40) - lua.fuel_left(), lua.memory_used())
}

/// S5GC-5: keys aimed at both homes under the table's seed and the seed it takes after it
/// stay together over a reseed (chains of `CHAIN_FREE + 2` and `CHAIN_FREE + 1` keys in 16
/// slots): the steps past `CHAIN_FREE` are fuel (each lookup of `k` costs more than one of a
/// plain absent key; nothing did before), and the program's results and fuel match C Lua's
/// and the full path's.
#[test]
fn s5b_open_walks_past_the_bound_are_fuel() {
    let aimed = chain_of_floats(5, 1, 2);
    let lookups = |key: &str| {
        format!(
            "{aimed} local q, n = {key}, 0 for i = 1, 3000 do if t[q] == nil then n = n + 1 end end return n"
        )
    };
    for fast in [true, false] {
        // `k` is `CHAIN_FREE + 1` steps down the longer chain: 1 charged per lookup.
        let (r1, f_aimed, _) = measured(&lookups("k"), TEST_SEED, fast);
        let (r2, f_plain, _) = measured(&lookups("0.125"), TEST_SEED, fast);
        assert_eq!((r1.as_str(), r2.as_str()), ("3000", "3000"));
        assert!(f_aimed >= f_plain + 3000, "fast {fast}: aimed {f_aimed}, plain {f_plain}");
    }
    let p = format!(
        "{aimed} local u = setmetatable({{}}, {{__index = t}}) local n = 0 \
         for i = 1, 50 do if u[k] == nil then n = n + 1 end if t[k] == nil then n = n + 1 end end \
         local c, s = 0, 0 for kk, v in pairs(t) do c = c + 1 s = s + v end \
         t[k] = function(self, x) return x + 1 end for i = 1, 50 do n = u[k](u, n) end \
         return n, c, s"
    );
    let ours = remedy(&p, 1 << 40);
    assert_eq!(ours, oracle(&p, 1 << 40));
}

/// S5GC-5: aimed keys (one seed and several) with lookups, removals, reinsertion, `next`
/// after removal, `#` and method calls through `__index`: C Lua's results, and the same
/// results and fuel with the fast loop on and off.
#[test]
fn s5b_open_aimed_keys_match_c_lua_and_the_full_path() {
    for setup in [
        chain_of_floats(14, 6200, 1),
        chain_of_floats(5, 1, 2),
        chain_of_floats(6, 20, 2),
        run_of_floats(14, 6200, 150),
    ] {
        let p = format!(
            "{setup} local u = setmetatable({{}}, {{__index = t}}) local n = 0 \
             for i = 1, 50 do if u[k] == nil then n = n + 1 end if t[k] == nil then n = n + 1 end end \
             local ks = {{}} for kk in pairs(t) do ks[#ks + 1] = kk end \
             for i = 1, #ks, 3 do t[ks[i]] = nil end \
             local c = 0 for kk, v in pairs(t) do c = c + 1 t[kk] = nil end \
             for i = 1, #ks, 2 do t[ks[i]] = i end \
             for i = 1, 40 do t[i] = i end \
             t[k] = function(self, x) return x + 1 end for i = 1, 50 do n = u[k](u, n) end \
             t.m = t[k] local o = setmetatable({{}}, {{__index = u}}) for i = 1, 50 do n = o:m(n) end \
             local s = 0 for kk, v in pairs(t) do if type(v) == 'number' then s = s + v end end \
             return n, c, #t, s"
        );
        let ours = remedy(&p, 1 << 40);
        assert_eq!(ours, oracle(&p, 1 << 40), "{setup:.80}");
        assert!(ours.is_ok(), "{ours:?}");
    }
}

/// S5B-OPEN-1: a new key took the first removed key's slot on its probe path, so how
/// many inserts used a fresh slot, and with it when the hash part was rebuilt (charged),
/// depended on the layout: the same churn (remove a key, add another, 100,000 times) used
/// 1,780,565, 1,780,309 or 1,780,181 fuel under three seeds. A new key now always takes a
/// node never used (as in Lua), so the count is the program's.
#[test]
fn s5b_open_churn_fuel_is_seed_independent() {
    let src = "local t, f = {}, {} for i = 1, 1000 do t['a' .. i] = i f[i + 0.5] = i end \
        for r = 1, 100 do for i = 1, 1000 do t['a' .. ((r - 1) * 1000 + i)] = nil t['a' .. (r * 1000 + i)] = i \
        f[(r - 1) * 1000 + i + 0.5] = nil f[r * 1000 + i + 0.5] = i end end \
        local c = 0 for k, v in pairs(t) do c = c + v end for k, v in pairs(f) do c = c + v end return c";
    for fast in [true, false] {
        let first = measured(src, TEST_SEED, fast);
        assert_eq!(first.0, "1001000");
        for seed in [0, 1, 2, 3, 0xdead_beef] {
            assert_eq!(measured(src, seed, fast), first, "seed {seed:#x} fast {fast}");
        }
    }
}

/// A home `h` of 2^`bits` slots such that no plain key `i + 0.25` (`i` up to `fill`) has `h`
/// or `h ^ mask` as either of its homes under table seed `seed`.
fn free_pair(seed: u64, bits: u32, fill: usize) -> u64 {
    let mask = (1u64 << bits) - 1;
    let mut used = vec![false; 1 << bits];
    for i in 1..=fill {
        let (a, b) = homes(i as f64 + 0.25, seed, bits);
        used[a as usize] = true;
        used[b as usize] = true;
    }
    (0..=mask).find(|&h| !used[h as usize] && !used[(h ^ mask) as usize]).unwrap()
}

/// A big table (2^`bits` slots, 3/4 full of plain float keys) with `PAIR_FULL` keys aimed
/// at both homes of one pair spread through the fill, and `k`, an absent key with those
/// homes; `q`, an absent plain key.
fn big_chain(bits: u32) -> String {
    let ss = table_seeds(TEST_SEED, 1);
    let fill = (3usize << bits) / 4 - 30;
    let home = free_pair(ss[0], bits, fill);
    let mut aimed = aimed_floats(&ss, bits, home, PAIR_FULL + 1);
    let k = aimed.pop().unwrap_or_default();
    format!(
        "local a = {{{}}} t = {{}} local j = 1 for i = 1, {fill} do t[i + 0.25] = i if i % {} == 0 and a[j] then t[a[j]] = i j = j + 1 end end \
         for i = j, #a do t[a[i]] = i end k = {k:?} q = -0.375",
        list(&aimed),
        fill / (PAIR_FULL + 1)
    )
}

/// Probe (not a test): aimed and plain lookups in big tables, ns per unit as ratios to
/// plain code. `BITS=21 cargo test -p remedy-lua --test all sweep5b_open::probe_big -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_big() {
    let bits: u32 = std::env::var("BITS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    let setup = big_chain(bits);
    for fast in [true, false] {
        let base = baseline(fast);
        for body in ["local x = t[q]", "local x = t[k]"] {
            let ns = ns_per_unit(&setup, body, 3_000_000, fast);
            println!("fast {fast} 2^{bits}: {:.1}x {ns:.1} ns/unit  {body}", ns / base);
        }
        let s2 = format!("{setup} setmetatable(t, {{__index = t}})");
        for key in ["q", "k"] {
            let body = format!("pcall(function() return t[{key}] end)");
            let ns = ns_per_unit(&s2, &body, 3_000_000, fast);
            println!("fast {fast} 2^{bits}: {:.1}x {ns:.1} ns/unit  {body}", ns / base);
        }
    }
}

/// Probe (not a test): many aimed pairs of chains in a table too big for the caches,
/// looked up in turn (so every chain step can be a cache miss), against plain absent keys
/// in the same table. `CHAIN` keys per pair (`PAIR_FULL`).
/// `BITS=21 cargo test -p remedy-lua --test all sweep5b_open::probe_dram -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_dram() {
    let bits: u32 = std::env::var("BITS").ok().and_then(|s| s.parse().ok()).unwrap_or(21);
    let chain: usize = std::env::var("CHAIN").ok().and_then(|s| s.parse().ok()).unwrap_or(PAIR_FULL);
    let ts = table_seeds(TEST_SEED, 1)[0];
    let n_keys = (3usize << bits) / 4 - 64;
    let chains = n_keys / chain;
    // Pairs (h, h ^ mask) for h below 2^(bits - 2): distinct at this size and the one before.
    let picked: Vec<Vec<f64>> = (0..chains).map(|c| aimed_floats(&[ts], bits, c as u64, chain + 1)).collect();
    // Round-robin, so one chain's nodes lie far apart.
    let mut keys = Vec::new();
    for r in 0..chain {
        for c in &picked {
            keys.extend_from_slice(&c[r].to_le_bytes());
        }
    }
    let mut absent = Vec::new();
    let mut plain = Vec::new();
    for (i, c) in picked.iter().enumerate() {
        absent.extend_from_slice(&c[chain].to_le_bytes());
        plain.extend_from_slice(&(-(i as f64) - 0.375).to_le_bytes());
    }
    for fast in [true, false] {
        let base = baseline(fast);
        for (name, probe) in [("plain", &plain), ("aimed", &absent)] {
            let mut lua =
                Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40).hash_seed(TEST_SEED), Libs::ALL);
            lua.set_fast_loop(fast);
            for (g, b) in [("KEYS", &keys), ("PROBE", probe)] {
                let s = lua.create_string(b).unwrap();
                lua.set_global(g, s).unwrap();
            }
            lua.exec(
                "t = {} for i = 1, #KEYS // 8 do t[string.unpack('<d', KEYS, i * 8 - 7)] = i end \
                 P = {} for i = 1, #PROBE // 8 do P[i] = string.unpack('<d', PROBE, i * 8 - 7) end KEYS, PROBE = nil, nil collectgarbage() collectgarbage('stop')",
                "=setup",
            )
            .unwrap();
            let f = lua.load("local t, P, j, n = t, P, 1, #P for r = 1, 1000000 do local x = t[P[j]] j = j % n + 1 end", "=body").unwrap();
            let left = 1u64 << 40;
            let start = Instant::now();
            lua.call(f.value(), &[]).unwrap();
            let el = start.elapsed().as_nanos() as f64;
            let fuel = (left - lua.fuel_left()) as f64;
            let ns = el / fuel;
            println!("  {:.1} ns per lookup, {:.2} fuel per lookup", el / 1e6, fuel / 1e6);
            println!(
                "fast {fast} 2^{bits} {} chains of {chain}: {name} {:.1}x ({ns:.1} ns/unit)",
                picked.len(),
                ns / base
            );
        }
    }
}

/// Probe (not a test): `__index` chains of 1,999 distinct small tables (16 slots), each
/// holding the same `PAIR_FULL` aimed keys (every table has the state's seed), `M`
/// of them looked up in turn so the tables do not stay in the caches; against tables of
/// plain keys. ns per unit as ratios to plain code.
/// `M=40 cargo test -p remedy-lua --test all sweep5b_open::probe_index_dram -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_index_dram() {
    let m: usize = std::env::var("M").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    for fast in [true, false] {
        let base = baseline(fast);
        for aimed in [false, true] {
            let (setup, body) = index_chains(m, aimed);
            let ns = ns_per_unit(&setup, &body, 3_000_000, fast);
            println!("fast {fast} {m} chains: aimed {aimed} {:.1}x ({ns:.1} ns/unit)", ns / base);
        }
    }
}

/// Probe (not a test): `__index` chains of 1,999 distinct tables of 2^`BITS` slots, each
/// with plain keys and either a run of keys whose home is one of the first 4 slots (the
/// S5GC-5 shape: `AIM=run`, `RUN` keys) or a pair of chains of `CHAIN_FREE + 1` keys aimed
/// at both homes (`AIM=chain`), or none (`AIM=none`), and an absent key aimed the same way.
/// `BITS=9 AIM=run RUN=200 cargo test -p remedy-lua --test all sweep5b_open::probe_index_big -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_index_big() {
    let env = |k: &str, d: usize| std::env::var(k).ok().and_then(|s| s.parse().ok()).unwrap_or(d);
    let bits = env("BITS", 9) as u32;
    let run = env("RUN", 200);
    let m = env("M", 1);
    let aim = std::env::var("AIM").unwrap_or_else(|_| "chain".into());
    let ss = table_seeds(TEST_SEED, 1);
    let mask = (1u64 << bits) - 1;
    let mut keys: Vec<f64> = match aim.as_str() {
        "run" => {
            let mut v = Vec::new();
            let mut x = 0.5f64;
            while v.len() < run + 1 {
                x += 1.0;
                if float_home(x, ss[0]) & mask < 4 {
                    v.push(x);
                }
            }
            v
        }
        "chain" => {
            let fill = (3usize << bits) / 4 - PAIR_FULL - 2;
            aimed_floats(&ss, bits, free_pair(ss[0], bits, fill), PAIR_FULL + 1)
        }
        _ => vec![-0.375],
    };
    let k = keys.pop().unwrap_or_default();
    let fill = (3usize << bits) / 4 - keys.len() - 2;
    let setup = format!(
        "local a = {{{}}} U = {{}} for j = 1, {m} do local last = {{}} \
         for i = 1, 1999 do local x = {{}} for i = 1, {fill} do x[i + 0.25] = i end for _, f in ipairs(a) do x[f] = true end \
         setmetatable(x, {{__index = last}}) last = x end U[j] = last end \
         k = {k:?} collectgarbage() collectgarbage('stop')",
        list(&keys)
    );
    let body = format!("for j = 1, {m} do local x = U[j][k] end");
    for fast in [true, false] {
        let base = baseline(fast);
        let ns = ns_per_unit(&setup, &body, 3_000_000, fast);
        println!("fast {fast} 2^{bits} {aim} {}: {:.1}x ({ns:.1} ns/unit)", keys.len(), ns / base);
    }
}

#[test]
#[ignore = "probe"]
fn probe_seeds() {
    let src = std::env::var("SRC").unwrap();
    for seed in [TEST_SEED, 0, 1, 2] {
        println!("{seed:#x} {:?}", measured(&src, seed, true));
    }
}

/// S5GC-5: fuel and memory never depend on the hash seed: big tables of string, float,
/// table and boolean keys, removals and new keys (churn), traversals with `next` while
/// fields are cleared, `#` over a table whose integer keys went to the hash part, and
/// weak keys. Eight seeds, fast loop on and off.
#[test]
fn s5b_open_fuel_is_seed_independent() {
    let src = "local t, f, o = {}, {}, {} local objs = {} \
        for i = 1, 40000 do t['k' .. i] = i f[i + 0.5] = i end \
        for i = 1, 2000 do objs[i] = {} o[objs[i]] = i end \
        local s = 0 for k, v in pairs(t) do s = s + v end \
        for r = 1, 6 do for i = r, 40000, 7 do t['k' .. i] = nil f[i + 0.5] = nil end \
          for i = 1, 6000 do t['z' .. r .. '_' .. i] = i f[-i - r * 0.25] = i end end \
        local c = 0 for k, v in pairs(t) do c = c + 1 if v % 3 == 0 then t[k] = nil end end \
        for k, v in next, f do if v % 2 == 0 then f[k] = nil end end \
        local m = {x = 1} for i = 1, 3000 do m[i * 2] = i end m[true] = 1 \
        for i = 1, 3000, 2 do m[i * 2] = nil end \
        local w = setmetatable({}, {__mode = 'k'}) for i = 1, 2000 do w[objs[i]] = i end \
        for i = 1, 2000, 2 do objs[i] = nil end collectgarbage() \
        local wc = 0 for _ in pairs(w) do wc = wc + 1 end \
        local mc = 0 for _ in pairs(m) do mc = mc + 1 end \
        return s, c, mc, wc";
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

/// The same results as C Lua, with the fast loop on and off (same results and fuel).
fn same_as_c(program: &str) {
    let ours = remedy(program, 100_000_000);
    assert_eq!(ours, oracle(program, 100_000_000), "{program}");
    assert!(ours.is_ok(), "{ours:?}");
}

/// S5COR-6b: a yield inside a protected call of a protected call (`pcall(pcall, f)`, any
/// mix with `xpcall`, any depth) said "attempt to yield across a C-call boundary": the
/// inner call ran as a nested call (S5COR-6). C Lua allows it. Results, errors (positions
/// and levels included), handler order and `__close` order after the yields are C Lua's.
#[test]
fn s5b_open_yield_inside_nested_protected_calls() {
    same_as_c(
        r##"
        local log = {}
        local function closing(tag, yields)
            return setmetatable({}, {__close = function(o, e)
                if yields then coroutine.yield("close " .. tag) end
                log[#log + 1] = tag .. ":" .. tostring(e)
            end})
        end
        local function pack(...) return {n = select("#", ...), ...} end
        -- Runs `f` in a coroutine, resuming it until it ends: every value it yields and
        -- what it returns.
        local function drive(f, ...)
            local co = coroutine.create(f)
            local out, args = {}, pack(...)
            while true do
                local r = pack(coroutine.resume(co, table.unpack(args, 1, args.n)))
                out[#out + 1] = r
                if coroutine.status(co) == "dead" then return out end
                args = pack("resumed", #out)
            end
        end
        local r = {}
        r[#r + 1] = drive(function(a) return pcall(pcall, function(x) local y, z = coroutine.yield(x) return y, z, "done" end, a) end, "arg")
        r[#r + 1] = drive(function() return pcall(pcall, pcall, function() coroutine.yield(1) coroutine.yield(2) return 3 end) end)
        r[#r + 1] = drive(function() return pcall(pcall, pcall, coroutine.yield, "native", "yield") end)
        r[#r + 1] = drive(function() return select("#", pcall(pcall, coroutine.yield)) end)
        r[#r + 1] = drive(function() return pcall(pcall, function() coroutine.yield() error("after yield") end) end)
        r[#r + 1] = drive(function() return pcall(pcall, function() coroutine.yield() error({}) end) end)
        r[#r + 1] = drive(function() return pcall(pcall, function() coroutine.yield() error("level 2", 2) end) end)
        r[#r + 1] = drive(function() return pcall(pcall, function() coroutine.yield() local x = nil .. 1 end) end)
        r[#r + 1] = drive(function() return pcall(pcall, error, "direct", 2) end)
        r[#r + 1] = drive(function() return pcall(pcall, 42) end)
        r[#r + 1] = drive(function() return pcall(xpcall, 42, function(m) return "h:" .. m end) end)
        -- Handlers: the innermost protected call's only, after the yield.
        r[#r + 1] = drive(function() return pcall(xpcall, function() coroutine.yield() error("x", 0) end, function(m) log[#log + 1] = "h1" return "h:" .. m end) end)
        r[#r + 1] = drive(function() return xpcall(pcall, function(m) log[#log + 1] = "never" return m end, function() coroutine.yield() error("y", 0) end) end)
        r[#r + 1] = drive(function() return xpcall(xpcall, function(m) return "outer" end, function() coroutine.yield() error("z", 0) end, function(m) return "inner:" .. m end) end)
        r[#r + 1] = drive(function() return pcall(xpcall, xpcall, function(m) return "outer" end, function() coroutine.yield() error("w", 0) end, function(m) return "inner:" .. m end) end)
        -- `__close` order, with yields in the closing handlers, on success and on error.
        r[#r + 1] = drive(function() return pcall(pcall, function()
            local a <close> = closing("a", true)
            local b <close> = closing("b", false)
            coroutine.yield("body")
            return "ok"
        end) end)
        r[#r + 1] = drive(function() return pcall(pcall, function()
            local a <close> = closing("c", true)
            local b <close> = closing("d", true)
            coroutine.yield("body")
            error("boom", 0)
        end) end)
        r[#r + 1] = drive(function() return pcall(xpcall, function()
            local a <close> = closing("e", true)
            coroutine.yield("body")
            error("bang", 0)
        end, function(m) log[#log + 1] = "handler:" .. m return "H" end) end)
        -- A coroutine run by `wrap` inside: its yields go to the inner `pcall`, its death
        -- ends it.
        r[#r + 1] = drive(function() return pcall(pcall, coroutine.wrap(function() coroutine.yield("w1") error("wrapped", 0) end)) end)
        r[#r + 1] = drive(function()
            local w = coroutine.wrap(function() coroutine.yield("w2") error("dies", 0) end)
            local a = pack(pcall(pcall, w))
            local b = pack(pcall(pcall, w))
            coroutine.yield("between")
            local c = pack(pcall(pcall, w))
            return a, b, c
        end)
        -- Deep: 150 levels, with a yield at the bottom.
        r[#r + 1] = drive(function()
            local fs = {}
            for i = 1, 150 do fs[i] = pcall end
            fs[#fs + 1] = function(...) return coroutine.yield(...) end
            fs[#fs + 1] = "deep"
            local res = pack(pcall(table.unpack(fs)))
            return res.n, res[1], res[150], res[151], res[152], res[153]
        end)
        -- In a method call, a tail call and a `for` iterator.
        local obj = {}
        function obj:m(...) return pcall(pcall, function(...) return coroutine.yield(...) end, ...) end
        r[#r + 1] = drive(function() return obj:m(1, 2) end)
        r[#r + 1] = drive(function() local function t() return pcall(pcall, coroutine.yield, "tail") end return t() end)
        r[#r + 1] = drive(function()
            local s = 0
            for i, v in function(_, i) if i < 3 then local _, _, x = pcall(pcall, coroutine.yield, i) return i + 1, x end end, nil, 0 do s = s + i end
            return s
        end)
        r[#r + 1] = table.concat(log, " ")
        return r
        "##,
    );
}

/// S5COR-6b: error levels and positions inside nested protected calls (each protected call
/// is a C function in C Lua, so a level that names one gives no position), and the number
/// of times a failing message handler runs before "C stack overflow" (C Lua's C levels:
/// every enclosing protected call counts) are C Lua's, with and without coroutines.
#[test]
fn s5b_open_nested_protected_calls_count_c_levels() {
    same_as_c(
        r##"
        local r = {}
        local function lvl(n) return function() error("at " .. n, n) end end
        for n = 1, 4 do
            r[#r + 1] = {pcall(pcall, lvl(n))}
            r[#r + 1] = {pcall(pcall, pcall, lvl(n))}
            r[#r + 1] = {pcall(function() return pcall(pcall, lvl(n)) end)}
            r[#r + 1] = {pcall(pcall, error, "e" .. n, n)}
            r[#r + 1] = {pcall(pcall, pcall, error, "e" .. n, n)}
        end
        local function runaway()
            local seen = 0
            local res = {xpcall(error, function(m)
                seen = seen + 1
                if m == "C stack overflow" then return "overflow at " .. seen end
                error(m, 0)
            end, "first")}
            return res[2], seen
        end
        r[#r + 1] = {runaway()}
        r[#r + 1] = {pcall(pcall, runaway)}
        r[#r + 1] = {pcall(pcall, pcall, runaway)}
        r[#r + 1] = {pcall(xpcall, runaway, print)}
        r[#r + 1] = {coroutine.wrap(function() return pcall(pcall, function() coroutine.yield() return runaway() end) end)()}
        local co = coroutine.wrap(function() return pcall(pcall, pcall, function() coroutine.yield() return runaway() end) end)
        co()
        r[#r + 1] = {co()}
        -- S5B-OPEN-3: protected (and continued) calls a resume after a yield unrolled no
        -- longer count; ones made after it do, in place of returned ones too.
        local g = coroutine.wrap(function() return pcall(function() coroutine.yield() return runaway() end) end)
        g()
        r[#r + 1] = {g()}
        local h = coroutine.wrap(function() return pcall(function() coroutine.yield() return pcall(runaway) end) end)
        h()
        r[#r + 1] = {h()}
        local k = coroutine.wrap(function()
            local a = {pcall(function() coroutine.yield() return 1 end)}
            local b = {pcall(pcall, function() return pcall(runaway) end)}
            local c = {pcall(function() local x <close> = setmetatable({}, {__close = function() end}) coroutine.yield() return pcall(runaway) end)}
            return a[2], b[4], b[5], c[3], c[4]
        end)
        k()
        k()
        r[#r + 1] = {k()}
        local t = setmetatable({}, {__index = function(t, key) coroutine.yield() return select(2, runaway()) end})
        local m = coroutine.wrap(function() return pcall(function() return t.x end) end)
        m()
        r[#r + 1] = {m()}
        local f = coroutine.wrap(function() return pcall(xpcall, function() coroutine.yield() return runaway() end, print) end)
        f()
        r[#r + 1] = {f()}
        return r
        "##,
    );
}

/// S5COR-6b: nesting costs no native stack: 20,000 protected calls each the callee of the
/// next, with a yield at the bottom, on a 256 KiB stack (C Lua stops at its 200 C levels;
/// RemedyLua has no such limit here).
#[test]
fn s5b_open_deep_nested_protected_calls_use_no_native_stack() {
    let r = std::thread::Builder::new()
        .stack_size(256 << 10)
        .spawn(|| {
            let src = "local fs = {} for i = 1, 20000 do fs[i] = (i % 2 == 0) and pcall or pcall end \
                fs[#fs + 1] = function(x) local y = coroutine.yield(x) return y .. '!' end fs[#fs + 1] = 'bottom' \
                local co = coroutine.wrap(function() local t = table.pack(pcall(table.unpack(fs))) return t.n, t[1], t[20001], t[20002] end) \
                local y = co() return y, co('up')";
            let mut out = Vec::new();
            for fast in [true, false] {
                out.push(measured(src, TEST_SEED, fast));
            }
            out
        })
        .unwrap()
        .join()
        .unwrap();
    assert_eq!(r[0].0, "bottom 20002 true true up!");
    assert_eq!(r[0], r[1]);
}
