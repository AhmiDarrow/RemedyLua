//! Sweep 5, area "collector, tables, coroutines and allocator": a hostile
//! script against hash parts, weak tables and ephemerons, coroutines and the slab
//! allocator. Each test is a finding (`S5GC-n`) or an attack that is stopped correctly.
//! Every program runs with the fast loop on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::time::Instant;

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua};

/// Runs `setup`, then `body` in a loop until `fuel` is spent: nanoseconds per unit of fuel.
fn ns_per_unit(setup: &str, body: &str, fuel: u64, fast: bool) -> f64 {
    // (384 MiB: S5GC-2's 2M closures are charged what their upvalue lists take since
    // SEC-M9, past 256 MiB; the cost measured is per unit of fuel either way.)
    let mut lua = Lua::new(Limits::default().memory(384 << 20).fuel(1 << 40).hash_seed(super::common::TEST_SEED), Libs::ALL);
    lua.set_fast_loop(fast);
    lua.set_print(|_| {});
    lua.exec(setup, "=setup").unwrap();
    lua.set_limits(lua.limits().fuel(fuel));
    let f = lua.load(format!("while true do {body}\nend"), "=body").unwrap();
    let start = Instant::now();
    let r = lua.call(f.value(), &[]);
    let t = start.elapsed();
    assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{body}: {r:?}");
    t.as_nanos() as f64 / fuel as f64
}

/// A typical library call's cost per unit on this machine and build.
fn baseline(fast: bool) -> f64 {
    ns_per_unit("t = {} s = 'abc'", "local x = tostring(#s) .. s t[1] = x", 300_000, fast)
}

/// At most this many times plain code per unit of fuel (sweep 4's bound).
const BOUND: f64 = 12.0;

/// Each `(setup, body)` costs at most `BOUND` times plain code per unit of fuel.
fn assert_metered(cases: &[(&str, &str)], fuel: u64) {
    for fast in [true, false] {
        let base = baseline(fast);
        let mut bad = Vec::new();
        for (setup, body) in cases {
            let mut ns = ns_per_unit(setup, body, fuel, fast);
            if ns > BOUND * base {
                // Noisy machine: the best of three.
                ns = ns.min(ns_per_unit(setup, body, fuel, fast)).min(ns_per_unit(setup, body, fuel, fast));
            }
            if ns > BOUND * base {
                bad.push(format!("{body}: {ns:.0} ns/unit = {:.0}x plain code", ns / base));
            }
        }
        assert!(bad.is_empty(), "fast {fast}, plain {base:.1} ns/unit:\n{}", bad.join("\n"));
    }
}

/// Probe (not a test): ns per unit of the cases, as ratios to plain code.
/// `cargo test -p remedy-lua --test all sweep5_gc::probe_metering -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_metering() {
    let cases = probe_cases();
    for fast in [true, false] {
        let base = baseline(fast);
        for (setup, body) in &cases {
            let ns = ns_per_unit(setup, body, 300_000, fast);
            println!("fast {fast}: {:.1}x  {body}", ns / base);
        }
    }
}

fn probe_cases() -> Vec<(String, String)> {
    let sel = std::env::var("S5").unwrap_or_default();
    let mut v = Vec::new();
    if sel.is_empty() || sel == "sort" {
        for (bits, n, l) in [(15, 64, 500), (15, 2, 8000), (17, 2, 30000)] {
            let d = deep_chains(bits, n, l);
            v.push((d.clone(), format!("table.sort(t) -- {bits} {n} {l}")));
            v.push((d.clone(), "local x = #t".to_owned()));
            v.push((format!("{d} setmetatable(t, {{__index = t}}) k = 1 + ((1 << ({bits} + 1)) - 1) * 999"), "pcall(function() return t[k] end)".to_owned()));
        }
    }
    if sel.is_empty() || sel == "sparse" {
        let sparse = sparse_slab;
        v.push((sparse(1_000, "{}"), "local t = {}".to_owned()));
        v.push((sparse(1_200_000, "{}"), "local t = {}".to_owned()));
        v.push((sparse(2_000_000, "function() return i end"), "local t = {}".to_owned()));
        v.push((sparse(200_000, "coroutine.create(print)"), "local t = {}".to_owned()));
    }
    if sel.is_empty() || sel == "nodes" {
        let d = run_of_floats(super::common::TEST_SEED, 14, 6200, 150);
        v.push((d.clone(), "local x = t[k]".to_owned()));
        v.push((format!("{d} setmetatable(t, {{__index = t}})"), "pcall(function() return t[k] end)".to_owned()));
    }
    if sel.is_empty() || sel == "intern" {
        let (setup, n) = (colliding_strings(super::common::TEST_SEED, 4000), 4000);
        v.push((setup.clone(), format!("local x = a[{n}] .. b[{n}]")));
        v.push((setup.clone(), "local x = a[2] .. b[2]".to_owned()));
        v.push((setup, "local x = a[1] .. 'zzzzzzzz'".to_owned()));
    }
    v
}

