//! Sweep 6 review: regressions for what the review of sweep 6's changes found.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Libs, Limits, Lua};

/// One call of `body` after `setup` (hash seed `seed`, fast loop `fast`) under a clock that
/// counts its reads (each a look at the deadline): the reads, the fuel and the result.
fn deadline_reads(setup: &str, body: &str, seed: u64, fast: bool) -> (usize, u64, String) {
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

/// S6R-4 (low, LR-8): a finished or closed coroutine's stack kept for the next one
/// (`spare_stacks`) left the memory account (`release`) as it went into the slack, which
/// is part of the account: up to 8 stacks of 2,048 slots (a sixteenth of the cap at most)
/// held and not counted, and the live bytes the collector paces on short by twice that.
/// Finishing 8 coroutines took 37,376 bytes off `memory_used`, their frame records
/// (1,408) and their stacks; now their frame records only.
#[test]
fn s6r_4_spare_coroutine_stacks_stay_counted() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    lua.exec("collectgarbage('stop') C = {} for i = 1, 8 do C[i] = coroutine.wrap(function() coroutine.yield() return 1 end) C[i]() end", "=s").unwrap();
    let before = lua.memory_used();
    lua.exec("for i = 1, 8 do C[i]() end", "=f").unwrap();
    let drop = before.saturating_sub(lua.memory_used());
    assert!(drop <= 8 * 1024, "finishing 8 coroutines took {drop} bytes off the account, their stacks kept");
    // Reused by the next ones, and given back by a full collection.
    lua.exec("for i = 1, 8 do C[i] = coroutine.wrap(function() return 1 end) C[i]() end C = nil collectgarbage()", "=g").unwrap();
}

/// S6R-3 (low, S6T-KEY): the lookups natives make through `index_nested` and
/// `set_index_nested` did not settle a long key first, so a 16 MiB key was still hashed in
/// one go deep in the interner (6.4 ms past a 1 ms limit, growing with `string_len`):
/// `string.gsub` with a table replacement looks each capture up (a fresh string, never
/// interned), `require` its name in `package.loaded`. Before: 259 deadline reads, as many
/// as `'%1'` makes; `R[c]` in a function, 274. Now a look every megabyte there too, with
/// the fuel the same with the fast loop on and off and under every seed.
#[test]
fn s6r_3_native_lookups_settle_long_keys() {
    let _heavy = super::common::memory_heavy();
    let setup = "L = string.rep('a', 16 << 20) R = {} P = setmetatable({}, {__index = R})";
    // The match alone (an empty replacement copies nothing; `'%1'` now copies a megabyte
    // between looks too, LR-29).
    let plain = deadline_reads(setup, "return (string.gsub(L, '^(.*)$', ''))", 1, true).0;
    // (`P`'s miss goes on to `R`: through `__index`.)
    for body in ["return (string.gsub(L, '^(.*)$', R))", "return (string.gsub(L, '^(.*)$', P))"] {
        let first = deadline_reads(setup, body, 1, true);
        assert!(first.0 >= plain + 15, "{body}: {} deadline reads, {plain} for the match alone", first.0);
        let full = deadline_reads(setup, body, 1, false);
        assert_eq!((full.1, &full.2), (first.1, &first.2), "{body}: fast loop off");
        for seed in [0, 7, u64::MAX] {
            let other = deadline_reads(setup, body, seed, true);
            assert_eq!((other.1, &other.2), (first.1, &first.2), "{body}: seed {seed:#x}");
        }
    }
}

