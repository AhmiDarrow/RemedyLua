//! The table fast paths keep results as C Lua has
//! them, and results, fuel and memory exactly as the full path has them, at every budget.

use super::common::{oracle, remedy, remedy_measured};

const FUEL: u64 = 2_000_000_000;

/// The same results as C Lua, and the same results, fuel and memory with and without the
/// fast loop (`common::remedy`).
fn same_as_c(program: &str) {
    let ours = remedy(program, FUEL);
    assert_eq!(ours, oracle(program, FUEL), "{program}");
    assert!(ours.is_ok(), "{ours:?}");
}

/// Every fuel budget up to what the program needs stops the fast loop and the full path at
/// the same step, with the same results and memory (a fast path that charged its steps
/// after the work, or skipped a check the full path makes, stops elsewhere).
fn same_at_every_budget(program: &str) {
    let need = remedy_measured(program, FUEL, true).1;
    assert!(need < 60_000, "keep the program small: {need} steps");
    // Every budget for small programs; for the bigger ones a stride that is no multiple of
    // a loop's steps.
    let stride = if need <= 8_000 { 1 } else { 7 };
    for fuel in (1..=need + 1).rev().step_by(stride) {
        let fast = remedy_measured(program, fuel, true);
        let slow = remedy_measured(program, fuel, false);
        assert_eq!(fast, slow, "budget {fuel} of {need}:\n{program}");
    }
}