/// A table's key hash (`value::hash_key`) for a float key under table seed `seed`.
fn float_home(f: f64, seed: u64) -> u64 {
    let mut h = (f.to_bits() ^ 0x5555) ^ seed;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// A table `t` with `fill` plain float keys and then `run` float keys whose home slot is
/// one of the first 4 of its 2^`bits` hash slots (made by a script that knows the seed),
/// and `k`: an absent key with the same home, whose lookup walks the whole run.
fn run_of_floats(seed: u64, bits: u32, fill: usize, run: usize) -> String {
    let ts = mix_seed(seed, 2);
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
    let list: Vec<String> = aimed.iter().map(|f| format!("{f:?}")).collect();
    format!(
        "t = {{x = 1}} for i = 1, {fill} do t[i + 0.25] = i end \
         for _, f in ipairs({{{}}}) do t[f] = f end k = {k:?}",
        list.join(",")
    )
}

/// Slabs left mostly free: `n` objects made by `make`, one in 1,000 kept (the last too,
/// so the slab's tail is live and nothing is trimmed).
fn sparse_slab(n: u32, make: &str) -> String {
    format!(
        "keep = {{}} do local j = {{}} for i = 1, {n} do j[i] = {make} end \
         for i = 1000, {n}, 1000 do keep[#keep + 1] = j[i] end end collectgarbage() collectgarbage()"
    )
}

/// S5GC-1: a lookup's chain steps in an integer part are charged as the longest of the
/// instruction's (or the native's since its last charge). Work that looks up many keys in
/// one charge paid for one: `table.sort` read every element without charging them (24x
/// plain code per unit of fuel with 64 keys at the end of chains of 500), an `__index`
/// chain charged a step per link but one lookup's probing (130x to 160x for a table that
/// is its own `__index`), and `#t`'s search and `gsub` with a table were the same.
#[test]
fn s5gc_1_many_lookups_in_one_charge_each_pay() {
    let d = deep_chains(15, 64, 500);
    let looped =
        format!("{} setmetatable(t, {{__index = t, __newindex = t}}) k = 1 + ((1 << 16) - 1) * 999", deep_chains(15, 2, 8000));
    let gsub = format!("{d} s = string.rep('x', 63)");
    // `#t` probes keys 1, 2, 4, ... 2^40 and then halves: each at the end of a chain.
    let border = "t = {} local J = 1 << 50 for j = 1, (1 << 15) + 1 do t[J + j] = true end \
                  for j = 1, (1 << 15) + 1 do t[J + j] = nil end local M = (1 << 16) - 1 \
                  for b = 0, 40 do local i = 1 << b t[i % M + M * 3000] = true t[i] = b \
                  for c = 1, 600 do t[i % M + M * (3000 + c)] = true end end";
    assert_metered(
        &[
            (&d, "table.sort(t)"),
            (&d, "table.sort(t, function(a, b) return a > b end)"),
            (&looped, "pcall(function() return t[k] end)"),
            (&looped, "pcall(function() t[k] = 1 end)"),
            (&gsub, "string.gsub(s, '()', t)"),
            (border, "local n = #t"),
        ],
        300_000,
    );
}

/// S5GC-1: the same programs give C Lua's results, and the same fuel with the fast loop
/// on and off (the fast loop's `__index` chains charge each link's probing as the full
/// path does).
#[test]
fn s5gc_1_probe_charging_matches_c_lua_and_the_full_path() {
    let progs = [
        format!("{} table.sort(t) local s = {{}} for i = 1, #t do s[i] = t[i] end return #t, table.concat(s, ',')", deep_chains(11, 16, 60)),
        format!(
            "{} local u = setmetatable({{}}, {{__index = setmetatable({{}}, {{__index = t}})}}) \
             local s = 0 for i = 1, 8 do s = s + u[i] end local n = 0 for i = 1, 200 do if u[i + 2047 * 1001] then n = n + 1 end end return s, n, #t",
            deep_chains(10, 8, 60)
        ),
    ];
    for p in &progs {
        let ours = super::common::remedy(p, 1 << 40);
        let theirs = super::common::oracle(p, 1 << 40);
        assert_eq!(ours, theirs, "{p}");
    }
}

/// S5GC-2: collector walks over free slab slots were free. A spike of objects, then one
/// in 1,000 kept: every later cycle walked the whole slab (and the atomic step and the
/// stack shrink the whole thread slab) for the work of the few live ones, 15x to 30x
/// plain code per unit of fuel under a 256 MB limit, and more under a larger one. The
/// walks now skip free slots 64 and 4,096 at a time (a bitmap per slab), so they still
/// cost no fuel: a program uses the same fuel in a fresh state and in one with sparse
/// slabs.
#[test]
fn s5gc_2_sparse_slabs_are_skipped() {
    let tables = sparse_slab(1_200_000, "{}");
    let funcs = sparse_slab(2_000_000, "function() return i end");
    let threads = sparse_slab(200_000, "coroutine.create(print)");
    assert_metered(&[(&tables, "local t = {}"), (&funcs, "local t = {}"), (&threads, "local t = {}")], 300_000);
    let work = "local t = {} for i = 1, 20000 do t[i % 100 + 1] = {i} end \
                local co = coroutine.wrap(function() for i = 1, 100 do coroutine.yield({}) end end) \
                for i = 1, 100 do co() end collectgarbage() return #t";
    // 300 objects kept, from 300 or from 300,000 made.
    let kept = |n: u32, step: u32, make: &str| {
        format!(
            "keep = {{}} do local j = {{}} for i = 1, {n} do j[i] = {make} end \
             for i = {step}, {n}, {step} do keep[#keep + 1] = j[i] end end collectgarbage() collectgarbage()"
        )
    };
    for fast in [true, false] {
        let fuel_in = |setup: &str| {
            let mut lua = Lua::new(Limits::default().memory(256 << 20).hash_seed(super::common::TEST_SEED), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(setup, "=setup").unwrap();
            let f = lua.load(work, "=work").unwrap();
            lua.collect_garbage().unwrap();
            lua.set_limits(lua.limits().fuel(1 << 40));
            lua.call(f.value(), &[]).unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        assert_eq!(fuel_in(&kept(300_000, 1000, "{}")), fuel_in(&kept(300, 1, "{}")), "fast {fast}");
        let co = "coroutine.create(print)";
        assert_eq!(fuel_in(&kept(300_000, 1000, co)), fuel_in(&kept(300, 1, co)), "fast {fast}");
    }
}

/// S5GC-3: the string interner's walks were free. A script that knows the hash seed
/// makes any number of strings with one hash (`colliding_strings`); with 4,000 of them,
/// each `..` that makes one again walked all 4,000 (290x plain code per unit of fuel), and
/// interning them was quadratic, in `load` too. Steps over strings with the same hash
/// bits are now fuel; strings aimed at one run by the low bits of their hashes only
/// (`aimed_strings`) spread again under a new salt.
#[test]
fn s5gc_3_interner_walks_are_charged() {
    let n = 4000;
    let setup = colliding_strings(super::common::TEST_SEED, n);
    let last = format!("local x = a[{n}] .. b[{n}]");
    let native = format!("local x = table.concat({{a[{n}], b[{n}]}})");
    let format = format!("local x = string.format('%s%s', a[{n}], b[{n}])");
    // Interning them all again from source.
    let src = format!("{setup} src = 'return {{' .. string.format(string.rep('%q,', #keep), table.unpack(keep)) .. '}}' keep = nil a = nil b = nil collectgarbage()");
    let aimed = aimed_strings(super::common::TEST_SEED, 3000, 14);
    let other = "local x = a[1] .. 'zzzzzzzz'";
    assert_metered(
        &[
            (&setup, &last),
            (&setup, &native),
            (&setup, &format),
            (&setup, other),
            (&src, "local f = load(src)"),
            (&aimed, "local x = a[3000] .. b[3000]"),
            (&aimed, "local x = a[1] .. b[1]"),
        ],
        1_000_000,
    );
}

/// S5GC-3: with the walks charged, colliding strings still give C Lua's results, and the
/// same fuel with the fast loop on and off (its `..` charges them as the full path does).
#[test]
fn s5gc_3_colliding_strings_match_c_lua_and_the_full_path() {
    let setup = colliding_strings(super::common::TEST_SEED, 300);
    let p = format!(
        "{setup} local n, t = 0, {{}} for i = 1, #a do local s = a[i] .. b[i] n = n + #s t[s] = i end \
         for i = #a, 1, -1 do assert(t[a[i] .. b[i]] == i) end keep = nil collectgarbage() \
         local u = {{}} for i = 1, #a do u[#u + 1] = string.format('%s%s', a[i], b[i]) end return n, #u"
    );
    let ours = super::common::remedy(&p, 1 << 40);
    assert_eq!(ours, super::common::oracle(&p, 1 << 40));
    assert_eq!(ours, Ok("4800 300".to_owned()));
}

/// S5GC-5 (closed in sweep 5b): a lookup among non-integer keys walked up to `disp_bound`
/// slots (a few hundred) free. With the seed known, float keys aimed at one run below that
/// bound made an absent key's lookup walk it all; through an `__index` loop, 15x to 25x
/// plain code per unit of fuel at 2^14 slots (`probe_metering` with `S5=nodes`), and
/// charging those walks made fuel depend on the seed (random keys reach some 270 slots at
/// 2^20). The hash part is now Lua's chained layout: a lookup walks only its home's keys,
/// at most `table::CHAIN_FREE` steps free (random keys never get there), the rest charged
/// (`sweep5b_open`). Results and fuel stay C Lua's and the full path's.
#[test]
fn s5gc_5_aimed_float_keys_match_c_lua_and_the_full_path() {
    let d = run_of_floats(super::common::TEST_SEED, 14, 6200, 150);
    // The same results as C Lua, and the same fuel with the fast loop on and off (lookups,
    // method calls and `__index` links that walk the run).
    let p = format!(
        "{d} local u = setmetatable({{}}, {{__index = t}}) local n = 0 \
         for i = 1, 50 do if u[k] == nil then n = n + 1 end if t[k] == nil then n = n + 1 end end \
         t[k] = function(self, x) return x + 1 end for i = 1, 50 do n = u[k](u, n) end \
         t.m = t[k] local o = setmetatable({{}}, {{__index = u}}) for i = 1, 50 do n = o:m(n) end return n"
    );
    let ours = super::common::remedy(&p, 1 << 40);
    assert_eq!(ours, super::common::oracle(&p, 1 << 40));
    assert_eq!(ours, Ok("200".to_owned()));
}

/// The state seed `base` from the first `math.random(0)` of a fresh state, as the
/// generator was seeded before S5GC-4 (`rng[1] = mix_seed(base, 3) ^ golden`, and
/// xoshiro256** outputs `rotl(rng[1] * 5, 7) * 9`): every step inverts.
fn base_from_first_random(r: u64) -> u64 {
    let inv = |a: u64| {
        // Newton's iteration for the inverse of an odd number mod 2^64.
        let mut x = a;
        for _ in 0..6 {
            x = x.wrapping_mul(2u64.wrapping_sub(a.wrapping_mul(x)));
        }
        x
    };
    let s1 = r.wrapping_mul(inv(9)).rotate_right(7).wrapping_mul(inv(5));
    let mut h = s1 ^ 0x9E37_79B9_7F4A_7C15;
    // `mix_seed` backwards: each `h ^= h >> 33` is its own inverse.
    h ^= h >> 33;
    h = h.wrapping_mul(inv(0xc4ce_b9fe_1a85_ec53));
    h ^= h >> 33;
    h = h.wrapping_mul(inv(0xff51_afd7_ed55_8ccd));
    h ^= h >> 33;
    h ^ 3u64.wrapping_mul(0x9E37_79B9_7F4A_7C15)
}

/// S5GC-4: `math.random(0)` gave back the generator's state word, and the generator was
/// seeded with an invertible mix of the state's one seed, so one call (in a state with a
/// fresh random seed, the default) recovered the seed and with it the string hash: the
/// script could then make colliding strings at will (S5GC-3). Now the generator's seed is
/// a one-way function of it: strings made for the seed the old inversion finds do not
/// collide (their interning costs no more fuel than other strings).
#[test]
fn s5gc_4_math_random_does_not_give_away_the_hash_seed() {
    let fuel_of = |lua: &mut Lua, src: &str| {
        lua.set_limits(lua.limits().fuel(1 << 40));
        lua.exec(src, "=t").unwrap();
        (1u64 << 40) - lua.fuel_left()
    };
    for fast in [true, false] {
        for seed in [None, Some(super::common::TEST_SEED)] {
            let mut limits = Limits::default().memory(256 << 20);
            if let Some(s) = seed {
                limits = limits.hash_seed(s);
            }
            let mut lua = Lua::new(limits, Libs::ALL);
            lua.set_fast_loop(fast);
            let r = match lua.exec("return math.random(0)", "=r").unwrap()[0] {
                remedy_lua::Value::Int(i) => i as u64,
                other => panic!("{other:?}"),
            };
            let guess = base_from_first_random(r);
            if let Some(s) = seed {
                assert_ne!(guess, s, "the old inversion still finds the seed");
            }
            // Strings made for the guess, against as many made for another seed (to the
            // state's seed, both are random strings).
            let n = 2000;
            let plain = fuel_of(&mut lua, &colliding_strings(guess ^ 0xdead_beef, n));
            let crafted = fuel_of(&mut lua, &colliding_strings(guess, n));
            assert!(crafted < plain * 2, "fast {fast} seed {seed:?}: crafted strings cost {crafted} fuel, plain {plain}");
            if let Some(s) = seed {
                // The check has teeth: strings made for the real seed do cost more.
                let real = fuel_of(&mut lua, &colliding_strings(s, n));
                assert!(real > plain * 10, "fast {fast}: strings for the real seed cost {real} fuel, plain {plain}");
            }
        }
    }
}

// The state's string hash (`heap::hash_bytes` under `heap::mix_seed(seed, 1)`), as a
// script that knows `Limits::hash_seed` can compute it.
fn mix_seed(base: u64, n: u64) -> u64 {
    let mut h = base ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
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

/// The state's string hash (`heap::hash_bytes`) under state seed `s`.
fn hash_bytes(s: u64, b: &[u8]) -> u64 {
    let k1 = s ^ 0xa076_1d64_78bd_642f;
    let k2 = s.rotate_left(32) ^ 0xe703_7ed1_a0b4_28db;
    let mut h = s ^ (b.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut chunks = b.chunks_exact(8);
    for c in &mut chunks {
        h = fold_mul(h ^ u64::from_le_bytes(c.try_into().unwrap()) ^ k1, k2);
    }
    let mut w = [0u8; 8];
    w[..chunks.remainder().len()].copy_from_slice(chunks.remainder());
    h = fold_mul(h ^ u64::from_le_bytes(w) ^ k1, k2);
    fold_mul(h ^ k2, k1 | 1)
}

/// Strings of 16 bytes (as halves `a[i] .. b[i]`, interned in `keep`) whose hashes all
/// differ but put them in the first 4 slots of an interner of 2^`bits` slots (its first
/// salt): aimed by their low bits.
fn aimed_strings(seed: u64, n: usize, bits: u32) -> String {
    let s = mix_seed(seed, 1);
    let mask = (1u64 << bits) - 1;
    let lit = |w: u64| w.to_le_bytes().iter().map(|b| format!("\\x{b:02x}")).collect::<String>();
    let (mut a, mut b) = (String::new(), String::new());
    let mut w2 = 0u64;
    for i in 0..n as u64 {
        let w1 = 0x4242_4242_0000_0000 | i;
        loop {
            w2 += 1;
            let mut bytes = w1.to_le_bytes().to_vec();
            bytes.extend_from_slice(&w2.to_le_bytes());
            if fold_mul(hash_bytes(s, &bytes), 0x9E37_79B9_7F4A_7C15) & mask < 4 {
                break;
            }
        }
        a.push_str(&format!("\"{}\",", lit(w1)));
        b.push_str(&format!("\"{}\",", lit(w2)));
    }
    format!("a = {{{a}}} b = {{{b}}} keep = {{}} for i = 1, #a do keep[i] = a[i] .. b[i] end")
}

/// Strings of 16 bytes, all with one hash under the state seed made from `seed`: the
/// first 8 bytes free, the next 8 cancel them (`h1 ^ w2` is the same for all). Setup code
/// that keeps them as halves `a[i] .. b[i]` (8 bytes each) and interns the whole strings.
pub(super) fn colliding_strings(seed: u64, n: usize) -> String {
    let s = mix_seed(seed, 1);
    let k1 = s ^ 0xa076_1d64_78bd_642f;
    let k2 = s.rotate_left(32) ^ 0xe703_7ed1_a0b4_28db;
    let h0 = s ^ 16u64.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let lit = |w: u64| w.to_le_bytes().iter().map(|b| format!("\\x{b:02x}")).collect::<String>();
    let mut a = String::new();
    let mut b = String::new();
    for i in 0..n as u64 {
        let w1 = 0x4141_4141_0000_0000 | i;
        let h1 = fold_mul(h0 ^ w1 ^ k1, k2);
        let w2 = h1 ^ 0x1234_5678_9abc_def0;
        a.push_str(&format!("\"{}\",", lit(w1)));
        b.push_str(&format!("\"{}\",", lit(w2)));
    }
    format!("a = {{{a}}} b = {{{b}}} keep = {{}} for i = 1, #a do keep[i] = a[i] .. b[i] end")
}

/// Keys 1..n each at the end of an integer-part chain of `l` colliding keys, in an
/// integer part of 2^(bits + 1) nodes (Lua's node layout: main position
/// `k mod (size - 1)`; a later collider goes right after the main position, pushing the
/// earlier ones down the chain). Needs `n * (l + 2) < 2^bits`.
fn deep_chains(bits: u32, n: u32, l: u32) -> String {
    format!(
        "t = {{}}
local J = 1 << 40
for j = 1, (1 << {bits}) + 1 do t[J + j] = true end   -- the integer part grows to 2^(bits+1)
for j = 1, (1 << {bits}) + 1 do t[J + j] = nil end    -- keys stay in their nodes, values gone
local M = (1 << ({bits} + 1)) - 1
for i = 1, {n} do
  t[i + M * 1000] = true          -- takes i's main position
  t[i] = i * 7 % {n}              -- chained right after it
  for c = 1, {l} do t[i + M * (1000 + c)] = true end   -- each pushes i one further down
end
-- (the colliders stay: removing them walks their chains)
assert(#t == {n})"
    )
}

/// S5GC-6: a `__close` handler that yields while `coroutine.close` runs it failed with
/// "attempt to yield from outside a coroutine"; C Lua says "attempt to yield across a
/// C-call boundary" (the coroutine is not yieldable there, and it is not the main one).
#[test]
fn s5gc_6_yield_in_close_handler_matches_c_lua() {
    let p = "local co = coroutine.create(function() \
               local a <close> = setmetatable({}, {__close = function() coroutine.yield('y') end}) \
               coroutine.yield() end) \
             coroutine.resume(co) \
             local r = table.pack(pcall(coroutine.close, co)) \
             return r[1], r[2], r[3], coroutine.status(co), select(2, pcall(coroutine.yield))";
    let ours = super::common::remedy(p, 1 << 30);
    assert_eq!(ours, super::common::oracle(p, 1 << 30));
}

/// S5GC-7: call frame records (about 100 bytes each) were not charged, only the stack
/// slots (some 36 bytes a level): coroutines parked deep in recursion held 285 MB of
/// process memory under a 64 MB limit. The frame list is now charged as it grows (the
/// fast loop leaves growing it to the full path), and given back when a coroutine dies.
#[test]
fn s5gc_7_call_frames_are_charged() {
    let park = "local function mk() \
                  local co = coroutine.create(function() \
                    local function f(n) if n == 0 then coroutine.yield() return 0 end local x = f(n - 1) return x end \
                    f(1900) end) \
                  coroutine.resume(co) return co end";
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().memory(64 << 20), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.collect_garbage().unwrap();
        let base = lua.memory_used();
        lua.exec(format!("{park} cs = {{}} for i = 1, 50 do cs[i] = mk() end"), "=park").unwrap();
        lua.collect_garbage().unwrap();
        let per_level = (lua.memory_used() - base) as f64 / (50.0 * 1900.0);
        assert!(per_level > 100.0, "fast {fast}: {per_level:.0} bytes charged a level");
        // At the limit, a memory error, not a process past it.
        let r = lua.exec(format!("{park} local more = {{}} for i = 1, 100000 do more[i] = mk() end"), "=more");
        assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{r:?}");
        // Finished or dropped, they give it all back.
        lua.exec("for i = 1, #cs do coroutine.close(cs[i]) end cs = nil", "=drop").unwrap();
        lua.collect_garbage().unwrap();
        lua.collect_garbage().unwrap();
        assert!(lua.memory_used() <= base + 4096, "fast {fast}: {} after, {base} before", lua.memory_used());
    }
    // Deep recursion gives the same results, fuel and memory with the fast loop on and off.
    let p = "local function f(n) if n == 0 then return collectgarbage('count') end local x = f(n - 1) return x end \
             local co = coroutine.wrap(function() return f(1500) end) local a = co() local b = f(1700) return a > 0, b > 0";
    assert_eq!(super::common::remedy(p, 1 << 30), Ok("true true".to_owned()));
}


/// What held: weak tables, ephemerons and coroutines give C Lua's results with a whole
/// collection at every allocation (`set_gc_stress`), on a 64 KiB native stack, with the
/// fast loop on and off; long ephemeron chains through many tables and cycles through
/// other ephemerons are collected, and memory goes back to where it was.
#[test]
fn s5gc_held_weak_tables_and_coroutines_under_stress() {
    for (name, p) in [("weak", WEAK_PROGRAM), ("coroutines", COROUTINE_PROGRAM)] {
        let theirs = super::common::oracle(p, 1 << 30);
        for fast in [true, false] {
            let ours = super::common::small_stack(move || {
                let mut lua = Lua::new(Limits::default().hash_seed(super::common::TEST_SEED), Libs::ALL);
                lua.set_fast_loop(fast);
                lua.set_gc_stress(true);
                match lua.exec(super::common::wrap(p), "=wrapper") {
                    Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
                    Err(e) => Err(e.to_string()),
                }
            });
            assert_eq!(ours, theirs, "{name}, fast {fast}");
        }
    }
    // Ephemeron chains spread over 64 tables, each link's value holding the next key and
    // its own key, closed into a ring back to the root: all of it goes once the roots do.
    let chains = "local ws = {} for j = 1, 64 do ws[j] = setmetatable({}, {__mode = 'k'}) end \
                  roots = {} for r = 1, 20 do local first = {} roots[r] = first local prev = first \
                    for i = 1, 2000 do local k = {} ws[(r * 7 + i) % 64 + 1][prev] = {k, prev} prev = k end \
                    ws[r][prev] = first end \
                  collectgarbage() local n = 0 for j = 1, 64 do for _ in pairs(ws[j]) do n = n + 1 end end \
                  roots = nil collectgarbage() collectgarbage() \
                  local m = 0 for j = 1, 64 do for _ in pairs(ws[j]) do m = m + 1 end end ws = nil return n, m";
    for fast in [true, false] {
        let (r, grew) = super::common::small_stack(move || {
            let mut lua = Lua::new(Limits::default().memory(256 << 20), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.collect_garbage().unwrap();
            let base = lua.memory_used();
            let r = lua.exec(chains, "=chains").map(|v| v.iter().map(|x| super::common::show(&lua, *x)).collect::<Vec<_>>());
            lua.collect_garbage().unwrap();
            (r, lua.memory_used().saturating_sub(base))
        });
        assert_eq!(r.unwrap(), ["40020", "0"], "fast {fast}");
        assert!(grew < 64 << 10, "fast {fast}: {grew} bytes kept");
    }
}

/// Weak tables and ephemerons (weak keys, values, both; ephemeron cycles and chains;
/// strings never collected; a table that is its own key and its own weak metatable;
/// `__mode` changed after creation; collection during traversal; coroutines as keys).
const WEAK_PROGRAM: &str = r##"-- weak tables / ephemerons under stress: results must match C Lua
local out = {}
local function count(t) local c = 0 for _ in pairs(t) do c = c + 1 end return c end

-- 1. weak keys with self-referencing values (ephemeron: value refers to key)
local w = setmetatable({}, {__mode = 'k'})
for i = 1, 100 do local k = {} w[k] = {k} end
collectgarbage() collectgarbage()
out[#out + 1] = count(w)

-- 2. ephemeron cycle through two tables
local a = setmetatable({}, {__mode = 'k'})
local b = setmetatable({}, {__mode = 'k'})
do
  local k1, k2 = {}, {}
  a[k1] = k2 b[k2] = k1
end
collectgarbage()
out[#out + 1] = count(a) + count(b)

-- 3. a key kept by a strong ref, chain through ephemerons
local root = {}
local e = setmetatable({}, {__mode = 'k'})
local prev = root
for i = 1, 50 do local n = {} e[prev] = n prev = n end
collectgarbage()
out[#out + 1] = count(e)
root = nil
collectgarbage()
out[#out + 1] = count(e)

-- 4. weak values, strings never collected
local wv = setmetatable({}, {__mode = 'v'})
wv[1] = 'str' wv[2] = {} wv[3] = 'x' .. 1 wv.x = function() end wv[4] = 4
collectgarbage()
out[#out + 1] = tostring(wv[1]) .. tostring(wv[2]) .. tostring(wv[3]) .. tostring(wv.x) .. tostring(wv[4])

-- 5. weak table that is its own key and metatable
local s = {}
s.__mode = 'k'
setmetatable(s, s)
s[s] = s
collectgarbage()
out[#out + 1] = tostring(s[s] == s)

-- 6. __mode change after creation
local m = {__mode = 'k'}
local t6 = setmetatable({}, m)
t6[{}] = 1
m.__mode = nil
t6[{}] = 2
collectgarbage()
out[#out + 1] = count(t6)

-- 7. weak keys and values (all-weak) with keys kept
local kv = setmetatable({}, {__mode = 'kv'})
local keepk = {}
kv[keepk] = {} kv[{}] = keepk kv[1] = keepk
collectgarbage()
out[#out + 1] = count(kv)

-- 8. next after removal during traversal
local nt = setmetatable({}, {__mode = 'v'})
local hold = {}
for i = 1, 20 do local v = {} nt[i] = v if i % 2 == 1 then hold[i] = v end end
local seen = 0
for k, v in pairs(nt) do seen = seen + 1 collectgarbage() end
out[#out + 1] = seen .. ':' .. count(nt)

-- 9. coroutines as weak keys
local cw = setmetatable({}, {__mode = 'k'})
local co = coroutine.create(function() coroutine.yield() end)
coroutine.resume(co)
cw[co] = true
cw[coroutine.create(print)] = true
collectgarbage()
out[#out + 1] = count(cw)

-- 10. ephemeron where value references key via closure
local ce = setmetatable({}, {__mode = 'k'})
for i = 1, 10 do local k = {} ce[k] = function() return k end end
collectgarbage()
out[#out + 1] = count(ce)

-- 11. a weak-valued table as a metatable's __index cache
local cache = setmetatable({}, {__mode = 'v'})
local obj = setmetatable({}, {__index = cache})
cache.f = {}
collectgarbage()
out[#out + 1] = tostring(obj.f)

return table.concat(out, ' ')
"##;

/// Coroutines: yields across pcall, metamethods, `__close`, `__pairs` and (an error,
/// as in C Lua) `table.sort`, `gsub` and `load` readers; resuming itself, closing
/// running, normal, dead and suspended coroutines with failing or yielding `__close`,
/// errors in error handlers, nested `wrap`.
const COROUTINE_PROGRAM: &str = r##"local out = {}
local function p(...) local t = table.pack(...) for i = 1, t.n do t[i] = tostring(t[i]) end out[#out + 1] = table.concat(t, ',') end

-- yields across various boundaries
local co = coroutine.wrap(function()
  local ok, e = pcall(function() coroutine.yield(1) return 2 end)
  p('pcall', ok, e)
  local t = setmetatable({}, {__index = function(_, k) return coroutine.yield(k) end})
  p('index', t.foo)
  local ok2, e2 = pcall(table.sort, {3, 2, 1}, function(a, b) coroutine.yield('s') return a < b end)
  p('sort', ok2, e2)
  local ok3, e3 = pcall(string.gsub, 'ab', '.', function(c) coroutine.yield(c) end)
  p('gsub', ok3, e3)
  for k in pairs(setmetatable({}, {__pairs = function(t) coroutine.yield('pairs') return next, {7}, nil end})) do p('pk', k) end
  do
    local x <close> = setmetatable({}, {__close = function() coroutine.yield('close') end})
  end
  p('after close')
  local ok4, e4 = pcall(load, function() coroutine.yield('load') end)
  p('load', ok4, e4)
  return 'end'
end)
for i = 1, 12 do local r = co(i) p('got', r) if r == 'end' then break end end

-- resume self, running, status
local selfco
selfco = coroutine.create(function() return coroutine.resume(selfco) end)
p('self', coroutine.resume(selfco))
p('main', select(2, coroutine.running()))
p('isyield', coroutine.isyieldable())
p('close main', pcall(coroutine.close, coroutine.running()))

-- close with pending __close that errors
local c2 = coroutine.create(function()
  local a <close> = setmetatable({}, {__close = function(_, e) error('c1') end})
  local b <close> = setmetatable({}, {__close = function(_, e) error('c2') end})
  coroutine.yield()
end)
coroutine.resume(c2)
p('close err', coroutine.close(c2))
p('status', coroutine.status(c2))
p('close again', coroutine.close(c2))

-- close with pending __close that yields
local c3 = coroutine.create(function()
  local a <close> = setmetatable({}, {__close = function() coroutine.yield('y') end})
  coroutine.yield()
end)
coroutine.resume(c3)
p('close yield', pcall(coroutine.close, c3))
p('st3', coroutine.status(c3))

-- error in error handler
p('xpcall', xpcall(function() error('a') end, function(m) error('b') end))
p('xpcall2', xpcall(function() error('a') end, function(m) local x = nil; return x.y end))

-- deep wrap nesting
local function nest(n) if n == 0 then return coroutine.yield(0) end local w = coroutine.wrap(function() return nest(n - 1) end) return w() end
local cn = coroutine.wrap(function() return nest(150) end)
p('nest', pcall(cn))

-- resume dead, wrap dead
local cd = coroutine.wrap(function() return 1 end)
cd()
p('dead', pcall(cd))

-- yield from main
p('yield main', pcall(coroutine.yield, 1))

-- resume normal coroutine
local outer
outer = coroutine.create(function()
  local inner = coroutine.create(function() return coroutine.resume(outer) end)
  return coroutine.resume(inner)
end)
p('normal', coroutine.resume(outer))

-- close running
local cr = coroutine.create(function() return pcall(coroutine.close, coroutine.running()) end)
p('close running', coroutine.resume(cr))

-- close normal
local cnorm
cnorm = coroutine.create(function()
  local inner = coroutine.create(function() return pcall(coroutine.close, cnorm) end)
  return coroutine.resume(inner)
end)
p('close normal', coroutine.resume(cnorm))

-- error object kept on dead coroutine, traceback of dead
local ce = coroutine.create(function() error({code = 1}) end)
local ok, e = coroutine.resume(ce)
p('errobj', ok, type(e), e.code)

-- collected while suspended with a pending to-be-closed variable: never closed
local closed = 0
for i = 1, 20 do
  local c = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() closed = closed + 1 end}) coroutine.yield() end)
  coroutine.resume(c)
end
collectgarbage() collectgarbage()
p('tbc collected', closed)

return table.concat(out, '\n')
"##;