/// The interner's hash (`heap::hash_bytes`) as a script that knows the seed can compute
/// it: the state after the start and `words` (whole 8-byte words), and the key words.
fn hash_state(seed: u64, len: usize, words: &[u8]) -> u64 {
    let fold = |a: u64, b: u64| {
        let r = u128::from(a) * u128::from(b);
        (r as u64) ^ ((r >> 64) as u64)
    };
    let (k1, k2) = keys(seed);
    let mut h = seed ^ (len as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    for c in words.chunks_exact(8) {
        h = fold(h ^ u64::from_le_bytes(c.try_into().unwrap()) ^ k1, k2);
    }
    h
}

fn keys(seed: u64) -> (u64, u64) {
    (seed ^ 0xa076_1d64_78bd_642f, seed.rotate_left(32) ^ 0xe703_7ed1_a0b4_28db)
}

/// The heap's seed for host seed `base` (`heap::mix_seed(base, 1)`).
fn heap_seed(base: u64) -> u64 {
    let mut h = base ^ 0x9E37_79B9_7F4A_7C15;
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// S6R-5 (high, S6T-EQ): the crit-bit tree that tells apart interned
/// strings of one whole hash and length (`Crit`) is boxed node in node, and was dropped
/// the default way, a call per level. With the seed known a script makes strings of one
/// hash whose tree is a chain (string `i` is zeros but bit `i`, then two words that cancel
/// the hash out), as deep as it has strings; dropping the state with 4,000 of them, 2 MiB,
/// overflowed the native stack. Freed a node at a time now, on a 64 KiB stack.
/// Setup code making `TAILS` and `T`: `n` strings of one whole hash and length under host
/// seed `base` whose crit-bit tree is a chain (string `i` is zeros but bit `i`, then two
/// words that cancel the hash out), each a key of `T` (`T[key] = i`); `KEY(i)` makes string
/// `i` afresh.
fn crit_chain(base: u64, n: usize) -> String {
    const PREFIX: usize = 512;
    let seed = heap_seed(base);
    let (k1, k2) = keys(seed);
    let fold = |a: u64, b: u64| {
        let r = u128::from(a) * u128::from(b);
        (r as u64) ^ ((r >> 64) as u64)
    };
    // Each string's last 16 bytes, as Lua escapes.
    let mut tails = String::from("TAILS = {");
    for i in 0..n {
        let mut p = vec![0u8; PREFIX];
        p[i / 8] = 1 << (i % 8);
        let w1 = 0x4141_4141_4141_4141u64;
        let h1 = fold(hash_state(seed, PREFIX + 16, &p) ^ w1 ^ k1, k2);
        let w2 = h1 ^ 0x1234_5678_9abc_def0;
        tails.push('"');
        for b in w1.to_le_bytes().iter().chain(&w2.to_le_bytes()) {
            tails.push_str(&format!("\\x{b:02x}"));
        }
        tails.push_str("\",");
    }
    tails.push('}');
    tails + " function KEY(i) local at = i // 8 \
              return string.rep('\\0', at) .. string.char(1 << (i % 8)) .. string.rep('\\0', 511 - at) .. TAILS[i + 1] end \
              T = {} for i = 0, #TAILS - 1 do T[KEY(i)] = i end"
}

#[test]
fn s6r_5_a_deep_crit_tree_drops_on_a_small_stack() {
    const N: usize = 4000;
    let base = 0x5eed_u64;
    let tails = crit_chain(base, N);
    super::common::small_stack(move || {
        let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(base), Libs::ALL);
        lua.exec(&tails, "=tails").unwrap();
        let r = lua.exec("local n = 0 for k, v in pairs(T) do n = n + 1 end return n", "=count").unwrap();
        assert_eq!(format!("{r:?}"), format!("[Int({N})]"));
        drop(lua);
    });
}

/// Fuel of 100 lookups of a fresh copy of the deepest string of `crit_chain(base, n)`.
fn deep_lookups_fuel(n: usize, fast: bool) -> u64 {
    let base = 0x5eed_u64;
    let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(base), Libs::ALL);
    lua.set_fast_loop(fast);
    lua.exec(&crit_chain(base, n), "=chain").unwrap();
    let r = lua.exec(&format!("local c = 0 for j = 1, 100 do if T[KEY({})] then c = c + 1 end end return c", n - 1), "=look").unwrap();
    assert_eq!(format!("{r:?}"), "[Int(100)]");
    (1u64 << 40) - lua.fuel_left()
}

/// S6R-5's other half: a lookup among strings of one whole hash and
/// length walks their crit-bit tree, a chain as deep as they are when the seed is known,
/// and the slots' walk, which charged each string of the same hash bits, stops at the
/// first now: 4,000 nodes a lookup, free. Each node walked (looking up, interning, taking
/// out) is a walk's step now: a lookup at the foot of a chain of 4,000 costs a unit a
/// node more than one in a chain of 2, the same with the fast loop on and off.
#[test]
fn s6r_5_crit_walks_are_paid() {
    super::common::small_stack(|| {
        let (deep, shallow) = (deep_lookups_fuel(4000, true), deep_lookups_fuel(2, true));
        println!("100 lookups: {deep} fuel at depth 4,000, {shallow} at depth 1");
        assert!(deep >= shallow + 100 * 3900, "100 lookups at depth 4,000: {deep} fuel, {shallow} at depth 1");
        assert_eq!(deep_lookups_fuel(4000, false), deep, "fast loop off");
    });
}

/// S6R-6 (critical, S6-7): `on_error` counts C Lua's C levels where an error escaping
/// a message handler was raised by walking the frames of the thread and every coroutine
/// that resumed it (`c_levels`), once a handler round, uncharged: a handler that fails is
/// called again up to some 200 times, so `xpcall(error, function(e) error(e, 0) end)` 60
/// coroutines of 1,900 frames deep walked 200 times 114,000 frames for the fuel of one
/// walk, 157x plain code per unit of fuel (1.2x now). Each walk is charged as
/// `call_handler`'s is, a unit per 5 frames.
#[test]
fn s6r_6_a_failing_handler_pays_for_its_level_walks() {
    let fuel_of = |handler: &str| {
        let mut lua = Lua::new(Limits::default().fuel(1 << 40), Libs::ALL);
        let src = format!(
            "H = {handler} \
             function D(n, f) if n == 0 then return f() end local r = D(n - 1, f) return r end \
             function NEST(k) \
               if k == 0 then return (xpcall(error, H, 'x')) end \
               local c = coroutine.create(function() return D(1900, function() return NEST(k - 1) end) end) \
               local ok, r = coroutine.resume(c) return r \
             end \
             return NEST(60)"
        );
        lua.exec(&src, "=walk").unwrap();
        (1u64 << 40) - lua.fuel_left()
    };
    let caught = fuel_of("function(e) return e end");
    let failing = fuel_of("function(e) error(e, 0) end");
    assert!(failing > caught + 1_000_000, "a failing handler: {failing} fuel, {caught} with one that returns");
}