const INSERT_REMOVE: &[&str] = &[
    // Appends and pops through the array part as it grows and shrinks.
    "local t = {} for i = 1, 100 do table.insert(t, i) end local s = 0 for i = 1, 60 do s = s + table.remove(t) end return s, #t, t[40], t[41]",
    // Values the collector traces (the barrier), and nil (a border that stays).
    "local t = {} for i = 1, 40 do table.insert(t, {i}) table.insert(t, 'k' .. i) end table.insert(t, nil) return #t, t[80][1] == nil, t[79][1], t[80]",
    // Holes and the border hint: `#t` moves it, the slot written moves it again.
    "local t = {1, 2, 3, nil, 5} table.insert(t, 9) local a = #t table.insert(t, 10) return a, #t, t[4], t[6], t[7]",
    "local t = {} for i = 1, 17 do t[i] = i end t[17] = nil t[16] = nil table.insert(t, 'a') local r = {#t} t[8] = nil r[2] = #t table.insert(t, 'b') r[3] = #t r[4] = table.remove(t) r[5] = #t return table.unpack(r)",
    "local t = {} for i = 1, 33 do t[i] = i end for i = 33, 20, -1 do t[i] = nil end local r = {} for i = 1, 5 do r[#r + 1] = table.remove(t) r[#r + 1] = #t end table.insert(t, 0) r[#r + 1] = #t return table.unpack(r)",
    // Result counts: none, one, more wanted, all of them.
    "local t = {1, 2, 3} local a, b = table.remove(t) local n1 = select('#', table.insert(t, 7)) local n2 = select('#', table.remove(t)) local all = {table.remove(t)} local c, d, e = table.insert(t, 8) return a, b, n1, n2, all[1], #all, c, d, e, #t",
    // Empty tables (`remove` looks at t[0]), and t[0] itself.
    "local t = {} local a = table.remove(t) local n = select('#', table.remove(t)) t[0] = 'z' local b = table.remove(t) return a, n, b, t[0], #t",
    // A hash part beside the array, integer keys past it, and float keys.
    "local t = {x = 1, y = 2} for i = 1, 20 do table.insert(t, i) end t[30] = 30 table.insert(t, 21) local r = table.remove(t) t[2.0] = 'two' return r, #t, t[2], t.x, t[30]",
    "local t = {} t[1] = 1 t[2] = 2 t[4] = 4 table.insert(t, 3) table.insert(t, 5) return #t, t[3], t[5], table.remove(t), #t",
    // Metatables: `__len`, `__index`, `__newindex` (the full path), and one added later.
    "local log = {} local t = setmetatable({}, {__newindex = function(t, k, v) log[#log + 1] = k rawset(t, k, v) end}) for i = 1, 5 do table.insert(t, i) end return #t, table.concat(log, ',')",
    "local t = setmetatable({1, 2, 3}, {__len = function() return 1 end}) table.insert(t, 'x') return t[2], rawlen(t), table.remove(t), t[2]",
    "local t = {} for i = 1, 5 do table.insert(t, i) end setmetatable(t, {__index = function() return 'mt' end}) table.insert(t, 6) return table.remove(t), table.remove(t), #t, t[9]",
    // The functions rebound, called through other names, with other argument counts.
    "local ins, rem = table.insert, table.remove local t = {} ins(t, 1) ins(t, 2) table.insert = function(t, v) rawset(t, #t + 1, v * 10) end table.insert(t, 3) local x = rem(t) table.insert = ins table.insert(t, 4) return x, #t, t[3]",
    "local t = {1, 2, 3} table.insert(t, 1, 0) table.insert(t, 3, 9) local a = table.remove(t, 1) local b = table.remove(t, #t + 1) return a, b, table.concat(t, ',')",
    "local t = {} local ok1, e1 = pcall(table.insert, t) local ok2, e2 = pcall(table.insert, t, 1, 2, 3) local ok3, e3 = pcall(table.insert, nil, 1) local ok4, e4 = pcall(table.remove, 7) return ok1, e1, ok2, e2, ok3, e3, ok4, e4",
    "local t = {} local function f(...) table.insert(...) end f(t, 'a') f(t, 'b') local g = function(...) return table.remove(...) end return g(t), g(t), g(t), #t",
    // Shifts: `insert` and `remove` at a position, `table.move` within and across tables.
    "local t = {} for i = 1, 30 do t[i] = 'v' .. i end table.insert(t, 1, 'first') table.insert(t, 15, {}) local a = table.remove(t, 1) local b = table.remove(t, 10) local c = table.remove(t, #t) return a, b, c, #t, t[1], t[14] ~= nil, table.concat(t, ',', 20)",
    "local q = {} for i = 1, 50 do q[#q + 1] = {i} end local s = 0 while #q > 0 do s = s + table.remove(q, 1)[1] if #q % 9 == 0 and #q > 0 then table.insert(q, 1, {100}) table.remove(q, 1) end end return s, #q",
    "local t = {1, 2, 3, nil, 5, 6} table.insert(t, 2, 'x') local r = table.remove(t, 2) return r, #t, t[4], t[5]",
    "local t = {} for i = 1, 20 do t[i] = i end for i = 20, 11, -1 do t[i] = nil end table.insert(t, 5, 'm') local a = #t local b = table.remove(t, 3) return a, b, #t, t[10], t[11]",
    "local a = {1, 2, 3, 4, 5, 6, 7, 8} table.move(a, 2, 6, 3) local b = table.move(a, 1, 8, 2, {}) local c = table.move({'p', 'q'}, 1, 2, 4, {x = 1}) table.move(a, 4, 8, 1) return table.concat(a, ','), table.concat(b, ',', 2), c[4], c[5], c.x, #c",
    "local src = setmetatable({}, {__index = function(t, k) return k * 2 end}) local dst = {} table.move(src, 1, 5, 1, dst) local log = {} local d2 = setmetatable({}, {__newindex = function(t, k, v) log[#log + 1] = k .. '=' .. v rawset(t, k, v) end}) table.move({7, 8, 9}, 1, 3, 2, d2) return table.concat(dst, ','), table.concat(log, ' ')",
    "local t = {x = 'h'} for i = 1, 10 do t[i] = i end t[12] = 12 table.insert(t, 3, 'i') table.move(t, 1, 12, 5) return #t > 0, t[3], t[7], t[16], t[17], table.remove(t, 1)",
    // Through the collector's steps: fresh objects appended to a table it may have marked.
    // (Single steps: `collectgarbage('step', 1)` does C's work for 1 KiB, which ends each
    // cycle of these small heaps, as in C; these test steps that stop mid-cycle.)
    "collectgarbage('incremental', 100, 100) local keep = {} for i = 1, 300 do table.insert(keep, {i, tostring(i)}) if i % 7 == 0 then collectgarbage('step') end if i % 11 == 0 then table.remove(keep) end end collectgarbage() local s = 0 for i = 1, #keep do s = s + keep[i][1] + #keep[i][2] end return #keep, s",
    "collectgarbage('incremental', 100, 100) local keep = {} for i = 1, 200 do table.insert(keep, 1, {i, tostring(i)}) if i % 5 == 0 then collectgarbage('step') table.remove(keep, 2) end end collectgarbage() local s = 0 for i = 1, #keep do s = s + keep[i][1] + #keep[i][2] end return #keep, s",
];

