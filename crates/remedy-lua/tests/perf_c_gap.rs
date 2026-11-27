//! Speed: the paths the speed work changed, checked against C Lua with the
//! fast loop on and off (same results, fuel and memory: `common::remedy`).

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

use crate::common::{TEST_SEED, oracle, remedy, small_stack};

fn same_as_c(program: &str) {
    let ours = remedy(program, 50_000_000);
    let theirs = oracle(program, 50_000_000);
    assert_eq!(ours, theirs, "differs from C Lua:\n{program}");
}

/// The fast loop's context is cached across frames (`Lua::ctx_epoch`): a host function
/// that lowers the call depth limit mid-run must still have it apply from the next
/// frame switch on, as when the context was made at every switch.
#[test]
fn limits_changed_inside_a_host_function_apply() {
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().hash_seed(TEST_SEED), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.register("lower", |lua: &mut Lua, _args: &[Value]| {
            let l = lua.limits().call_depth(40);
            lua.set_limits(l);
            Ok(vec![])
        })
        .unwrap();
        let src = r"
            local function deep(n) if n == 0 then return 0 end return 1 + deep(n - 1) end
            deep(300)                      -- the frame list has room for 300 now
            lower()
            local ok, err = pcall(deep, 100)  -- a frame switch, then 100 levels
            return ok, tostring(err)
        ";
        let r = lua.exec(src, "=t").unwrap();
        assert_eq!(
            r.first().copied().and_then(|v| v.as_bool()),
            Some(false),
            "fast loop {fast}: the lowered limit did not apply"
        );
        let msg = r.get(1).and_then(|v| lua.to_string_lossy(*v)).unwrap_or_default();
        assert!(msg.contains("stack overflow"), "fast loop {fast}: {msg}");
    }
}