/// S6R-7 (critical, S6T-LOAD): a string's hash is kept with 0 meaning
/// "not computed yet", and `settle_long_key` hashed while it read 0: a long key whose hash
/// came out 0 was hashed again forever, never looking at the interrupt or the deadline.
/// With the seed known a script makes one: the 7-byte tail cancels the state to 1 before
/// the finish, whose multiply then folds to 0 (one try in 256 for the word before it).
/// `t[S] = 1` hung the host. No hash is 0 now.
#[test]
fn s6r_7_a_key_whose_hash_is_zero() {
    const W: usize = 131_073;
    let base = 0x5eed_u64;
    let seed = heap_seed(base);
    let (k1, k2) = keys(seed);
    let len = 8 * W + 7;
    let fold = |a: u64, b: u64| {
        let r = u128::from(a) * u128::from(b);
        (r as u64) ^ ((r >> 64) as u64)
    };
    let h0 = hash_state(seed, len, &vec![b'a'; 8 * (W - 1)]);
    let (j, t) = (0u64..).map(|j| (j, 1 ^ fold(h0 ^ j ^ k1, k2) ^ k1)).find(|&(_, t)| t >> 56 == 0).unwrap();
    // The finish (`heap::hash_finish`) of that state and tail: 0.
    assert_eq!(fold(fold(fold(h0 ^ j ^ k1, k2) ^ t ^ k1, k2) ^ k2, k1 | 1), 0);
    let esc = |b: &[u8]| b.iter().map(|x| format!("\\x{x:02x}")).collect::<String>();
    let src = format!(
        "local S = string.rep('a', {}) .. '{}' .. '{}' local t = {{}} t[S] = 1 return t[S] + #S",
        8 * (W - 1),
        esc(&j.to_le_bytes()),
        esc(&t.to_le_bytes()[..7])
    );
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(base), Libs::ALL);
        let _ = tx.send(format!("{:?}", lua.exec(&src, "=zero")));
    });
    let r = rx.recv_timeout(std::time::Duration::from_secs(30)).expect("t[S] = 1 for a key of hash 0 never returned");
    assert_eq!(r, format!("Ok([Int({})])", len + 1));
}

/// S6R-8 (medium, LR-29): gsub copies its replacement's captures a
/// piece at a time between looks at the stop, but a replacement's own text byte by byte
/// with none: `gsub('a', 'a', R)` with a 16 MiB `R` ran 41 ms with no look at the
/// interrupt or the deadline. Its runs of text are copied as the captures are now.
#[test]
fn s6r_8_gsub_literal_replacement_looks_at_the_deadline() {
    let _heavy = super::common::memory_heavy();
    let setup = "R = string.rep('x', (16 << 20) - 64)";
    let reads = deadline_reads(setup, "return #string.gsub('a', 'a', R)", 1, true);
    let short = deadline_reads(setup, "return #string.gsub('a', 'a', 'x')", 1, true);
    assert!(reads.0 >= short.0 + 15, "{} deadline reads, {} with a short replacement", reads.0, short.0);
    let full = deadline_reads(setup, "return #string.gsub('a', 'a', R)", 1, false);
    assert_eq!((full.1, &full.2), (reads.1, &reads.2), "fast loop off");
}

/// Sweep 6 review (low, S6T-EQ): the crit-bit trees of strings of one whole hash and
/// length (nodes of about 48 bytes a string, and the group's map entry) were outside the
/// memory account. The same 4,000 strings take at least 40 bytes a string more when they
/// collide (the seed known) than under another seed, where they make no tree, and what
/// they take is given back as they go.
#[test]
fn fx2_6_crit_nodes_are_charged() {
    const N: usize = 4000;
    let base = 0x5eed_u64;
    let used = |seed: u64| {
        let src = crit_chain(base, N);
        super::common::small_stack(move || {
            let mut lua = Lua::new(Limits::default().memory(256 << 20).fuel(1 << 40).hash_seed(seed), Libs::ALL);
            let mut round = || {
                lua.exec(&src, "=chain").unwrap();
                lua.exec("collectgarbage()", "=gc").unwrap();
                let full = lua.memory_used();
                lua.exec("T = nil collectgarbage() collectgarbage()", "=gc").unwrap();
                (full, lua.memory_used())
            };
            (round(), round())
        })
    };
    let (((tree, after), (_, again)), ((plain, plain_after), (_, plain_again))) = (used(base), used(base + 1));
    assert!(tree >= plain + N * 40, "colliding {tree} bytes, not colliding {plain}");
    // (What stays live after a round is the same either way: what the trees add to it
    // does not grow from round to round.)
    let (first, second) = (after.saturating_sub(plain_after), again.saturating_sub(plain_again));
    assert!(second <= first + 1024, "the trees' share after a round: {first} bytes, then {second}");
}