#[test]
fn table_insert_and_remove_in_place_are_the_library_calls() {
    for p in INSERT_REMOVE {
        same_as_c(p);
    }
}

#[test]
fn table_insert_and_remove_in_place_stop_where_the_full_path_stops() {
    let _heavy = super::common::memory_heavy();
    for p in INSERT_REMOVE {
        same_at_every_budget(p);
    }
}

/// A big array grown one key at a time (its buffer grows in place past the pool's sizes):
/// the same results and memory account with and without the fast loop, and as C Lua.
#[test]
fn big_arrays_grow_with_the_same_account() {
    same_as_c("local t = {} for i = 1, 300000 do t[i] = i end local u = {} for i = 1, 70000 do u[#u + 1] = i end return #t, t[300000], #u, collectgarbage('count') > 0");
    same_as_c("local t = {} for i = 1, 100 do t[i] = i end for i = 1, 100 do t[i] = nil end for i = 1, 5000 do t[i] = -i end return #t, t[5000]");
}

/// Reads and writes with constant integer keys (`t[1]`, `GetTableK`/`SetTableK`): the array
/// part read in place (the border hint moved as `get_int` moves it), the hash part, holes,
/// metatables and `__index` chains.
const CONST_KEYS: &[&str] = &[
    "local p = {3, 1, 2} local s = 0 for i = 1, 50 do p[2], p[1] = p[1], p[2] p[3] = p[1] + p[2] s = s + p[3] end return s, p[1], p[2], p[3]",
    "local t = {} for i = 1, 17 do t[i] = i end t[17] = nil t[16] = nil local a = #t local b = t[16] local c = t[15] local d = #t t[16] = 'x' return a, b, c, d, #t, t[16]",
    "local t = {1, 2, nil, 4} local r = {t[1], t[3], t[4], t[5], #t} t[3] = 3 r[6] = #t return table.unpack(r)",
    "local t = {x = 1} t[1] = 'a' t[2] = 'b' t[100] = 'c' t[-1] = 'n' t[0] = 'z' return t[1], t[2], t[3], t[100], t[-1], t[0], t[2.0]",
    "local base = {10, 20, 30} local t = setmetatable({}, {__index = base}) local u = setmetatable({5}, {__index = function(_, k) return k * 100 end}) return t[1], t[2], t[3], t[4], u[1], u[2], rawget(t, 1)",
    "local mt = {__newindex = function(t, k, v) rawset(t, k, v * 2) end} local t = setmetatable({1}, mt) t[1] = 7 t[2] = 8 return t[1], t[2]",
];

#[test]
fn constant_integer_keys_read_in_place() {
    for p in CONST_KEYS {
        same_as_c(p);
        same_at_every_budget(p);
    }
}

/// Constructors (`SetList` into the array part `NewTable` sized): fixed lists, lists ending
/// in a call or `...`, more items than one `SetList` flushes, nils, nesting, and fresh
/// objects stored while the collector runs.
const CONSTRUCTORS: &[&str] = &[
    "local s = 0 for i = 1, 200 do local t = {i, i * 2, {x = i}, 'k' .. i} s = s + t[1] + t[2] + t[3].x + #t[4] end return s",
    "local function f(...) return ... end local a = {f(1, 2, 3)} local b = {0, f(4, 5)} local c = {f()} local d = {f(nil, nil)} local function g(...) return {...}, select('#', ...) end local e, n = g(1, nil, 3) return #a, #b, b[3], #c, #d, n, e[3]",
    "local t = {1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54, 55, 56} local u = {nil, nil, 3, nil} return #t, t[51], t[56], u[3], #u",
    "local function many() local r = {} for i = 1, 300 do r[i] = i end return table.unpack(r) end local t = {many()} local u = {1, 2, many()} return #t, t[300], #u, u[302]",
    "local t = {x = 1, 10, y = 2, 20, [3] = 'three', 30} return t[1], t[2], t[3], t.x, t.y, #t",
    "local function f(...) return ... end local a = {[2] = 'h', f(1)} local b = {[3] = 'h', f(1, 2)} local c = {x = 1, f(1, 2, 3)} local d = {[4] = 4, 1, f(2, 3)} return a[1], a[2], b[3], #b, c[3], c.x, d[4], #d",
    "collectgarbage('incremental', 100, 100) local keep = {} for i = 1, 300 do keep[i % 50 + 1] = {tostring(i), {i}, i} if i % 3 == 0 then collectgarbage('step') end end collectgarbage() local s = 0 for i = 1, 50 do s = s + #keep[i][1] + keep[i][2][1] + keep[i][3] end return s",
];