/// Coroutine switches swap boxed thread states and reuse the results buffer: values of
/// every count go both ways, through `wrap`, `resume`, nested resumes and errors.
#[test]
fn coroutine_switches_carry_every_count_of_values() {
    same_as_c(
        r"
        local log = {}
        local function pack(...) return select('#', ...), ... end
        local co = coroutine.create(function(...)
          local n = select('#', ...)
          for i = 0, 20 do
            local got = table.pack(coroutine.yield(table.unpack({...}, 1, i)))
            n = n + got.n
          end
          return n, 'done'
        end)
        for i = 0, 21 do
          local args = {}
          for j = 1, i do args[j] = j * i end
          local r = table.pack(coroutine.resume(co, table.unpack(args, 1, i)))
          log[#log + 1] = r.n .. ':' .. tostring(r[2]) .. ':' .. tostring(r[r.n])
        end
        -- Three levels: a coroutine resuming another resuming a third.
        local c3 = coroutine.wrap(function(a) while true do a = coroutine.yield(a * 3) end end)
        local c2 = coroutine.wrap(function(a) while true do a = coroutine.yield(c3(a) + 1) end end)
        local c1 = coroutine.wrap(function(a) while true do a = coroutine.yield(c2(a) * 2) end end)
        local s = 0
        for i = 1, 200 do s = s + c1(i) end
        log[#log + 1] = s
        -- Errors cross a switch; the dead coroutine refuses more.
        local bad = coroutine.create(function() coroutine.yield(1); error('boom') end)
        log[#log + 1] = tostring(select(2, coroutine.resume(bad)))
        log[#log + 1] = tostring(select(2, coroutine.resume(bad)))
        log[#log + 1] = tostring(select(2, coroutine.resume(bad)))
        log[#log + 1] = coroutine.status(bad)
        -- Many live coroutines, resumed in turn.
        local cs = {}
        for i = 1, 50 do cs[i] = coroutine.wrap(function() local k = i; while true do k = k + coroutine.yield(k) end end) end
        local t = 0
        for r = 1, 20 do for i = 1, 50 do t = t + cs[i](r) end end
        log[#log + 1] = t
        return table.concat(log, ' ')
        ",
    );
}

/// The one-result return (`return x`) puts its value straight in place: every count of
/// results wanted, open upvalues closed on the way, calls of every shape.
#[test]
fn one_result_returns_in_every_context() {
    same_as_c(
        r"
        local function one(x) return x end
        local function boxed(x) local function get() return x end; x = x + 1; return get end
        local function tail(x) return one(x) end
        local out = {}
        one(1)                                  -- none wanted
        local a = one(2)                        -- one
        local b, c = one(3)                     -- two: the second is nil
        local t = { one(4), one(5) }            -- multiple in a constructor
        local n = select('#', one(nil))         -- all: a nil result counts
        out[#out + 1] = tostring(a) .. tostring(b) .. tostring(c) .. #t .. n
        out[#out + 1] = boxed(10)() + tail(7)
        local s = 0
        for i = 1, 1000 do s = s + one(i) + (boxed(i))() end
        out[#out + 1] = s
        out[#out + 1] = tostring((one(0.5)))
        local m = setmetatable({}, { __index = function(_, k) return one(k) end, __add = function(x, y) return one(42) end })
        out[#out + 1] = m.key .. (m + 1)
        return table.concat(out, ' ')
        ",
    );
}

/// The same on the small stack (64 KiB): deep chains of one-result returns and switches.
#[test]
fn one_result_returns_and_switches_on_the_small_stack() {
    small_stack(|| {
        same_as_c(
            r"
            local function down(n) if n == 0 then return 0 end return down(n - 1) end
            local co = coroutine.wrap(function() for i = 1, 100 do coroutine.yield(down(150)) end end)
            local s = 0
            for _ = 1, 100 do s = s + co() end
            return s
            ",
        );
    });
}

/// PERF-SEC-1: a write through an upvalue table with a constant key (`SETTABUP`: every
/// global assignment) that grew the table lost the fuel of the rehash on the full path
/// (and `SETTABUP` always ran there): `raw_set` charged it, and the instruction loop then
/// wrote its own count back over the charge. Only what the charge took from the reserve
/// (whole slices, when it overran the slice in hand) stayed: a rehash of 4,096 globals
/// cost what a plain write does, one of 65,536 a rounded amount. Growing the table through
/// `SETTABLE` (a key in a register) was charged exactly; both are now.
#[test]
fn perf_sec_1_global_writes_charge_their_rehash() {
    for size in [4096, 65536] {
        for fast in [true, false] {
            // (Fuel is per call: each starts with the full amount.)
            let used = |grow: &str| {
                let mut lua = Lua::new(Limits::default().fuel(1 << 40).hash_seed(TEST_SEED), Libs::ALL);
                lua.set_fast_loop(fast);
                // Exactly `size` globals: the next new one finds no free node.
                lua.exec(
                    format!(
                        "function grow() {grow} end
                         function keep() k1 = 2 end
                         local n = 0
                         for _ in pairs(_ENV) do n = n + 1 end
                         for i = 1, {size} - n do _ENV['k' .. i] = true end"
                    ),
                    "=setup",
                )
                .unwrap();
                let mut run = |src: &str| {
                    lua.exec(src, "=t").unwrap();
                    (1u64 << 40) - lua.fuel_left()
                };
                (run("keep()"), run("grow()"))
            };
            let (keep, by_settabup) = used("fresh = 1");
            // The same new key through SETTABLE: its key in a register (two steps more).
            let (_, by_settable) = used("local k = 'fresh'; _ENV[k] = 1");
            assert!(
                by_settable >= keep + size / 64,
                "size {size}, fast loop {fast}: no rehash ({by_settable} against {keep})"
            );
            assert_eq!(
                by_settabup + 2,
                by_settable,
                "size {size}, fast loop {fast}: a global write that rehashed cost {by_settabup}, a plain write {keep}"
            );
        }
    }
}

/// `gsub` binds one matcher state to its subject again after each replacement function
/// (the state is borrowed now, not moved): captures, counters and the step budget carry
/// over replacement calls and fuel charges alike.
#[test]
fn gsub_state_carries_over_replacement_calls() {
    same_as_c(
        r#"
        local parts = {}
        for i = 1, 3000 do parts[#parts + 1] = "k" .. i .. "=v" .. (i * 7) .. ";" end
        local s = table.concat(parts)          -- long enough to be charged mid-way
        local calls = 0
        local r1, n1 = s:gsub("(%a)(%d+)=(%a)(%d+)", function(a, b, c, d) calls = calls + 1 return d .. c .. "=" .. b .. a end)
        local r2, n2 = s:gsub("()k(%d)", function(p, d) return (p % 3 == 0) and d or nil end)
        local r3, n3 = s:gsub("^k1", "K")
        local r4, n4 = s:gsub("%d+", { ["1"] = "one", ["21"] = false }, 500)
        local ok, err = pcall(string.gsub, s, "(%d+)", function(d) if d == "2999" then error("stop at " .. d) end end)
        local r5, n5 = ("abc"):gsub("", "-")
        return #r1, n1, calls, r1:sub(1, 40), #r2, n2, r3:sub(1, 8), n3, r4:sub(1, 60), n4, ok, err, r5, n5
        "#,
    );
}

/// `coroutine.yield` called from Lua code runs without `call_native`'s set-up: the same
/// results, errors and positions as a native call, wherever it yields from.
#[test]
fn yields_from_lua_code_everywhere() {
    same_as_c(
        r#"
        local out = {}
        local y = coroutine.yield
        out[#out + 1] = select(2, pcall(coroutine.yield, 1))            -- outside a coroutine
        out[#out + 1] = select(2, pcall(function() y(1) end))
        local co = coroutine.wrap(function(...)
          local a = { y(...) }                                          -- every value through
          local ok, v = pcall(function() return y(#a) + 1 end)          -- across pcall
          local t = setmetatable({}, { __index = function(_, k) return y(k) end,
                                       __add = function(p, q) return y("add") end })
          local r = t.key .. tostring(t + 1)                            -- inside metamethods
          for x in function() return y("iter") end do r = r .. x break end
          return ok, v, r
        end)
        local vals = {}
        for i = 1, 40 do vals[i] = i end
        out[#out + 1] = select('#', co(table.unpack(vals)))
        out[#out + 1] = co(table.unpack(vals, 1, 7))
        out[#out + 1] = co(10)
        out[#out + 1] = co("K")
        out[#out + 1] = co("A")
        out[#out + 1] = table.concat({ tostring(co("I")) }, ",")
        local ok, err = pcall(coroutine.wrap(function() local t <close> = setmetatable({}, { __close = function() y() end }) end))
        out[#out + 1] = tostring(ok) .. " " .. tostring(err)
        return table.concat(out, " | ")
        "#,
    );
}

/// `find`, `match` and `gmatch` share one matcher state between calls, and a match's
/// captures keep four inline (the rest apart): many captures, position captures,
/// back-references, iterators interleaved, errors in between.
#[test]
fn matcher_state_reused_between_calls() {
    same_as_c(
        r#"
        local out = {}
        local s = "a1b2c3d4e5f6g7h8i9j0 xyzzy xy"
        out[#out + 1] = table.concat({ s:match("(%a)(%d)(%a)(%d)(%a)(%d)()(%a)(%d)(%a)(%d)") }, ",")
        out[#out + 1] = table.concat({ s:find("(x)(y)(z)(z)(y)") }, ",")
        out[#out + 1] = table.concat({ ("abab"):find("(ab)%1") }, ",")
        local g1, g2 = s:gmatch("(%a)(%d)"), s:gmatch("()(%d)")
        for _ = 1, 5 do
          local a, b = g1()
          local p, d = g2()
          out[#out + 1] = a .. b .. p .. d
        end
        out[#out + 1] = tostring(pcall(string.find, s, "(%d"))
        out[#out + 1] = tostring(pcall(string.match, s, "%"))
        out[#out + 1] = table.concat({ s:match("^(%a%d)(%a%d)(%a%d)(%a%d)(%a%d)(%a%d)(%a%d)(%a%d)(%a%d)(%a%d)") }, ",")
        out[#out + 1] = tostring(s:find("q"))
        local caps = string.rep("(.)", 32)
        out[#out + 1] = select('#', s:match(caps))
        out[#out + 1] = tostring(pcall(string.match, s, string.rep("(.)", 33)))
        return table.concat(out, " ")
        "#,
    );
}

/// CPU time per unit of fuel of `body` run in a loop after `setup` until the fuel runs out
/// (the best of three: the machine is shared).
fn cpu_per_fuel(setup: &str, body: &str, fuel: u64, fast: bool) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut lua =
            Lua::new(Limits::default().fuel(1 << 40).memory(256 << 20).hash_seed(TEST_SEED), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(setup, "=setup").unwrap();
        lua.set_limits(lua.limits().fuel(fuel));
        let f = lua.load(format!("while true do {body} end"), "=probe").unwrap();
        let start = remedy_lua_oracle::thread_cpu();
        let r = lua.call(f.value(), &[]);
        let t = remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64;
        assert!(matches!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel))), "{body}: {r:?}");
        best = best.min(t / (fuel - lua.fuel_left()).max(1) as f64);
    }
    best
}

/// The CPU per unit of fuel of `body` over that of plain code, measured together up to
/// three times while over `bound` (a loaded machine slows one measurement, not the other).
fn ratio_to_plain(setup: &str, body: &str, fast: bool, bound: f64) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        // The relative cost tests' own control (`sweep5b_walks::assert_ratio`).
        let plain = cpu_per_fuel("x = 0", "x = x + 1", 3_000_000, fast);
        best = best.min(cpu_per_fuel(setup, body, 3_000_000, fast) / plain);
        if best < bound {
            break;
        }
    }
    best
}

/// LR-11: a set (`[...]`) was walked whole for every character tried against it (charged
/// a step per 16 bytes), and `gsub` walked it again at each of its fuel charges to find
/// where the first item ends: a set of 65,536 bytes over 65,536 characters ran at 11x
/// plain code per unit of fuel with the fast loop. Sets are made once per call now, and a
/// state keeps what it worked out about its pattern: about 0.1x. The fuel is unchanged.
#[test]
fn lr_11_long_sets_cost_what_they_are_charged() {
    let setup =
        "S = string.rep('1', 65536) P = '[' .. string.rep('b', 65536) .. ']' F = '%f' .. P Q = '1' .. P";
    for fast in [true, false] {
        // (A long search stops at the matcher's step limit, "pattern too complex": caught.)
        let bodies = [
            "pcall(string.gsub, S, P, '')",
            "pcall(string.find, S, P)",
            "pcall(function() for _ in string.gmatch(S, P) do end end)",
            "pcall(string.find, S, F)",
            // A set after the first item: its end was looked for at every attempt.
            "pcall(string.find, S, Q)",
        ];
        for body in bodies {
            let r = ratio_to_plain(setup, body, fast, 3.0);
            assert!(r < 3.0, "fast {fast}: {body} at {r:.1}x plain code per unit of fuel");
        }
    }
}

/// Sets of 32 bytes or more match from a set made once per call (LR-11): the same matches
/// as C Lua for ranges, classes, negation, frontiers and sets after the first item.
#[test]
fn long_sets_match_as_c_lua() {
    same_as_c(
        r#"
        local subj = {}
        for c = 0, 255 do subj[#subj + 1] = string.char(c) end
        local s = table.concat(subj) .. " Hello, World_42! ]-^%"
        local long = {
          "[" .. string.rep("%a", 16) .. "]",
          "[^" .. string.rep("a-cx-z", 8) .. "]",
          "[%d%s" .. string.rep("_", 40) .. "]",
          "[" .. string.rep("%%%]%-", 10) .. "]",
          "[^%w%p" .. string.rep("\0", 30) .. "]",
          "[\128-\255" .. string.rep("q", 40) .. "]",
        }
        local out = {}
        for _, set in ipairs(long) do
          local n = select(2, s:gsub(set, ""))
          local a, b = s:find(set .. set)
          local f = select(2, s:gsub("%f" .. set, "|"))
          local g = 0
          for w in s:gmatch(set .. "+") do g = g + #w end
          out[#out + 1] = table.concat({ n, tostring(a), tostring(b), f, g, tostring(s:match("H" .. set)) }, ",")
        end
        return table.concat(out, " ; ")
        "#,
    );
}

/// `string.pack`, `packsize` and `unpack` charged a unit of fuel per 8 format bytes read:
/// a format of 16,384 items ran at about 5x plain code per unit (`unpack` 7.5x), up to
/// the property test's bound of 10 on some code layouts. Two units a byte now.
#[test]
fn pack_formats_are_charged_by_what_they_read() {
    let setup = "f = string.rep('x', 16384) d = string.rep('a', 16384) g = string.rep('i', 16384)";
    for fast in [true, false] {
        for body in [
            "pcall(string.packsize, f)",
            "pcall(string.pack, f)",
            "pcall(string.unpack, f, d)",
            "pcall(string.packsize, g)",
        ] {
            let r = ratio_to_plain(setup, body, fast, 3.0);
            assert!(r < 3.0, "fast {fast}: {body} at {r:.1}x plain code per unit of fuel");
        }
    }
}

/// A rehash was charged after it was done, so one the call's fuel could not pay for (a
/// million keys placed again: about 500,000 units) ran in full past the fuel limit. What it
/// surely costs, from the table's sizes, is checked first now: the call stops at its fuel
/// limit with the table untouched, on both paths and under any seed.
#[test]
fn a_rehash_the_fuel_cannot_pay_for_stops_before_it() {
    small_stack(|| {
        for fast in [true, false] {
            for seed in [TEST_SEED, 7] {
                let mut lua = Lua::new(Limits::default().fuel(1 << 30).hash_seed(seed), Libs::ALL);
                lua.set_fast_loop(fast);
                // 2^16 integer keys in the hash part, which they fill: a new key rehashes.
                lua.exec("t = {} for i = 1, 1 << 16 do t[i * 3] = true end", "=setup").unwrap();
                lua.set_limits(lua.limits().fuel(20_000));
                let r = lua.exec("local t = t t[0] = true", "=t");
                assert_eq!(r.err(), Some(Error::Limit(LimitKind::Fuel)), "fast loop {fast}, seed {seed}");
                assert_eq!(lua.fuel_left(), 0);
                lua.set_limits(lua.limits().fuel(1 << 30));
                let r = lua
                    .exec("local n = 0 for _ in pairs(t) do n = n + 1 end return t[0] == nil, n", "=t")
                    .unwrap();
                assert_eq!(
                    r.first().and_then(|v| v.as_bool()),
                    Some(true),
                    "fast loop {fast}, seed {seed}: the table was touched"
                );
                assert_eq!(r.get(1).and_then(|v| v.as_int()), Some(1 << 16));
                // With the fuel for it, the rehash is done.
                let r = lua.exec("local t = t t[0] = true return t[0]", "=t").unwrap();
                assert_eq!(r.first().and_then(|v| v.as_bool()), Some(true));
            }
        }
    });
}

/// The check before a rehash took a lower bound from the table's sizes (98,304 units for
/// 2^17 - 1 integer keys, against the 205,000 the rehash charges), so a call with fuel in
/// between still did the whole rehash past its limit. The charge is worked out exactly
/// first now: with 150,000 units the call stops with the table untouched, on both paths
/// and under any seed. (SEC8-F4: what it made is kept, its fuel paid, and the call that
/// ends it pays the rest: the two together the whole charge.)
#[test]
fn a_rehash_is_checked_against_its_whole_charge() {
    small_stack(|| {
        let mut fuel = Vec::new();
        for fast in [true, false] {
            for seed in [TEST_SEED, 7] {
                let fresh = || {
                    let mut lua = Lua::new(Limits::default().fuel(1 << 30).hash_seed(seed), Libs::ALL);
                    lua.set_fast_loop(fast);
                    // One key short of a power of two: the next new key rehashes.
                    lua.exec(
                        "t = {} for i = 1, (1 << 17) - 1 do t[i * 3] = i end t.zz = 1 t.zz = nil",
                        "=setup",
                    )
                    .unwrap();
                    lua
                };
                // (Each host call starts with the whole budget: its fuel is the budget less
                // what it left.)
                let mut lua = fresh();
                lua.exec("local t = t t[-1] = 1", "=t").unwrap();
                let whole = lua.limits().fuel - lua.fuel_left();
                assert!(whole > 150_000, "fast loop {fast}, seed {seed}: {whole}");
                let mut lua = fresh();
                lua.set_limits(lua.limits().fuel(150_000));
                let r = lua.exec("local t = t t[-1] = 1", "=t");
                assert_eq!(r.err(), Some(Error::Limit(LimitKind::Fuel)), "fast loop {fast}, seed {seed}");
                let first = lua.limits().fuel - lua.fuel_left();
                lua.set_limits(lua.limits().fuel(1 << 30));
                let r = lua.exec("return t[-1] == nil", "=t").unwrap();
                assert_eq!(
                    r.first().and_then(|v| v.as_bool()),
                    Some(true),
                    "fast loop {fast}, seed {seed}: the table was touched"
                );
                // The fuel it does take, the same both ways and for every seed: with the
                // stopped call's, the whole charge (and the write's steps again).
                lua.exec("local t = t t[-1] = 1", "=t").unwrap();
                let used = lua.limits().fuel - lua.fuel_left();
                assert!(
                    (whole..=whole + 100).contains(&(first + used)),
                    "fast loop {fast}, seed {seed}: {first} + {used} for {whole}"
                );
                fuel.push((whole, first, used));
            }
        }
        assert!(fuel.iter().all(|f| *f == fuel[0]), "fuel by loop and seed: {fuel:?}");
    });
}