#[test]
fn constructors_fill_in_place() {
    let _heavy = super::common::memory_heavy();
    for p in CONSTRUCTORS {
        same_as_c(p);
        same_at_every_budget(p);
    }
}

/// The table shapes `sweep3_sec::fuel_tracks_time` draws (`rand_tab`), of `n` keys.
fn shapes(n: usize) -> [String; 5] {
    [
        format!("t = {{}} for i = 1, {n} do t[i] = i end"),
        format!("t = {{}} for i = 1, {n} do t[i] = tostring(i) end"),
        format!("t = {{}} for i = 1, {n} do t['k' .. i] = i end"),
        format!("t = {{}} for i = 1, {n} do t[i * 2] = i * 0.5 end"),
        format!("t = {{}} for i = 1, {n} do t[i] = -i end for i = 1, {n}, 3 do t[i] = nil end"),
    ]
}

/// Probe (not a test): `table.move` and the shifts over a million keys, per shape, as ratios
/// to plain code. `cargo test -p remedy-lua --test all perf_tables::probe_moves -- --ignored --nocapture`
#[test]
#[ignore = "probe"]
fn probe_moves() {
    super::common::small_stack(|| {
        let n = 1 << 20;
        for fast in [true, false] {
            let base = super::sweep5b_open::baseline(fast);
            for (i, setup) in shapes(n).iter().enumerate() {
                for body in [format!("table.move(t, 1, {n}, 2)"), "table.insert(t, 1, 0) table.remove(t, 1)".to_owned()] {
                    let r = (0..2).map(|_| super::sweep5b_open::ns_per_unit(setup, &body, 3_000_000, fast) / base).fold(f64::MAX, f64::min);
                    println!("fast {fast} shape {i}: {r:.1}x  {body}");
                }
            }
        }
    });
}

/// Integer keys looked up in a big part of other keys (`Table::int_in_nodes`): a
/// `table.move` of a million integer keys over a table of string keys ran at 8.3x plain code
/// per unit of fuel (7.3x on the full path); each lookup in a part of 2^20 slots or more now
/// counts three probe steps (two read 4.1x under load against the 4x of
/// `moves_over_a_big_part_of_other_keys_cost_what_they_are_charged`). Deterministic: the fuel of a move over such a table, the same
/// under every seed and on both paths.
#[test]
fn integer_keys_in_a_big_part_of_other_keys_are_paid() {
    let _heavy = super::common::memory_heavy();
    super::common::small_stack(|| {
        // The fuel of `body` alone, over a table of `n` string keys built first.
        let fuel_of = |n: usize, seed: u64, fast: bool| {
            let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(format!("t = {{}} for i = 1, {n} do t['k' .. i] = i end collectgarbage()"), "=setup").unwrap();
            let body = "table.move(t, 1, 100000, 2) local s = 0 for i = 1, 100000 do if t[i] == nil then s = s + 1 end end return s";
            let f = lua.load(body, "=body").unwrap();
            // (Each call from the host gets the whole budget.)
            let out = format!("{:?}", lua.call(f.value(), &[]).unwrap().first().map(|v| lua.to_string_lossy(*v)));
            (out, lua.limits().fuel - lua.fuel_left())
        };
        let big = fuel_of(800_000, super::common::TEST_SEED, true);
        for seed in [super::common::TEST_SEED, 0, 1, 0xdead_beef] {
            for fast in [true, false] {
                assert_eq!(fuel_of(800_000, seed, fast), big, "seed {seed:#x} fast {fast}");
            }
        }
        // In a part of 2^20 slots: three steps for each of the 300,000 lookups (a read and a
        // write per element moved, a read per element after) against a part in the caches.
        let small = fuel_of(1_000, super::common::TEST_SEED, true);
        assert_eq!(big.0, small.0);
        assert!(big.1 >= small.1 + 800_000, "big {big:?}, small {small:?}");
    });
}

/// The same moves, as ratios to plain code: about 4x at most with margin (1.4 to 2.2x here),
/// against the 10x of `sweep3_sec::fuel_tracks_time`.
#[test]
fn moves_over_a_big_part_of_other_keys_cost_what_they_are_charged() {
    super::common::small_stack(|| {
        let n = 1 << 20;
        let setup = format!("t = {{}} for i = 1, {n} do t['k' .. i] = i end");
        for fast in [true, false] {
            let base = super::sweep5b_open::baseline(fast);
            let r = (0..3).map(|_| super::sweep5b_open::ns_per_unit(&setup, &format!("table.move(t, 1, {n}, 2)"), 3_000_000, fast) / base).fold(f64::MAX, f64::min);
            println!("fast {fast}: {r:.1}x");
            assert!(r < 4.0, "fast {fast}: table.move over string keys at {r:.1}x plain code per unit");
        }
    });
}

/// `table.sort` reading and writing slots in place (`InPlace::get`/`set2`): integers,
/// strings, a comparator, holes and the border hint, an `__index`/`__newindex` table, and an
/// invalid order function.
const SORTS: &[&str] = &[
    "local t, x = {}, 7 for i = 1, 300 do x = (x * 16807) % 2147483647 t[i] = x % 1000 end table.sort(t) local ok = true for i = 2, #t do ok = ok and t[i - 1] <= t[i] end return ok, t[1], t[300]",
    "local t = {} for i = 1, 200 do t[i] = 's' .. (i * 7919 % 200) end table.sort(t) table.sort(t, function(a, b) return a > b end) return t[1], t[200], #t",
    "local t = {} for i = 1, 40 do t[i] = 41 - i end t[41] = nil t[60] = 5 table.sort(t) return t[1], t[40], t[60], #t",
    "local log = 0 local base = {} for i = 1, 30 do base[i] = (i * 13) % 31 end local t = setmetatable({}, {__index = base, __newindex = function(t, k, v) log = log + 1 rawset(t, k, v) end}) table.sort(t, function(a, b) return a < b end) return log, rawget(t, 1), rawget(t, 30)",
    "local t = {} for i = 1, 50 do t[i] = i % 7 end local ok, e = pcall(table.sort, t, function(a, b) return true end) return ok, e",
    "local t = {} for i = 1, 60 do t[i] = {v = (i * 37) % 61} end table.sort(t, function(a, b) return a.v < b.v end) return t[1].v, t[60].v",
];

#[test]
fn sorts_read_and_write_in_place() {
    for p in SORTS {
        same_as_c(p);
        same_at_every_budget(p);
    }
}

/// `table.pack` copying its arguments into the array part made for them: none, nils,
/// collectable values while the collector runs, and many.
const PACKS: &[&str] = &[
    "local p, q, r = table.pack(), table.pack(nil), table.pack(1, nil, 3, nil) return p.n, #p, q.n, r.n, r[3], r[4]",
    "local function f(...) return table.pack(...) end local p = f(1, 2, 3) local big = {} for i = 1, 300 do big[i] = i end local q = f(table.unpack(big)) return p.n, p[3], q.n, q[300]",
    "collectgarbage('incremental', 100, 100) local keep = {} for i = 1, 160 do keep[i % 40 + 1] = table.pack({i}, 'k' .. i, i) if i % 5 == 0 then collectgarbage('step', 1) end end collectgarbage() local s = 0 for i = 1, 40 do s = s + keep[i][1][1] + #keep[i][2] + keep[i].n end return s",
];

#[test]
fn packs_copy_their_arguments_in_place() {
    for p in PACKS {
        same_as_c(p);
        same_at_every_budget(p);
    }
}

/// `utf8.codes` returns the same two iterator functions every call (C Lua's light C
/// functions `iter_auxstrict` and `iter_auxlax`), made once: equal across calls, and a
/// `for` over them as C Lua runs it, invalid sequences and the lax mode included.
const UTF8_CODES: &[&str] = &[
    "return utf8.codes('a') == utf8.codes('b'), utf8.codes('a', true) == utf8.codes('b'), utf8.codes('a', true) == utf8.codes('b', true)",
    "local n = 0 for p, c in utf8.codes('h\\u{e9}llo \\u{4e16}\\u{754c}') do n = n + p * 7 + c end return n",
    "local r = {} for p, c in utf8.codes('\\u{10FFFF}a', true) do r[#r + 1] = p .. ':' .. c end return table.concat(r, ',')",
    "return pcall(function() local n = 0 for p, c in utf8.codes('ab\\xff') do n = n + c end return n end)",
    "return pcall(function() for p, c in utf8.codes('\\x80') do end end)",
    "local f, s, i = utf8.codes('xyz') return f(s, i), f(s, 1), f(s, 3), select('#', f(s, 3))",
    // The generic `for` runs the iterators in place: a skipped run of continuations
    // (charged), lax and strict codes, a negative and a float control, one and three
    // variables, a string's and another state.
    "local f = utf8.codes('') local s = 'ab' .. ('\\x80'):rep(40) .. 'z\\u{e9}' local r = {} for p, c in f, s, 2 do r[#r + 1] = p .. ':' .. c end return table.concat(r, ',')",
    "local f = utf8.codes('', true) local n = 0 for p, c in f, '\\u{7FFFFFFF}\\u{D800}x', 0 do n = n + c end return n, pcall(function() for p, c in utf8.codes('\\u{D800}') do end end)",
    "local f = utf8.codes('') local r = {} for p in f, 'abc', -1 do r[#r + 1] = p end for p in f, 'abc', 1.0 do r[#r + 1] = p end return #r, r[1]",
    "local r = 0 for p, c, x in utf8.codes('\\u{4e16}a') do r = r + p + c + (x or 0) end return r, pcall(function() for p in utf8.codes('a'), {} do end end)",
];

#[test]
fn utf8_codes_iterators_are_made_once() {
    for p in UTF8_CODES {
        same_as_c(p);
        same_at_every_budget(p);
    }
}

/// LR-32 (long property test, seed 30028): `pairs` over a million string keys ran at 10.7 to
/// 14.5x plain code per unit of fuel (bound 10x), 13x in the probe here: each `next` found
/// its key again by hashing it, a cache miss in a 32 MB part a step, and the fast loop and
/// the library's `next` read the key's string object first. `next` now looks first at the
/// node it last returned (`Table::next_hint`, self-checking), where the key is the table's
/// own. Under 5x on both paths (1.2x and 2.9x here); fuel unchanged, the same on both paths
/// and under every seed; traversals that clear or change what they visit, or walk another
/// table between steps, give C Lua's results.
#[test]
fn lr32_pairs_over_a_big_part_finds_its_place_cheaply() {
    super::common::small_stack(|| {
        let n = 1 << 20;
        let setup = format!("t = {{}} for i = 1, {n} do t['k' .. i] = i end");
        for fast in [true, false] {
            let base = super::sweep5b_open::baseline(fast);
            let r = (0..3).map(|_| super::sweep5b_open::ns_per_unit(&setup, "for _ in pairs(t) do end", 20_000_000, fast) / base).fold(f64::MAX, f64::min);
            assert!(r < 5.0, "fast {fast}: pairs over {n} string keys at {r:.1}x plain code");
        }
        let mut fuel = Vec::new();
        for (fast, seed) in [(true, 1u64), (false, 1), (true, 7), (true, 0xdead_beef)] {
            let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default().memory(512 << 20).fuel(1 << 40).hash_seed(seed), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(&setup, "=setup").unwrap();
            // (Each call from the host gets the whole budget: what this one used.)
            let r = lua.exec("local c = 0 for k, v in pairs(t) do c = c + v end return c", "=walk").unwrap();
            assert_eq!(r, [remedy_lua::Value::Int((n as i64) * (n as i64 + 1) / 2)]);
            fuel.push(lua.limits().fuel - lua.fuel_left());
        }
        assert!(fuel.iter().all(|&f| f == fuel[0]), "fuel differs between paths or seeds: {fuel:?}");
    });
    for p in [
        "local t = {} for i = 1, 3000 do t['k' .. i] = i end local s = 0 \
         for k, v in pairs(t) do s = s + v t[k] = nil end return s, next(t)",
        "local t = {} for i = 1, 3000 do t['k' .. i] = i end local s = 0 \
         for k, v in pairs(t) do s = s + v t[k] = v * 2 end local u = 0 for _, v in pairs(t) do u = u + v end return s, u",
        "local a, b = {}, {} for i = 1, 500 do a['a' .. i] = i b['b' .. i] = i end local s, k1, k2 = 0, nil, nil \
         repeat k1 = next(a, k1) k2 = next(b, k2) if k1 then s = s + a[k1] end if k2 then s = s + b[k2] end until not k1 and not k2 return s",
    ] {
        same_as_c(p);
    }
}
