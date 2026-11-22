//! Sweep 5b: a cost fuzzer. Sweep 5's criticals were all one class: work proportional to
//! a structure a script shapes, walked without fuel (probe chains, free slab slots,
//! interner runs, nested interpolations). The fuel-against-time property
//! (`sweep3_sec::fuel_tracks_time_for_every_function`) measures fixed per-function
//! families with fixed argument shapes, so it cannot see a cost that depends on the state
//! earlier code left behind. This one generates that state.
//!
//! A program has two phases. SHAPE builds hostile state (sparse and churned tables with
//! mixed keys, `__index`/`__newindex` chains, colliding integer chains, dead objects with
//! few survivors, coroutines parked deep, open upvalues, shared and unshared upvalues,
//! long and prefix-sharing strings, weak tables of every mode, ephemeron chains, pending
//! to-be-closed variables, big varargs, a deep main stack, collector settings). OPERATION
//! then loops over a few operations on that state until the fuel runs out (lookups,
//! inserts, `#`, `next`/`pairs`/`ipairs`, `concat`, `sort`, front insert/remove, string
//! work, closures, coroutines, errors from depth, collector steps, metatable changes,
//! `tostring`/`format`, `load`). Measured: the thread's CPU per unit of fuel against the
//! median of plain code, and the process's resident memory against `memory_used`. A
//! program over the bound is measured again (median of three), then minimized (operations
//! and shapes dropped, sizes halved) and reported. Every program also runs with the fast
//! loop on and off (same result, fuel and memory), on a 64 KiB stack (`small_stack`), so
//! native recursion that follows a script's data aborts here.
//!
//! Quick by default (a fixed seed and a fresh one, printed on failure). Long run:
//! `S5B_COST_SECS=600 S5B_COST_SEED=7 cargo test --release -p remedy-lua --test all sweep5b_cost::cost_fuzz_long -- --ignored --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::time::Instant;

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua};

struct Rng(u64);

impl Rng {
    /// A generator for `seed` (mixed, so neighbouring seeds draw unrelated programs).
    fn new(seed: u64) -> Rng {
        let mut h = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        Rng((h ^ (h >> 31)) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    /// Log-uniform in `1..=max`.
    fn size(&mut self, max: u32) -> u32 {
        let bits = 32 - max.max(1).leading_zeros();
        let b = self.below(u64::from(bits) + 1) as u32;
        let hi = if b >= 32 { max } else { (1u32 << b).min(max) };
        (hi / 2 + self.below(u64::from(hi / 2 + 1)) as u32).clamp(1, max)
    }
}

/// A shape: Lua that builds state of size `n` (with `a`, a fixed random choice) and puts
/// what it built in the pools (`TABS` tables, `STRS` strings, `LISTS` arrays of strings,
/// `COS` suspended coroutines, `FNS` functions, `KEYS` keys).
type ShapeGen = fn(u32, u32) -> String;

/// Pools and helpers every program starts with.
const PRELUDE: &str = "TABS, STRS, LISTS, COS, FNS, KEYS = {}, {}, {}, {}, {}, {} ACC = 0 I = 0 \
NARGS = 0 ARGS = {} DEPTH = 0 MARK = MARK or function() end \
function PICK(p) local n = #p if n == 0 then return nil end I = I + 1 return p[I % n + 1] end \
function DEEPERR(d) if d <= 0 then error({d}) end local x = DEEPERR(d - 1) return x end \
function PARK(d, f) if d <= 0 then return f() end local x = PARK(d - 1, f) return x end \
function FOREVER() while true do coroutine.yield(1) end end \n";

const SHAPES: &[(&str, u32, ShapeGen)] = &[
    ("sparse array", 1 << 20, |n, a| {
        let keep = [2, 64, 1024][a as usize % 3];
        format!("do local t = {{}} for i = 1, {n} do t[i] = i end for i = 1, {n} do if i % {keep} ~= 0 then t[i] = nil end end TABS[#TABS + 1] = t end")
    }),
    ("churned string keys", 1 << 18, |n, a| {
        let left = [1, 16, n / 2][a as usize % 3];
        format!("do local t = {{}} for i = 1, {n} do t['k' .. i] = i end for i = 1, {n} - {left} do t['k' .. i] = nil end \
                 TABS[#TABS + 1] = t KEYS[#KEYS + 1] = 'k' .. {n} KEYS[#KEYS + 1] = 'k1' end")
    }),
    ("mixed keys with deletions", 1 << 17, |n, _| {
        format!("do local t, keep = {{}}, {{}} for i = 1, {n} do t[i * 3] = i t[i + 0.5] = i t['s' .. i] = i t[i % 2 == 0] = i \
                 local o = {{}} t[o] = i if i % 7 == 0 then keep[#keep + 1] = o end t[-i] = i end \
                 for i = 1, {n}, 2 do t[i * 3] = nil t[i + 0.5] = nil t['s' .. i] = nil t[-i] = nil end \
                 TABS[#TABS + 1] = t TABS[#TABS + 1] = keep KEYS[#KEYS + 1] = 2.5 KEYS[#KEYS + 1] = -2 KEYS[#KEYS + 1] = true end")
    }),
    ("integer collision chains", 1 << 16, |n, a| {
        // Keys at the end of chains of colliding integers (Lua's node layout; no seed).
        let bits = (32 - n.max(64).leading_zeros()).clamp(6, 17);
        let len = [8, 64, 500][a as usize % 3].min((1u32 << bits) / 4);
        let chains = ((1u32 << bits) / (len + 2)).clamp(1, 64);
        format!(
            "do local t = {{}} local J = 1 << 40 for j = 1, (1 << {bits}) + 1 do t[J + j] = true end \
             for j = 1, (1 << {bits}) + 1 do t[J + j] = nil end local M = (1 << ({bits} + 1)) - 1 \
             for i = 1, {chains} do t[i + M * 1000] = true t[i] = i for c = 1, {len} do t[i + M * (1000 + c)] = true end end \
             TABS[#TABS + 1] = t KEYS[#KEYS + 1] = 1 KEYS[#KEYS + 1] = {chains} KEYS[#KEYS + 1] = 1 + M * 999 end"
        )
    }),
    ("__index/__newindex chain", 1 << 14, |n, a| {
        let nw = if a % 2 == 0 { ", __newindex = b" } else { "" };
        format!("do local b = {{v = 1, k1 = 2}} for i = 1, {n} do b = setmetatable({{}}, {{__index = b{nw}}}) end TABS[#TABS + 1] = b end")
    }),
    ("__index function chain", 180, |n, _| {
        format!("do local b = {{v = 1}} for i = 1, {n} do local p = b b = setmetatable({{}}, {{__index = function(t, k) return p[k] end}}) end TABS[#TABS + 1] = b end")
    }),
    ("__index self loop", 1, |_, _| "do local t = {} setmetatable(t, {__index = t, __newindex = t}) TABS[#TABS + 1] = t end".into()),
    ("metatable chain", 1 << 12, |n, _| {
        format!("do local m = {{__index = function() return 1 end}} for i = 1, {n} do m = setmetatable({{__index = m, __eq = m.__eq, __len = function() return i end}}, m) end \
                 TABS[#TABS + 1] = setmetatable({{}}, m) end")
    }),
    ("dead objects, few survivors", 1 << 21, |n, a| {
        let make = ["{}", "function() return i end", "coroutine.create(FOREVER)", "'d' .. i", "{i, i}", "setmetatable({}, {__mode = 'k'})"][a as usize % 6];
        // Each kind sized to fit the 256 MiB cap alone, with room (at most about 150 MiB):
        // a weak table costs about 150 bytes, a plain or two-slot table 75 (long-run
        // finding: at 2^21 they did not fit and the setup failed).
        let n = n.min([1 << 20, 1 << 20, 1 << 17, 1 << 21, 1 << 20, 1 << 19][a as usize % 6]);
        format!("do KEEP = KEEP or {{}} local j = {{}} for i = 1, {n} do j[i] = {make} end for i = 1000, {n}, 1000 do KEEP[#KEEP + 1] = j[i] end \
                 KEEP[#KEEP + 1] = j[{n}] end collectgarbage() collectgarbage()")
    }),
    ("coroutines parked deep", 1 << 16, |n, a| {
        let depth = [10, 190, 1900][a as usize % 3].min(n);
        let count = (n / depth).clamp(1, 2000);
        format!("for j = 1, {count} do local co = coroutine.create(function() PARK({depth}, FOREVER) end) coroutine.resume(co) COS[#COS + 1] = co end")
    }),
    ("coroutines parked in coroutines", 150, |n, _| {
        // A chain of suspended coroutines: each holds the next, and resumes it when resumed.
        format!("do local function nest(d) if d == 0 then while true do coroutine.yield(0) end end \
                 local co = coroutine.wrap(function() return nest(d - 1) end) co() while true do coroutine.yield(co()) end end \
                 local co = coroutine.create(nest) coroutine.resume(co, {n}) COS[#COS + 1] = co end")
    }),
    ("closures sharing an upvalue", 1 << 17, |n, a| {
        if a % 2 == 0 {
            format!("do local x = 0 for i = 1, {n} do FNS[#FNS + 1] = function(y) x = x + 1 return x end end end")
        } else {
            format!("for i = 1, {n} do local x = i FNS[#FNS + 1] = function(y) x = x + 1 return x end end")
        }
    }),
    ("open upvalues parked", 1 << 12, |n, _| {
        // Coroutines parked with 200 captured locals per frame.
        let locals: Vec<String> = (0..200).map(|i| format!("l{i}")).collect();
        let caps: Vec<String> = (0..200).map(|i| format!("l{i}")).collect();
        let depth = n.clamp(1, 1900);
        format!(
            "do local function fat(d) local {} = d if d <= 0 then local f = function() return {} end while true do coroutine.yield(f) end end \
             local r = fat(d - 1) return r end local co = coroutine.create(fat) coroutine.resume(co, {depth}) COS[#COS + 1] = co end",
            locals.join(", "),
            caps.join(" + ")
        )
    }),
    ("function with many upvalues", 1, |_, _| {
        let names: Vec<String> = (0..250).map(|i| format!("u{i}")).collect();
        format!(
            "do local {} = 1 FNS[#FNS + 1] = function(y) local g = function() return u7 + y end return g() + u200 end end",
            names.join(", ")
        )
    }),
    ("long strings", 1 << 24, |n, _| format!("STRS[#STRS + 1] = string.rep('ab', {n} // 2) .. 'x'")),
    ("short strings sharing a prefix", 1 << 18, |n, a| {
        let pre = "p".repeat([1, 30, 39][a as usize % 3]);
        format!("do local l = {{}} for i = 1, {n} do l[i] = '{pre}' .. i end LISTS[#LISTS + 1] = l STRS[#STRS + 1] = l[1] end")
    }),
    ("weak table", 1 << 18, |n, a| {
        let mode = ["k", "v", "kv"][a as usize % 3];
        format!("do local w = setmetatable({{}}, {{__mode = '{mode}'}}) KEEP = KEEP or {{}} for i = 1, {n} do local o = {{}} w[o] = o w[i] = {{}} \
                 if i % 3 == 0 then KEEP[#KEEP + 1] = o end end TABS[#TABS + 1] = w end")
    }),
    ("ephemeron chain", 1 << 17, |n, a| {
        // e[k_i] = k_{i+1}, keys inserted last first: each atomic pass finds one more.
        // (Choices 4 and 5: the same chain in a strong table, as a control.)
        let rev = a % 2 == 0;
        let (from, to, step) = if rev { (n, 1, -1) } else { (1, n, 1) };
        let mode = if a % 6 >= 4 { "" } else { "__mode = 'k'" };
        format!("do local e = setmetatable({{}}, {{{mode}}}) local ks = {{}} for i = 1, {n} + 1 do ks[i] = {{}} end \
                 for i = {from}, {to}, {step} do e[ks[i]] = ks[i + 1] end ROOT = ks[1] ks = nil TABS[#TABS + 1] = e end")
    }),
    ("pending to-be-closed variables", 1900, |n, _| {
        format!("do local function tbc(d) if d <= 0 then while true do coroutine.yield() end end \
                 local x <close> = setmetatable({{}}, {{__close = function() end}}) local r = tbc(d - 1) return r end \
                 local co = coroutine.create(tbc) coroutine.resume(co, {n}) COS[#COS + 1] = co end")
    }),
    ("coroutine parked with big varargs", 1 << 20, |n, _| {
        format!("do local co = coroutine.create(function(...) while true do coroutine.yield(select('#', ...)) end end) \
                 local t = {{}} for i = 1, {n} do t[i] = i end coroutine.resume(co, table.unpack(t)) COS[#COS + 1] = co end")
    }),
    ("main stack: big varargs", 1 << 20, |n, _| format!("NARGS = {n} for i = 1, {n} do ARGS[i] = i end")),
    ("main stack: deep", 1900, |n, _| format!("DEPTH = {n}")),
    ("many functions loaded", 1 << 14, |n, _| {
        format!("for i = 1, {n} do FNS[#FNS + 1] = load('local a = ... return function(b) return (b or 0) + ' .. i .. ' end')() end")
    }),
    ("array of strings", 1 << 17, |n, _| {
        format!("do local l = {{}} for i = 1, {n} do l[i] = tostring(i * 7919 % {n}) end LISTS[#LISTS + 1] = l TABS[#TABS + 1] = l end")
    }),
    ("interner churn", 1 << 20, |n, _| format!("do local l = {{}} for i = 1, {n} do l[i] = 'c' .. i end end collectgarbage() collectgarbage()")),
    ("big live string-keyed table", 1 << 19, |n, _| {
        format!("do local t = {{}} for i = 1, {n} do t['f' .. i] = i end TABS[#TABS + 1] = t KEYS[#KEYS + 1] = 'f1' end")
    }),
    ("nested tables", 1 << 16, |n, _| format!("do local t = {{}} for i = 1, {n} do t = {{t}} end TABS[#TABS + 1] = t end")),
    ("collector settings", 1, |_, a| {
        [
            "collectgarbage('generational')",
            "collectgarbage('incremental', 1000, 4)",
            "collectgarbage('setpause', 0)",
            "collectgarbage('setstepmul', 1000)",
            "collectgarbage('stop')",
        ][a as usize % 5]
            .into()
    }),
    ("one key short of a power of two", 1 << 20, |n, a| {
        // Full but for one node: every new key rehashes it (as in C Lua).
        let bits = (32 - n.leading_zeros()).max(2);
        let key = ["i * 3", "'r' .. i", "i + 0.5"][a as usize % 3];
        format!("do local t = {{}} for i = 1, (1 << {bits}) - 1 do t[{key}] = i end TABS[#TABS + 1] = t end")
    }),
    ("objects with __tostring, __name and __pairs", 1 << 14, |n, a| {
        let name = "N".repeat([1, 100, 4000][a as usize % 3]);
        format!("do local m = {{__name = '{name}', __tostring = function(t) return 'obj' end, __pairs = function(t) return next, t, nil end, \
                 __index = function(t, k) return k end, __len = function() return 3 end, __eq = function() return true end}} \
                 for i = 1, {n} do local o = setmetatable({{i}}, m) if i % 64 == 0 then TABS[#TABS + 1] = o end end end")
    }),
    // The opt-in libraries (`Libs::DEBUG_INFO`, `Libs::REQUIRE`): what a traceback's name
    // search and `require`'s lookups walk.
    ("many loaded modules", 1 << 16, |n, a| {
        let v = ["i", "{i}", "{i, 'x' .. i, function() end}"][a as usize % 3];
        format!("for i = 1, {n} do package.loaded['mod' .. i] = {v} end")
    }),
    ("churned loaded table", 1 << 16, |n, _| {
        format!("for i = 1, {n} do package.loaded['c' .. i] = i end for i = 1, {n} - 1 do package.loaded['c' .. i] = nil end")
    }),
    ("table with a huge array part", 1 << 22, |n, _| {
        format!("do local t = table.move({{}}, 1, 0, 1) t[{n}] = 1 for i = 1, {n} // 2 do t[i] = i end for i = 1, {n} // 2 - 1 do t[i] = nil end TABS[#TABS + 1] = t end")
    }),
];

/// Operations: Lua run in the loop. `I` counts iterations, `ACC` collects results, `...`
/// is the main stack's varargs.
const OPS: &[(&str, &str)] = &[
    ("lookup", "local t = PICK(TABS) if t then local v = t[I % 64] local w = t.v local k = PICK(KEYS) local z = k ~= nil and t[k] ACC = ACC + (v and 1 or 0) end"),
    ("lookup absent", "local t = PICK(TABS) if t then local v = t[I * 7919 + 0.5] local w = t[-I] local z = t.absent ACC = ACC + (v and 1 or 0) end"),
    ("rawget", "local t = PICK(TABS) if t then local k = PICK(KEYS) if k ~= nil then ACC = ACC + (rawget(t, k) and 1 or 0) end end"),
    ("insert and delete", "local t = PICK(TABS) if t then pcall(function() t[-I - 0.5] = 1 t[-I - 0.5] = nil t['n' .. I] = 1 t['n' .. I] = nil end) end"),
    ("churn a new integer key", "local t = PICK(TABS) if t then pcall(function() t[-I] = 1 t[-I] = nil end) end"),
    ("toggle a string key", "local t = PICK(TABS) if t then pcall(function() t.zz = 1 t.zz = nil end) end"),
    ("close a coroutine with upvalues", "local co = coroutine.create(function() local a, b, c = I, 2, 3 local f = function() return a + b + c end coroutine.yield(f) end) coroutine.resume(co) coroutine.close(co)"),
    ("table.move", "local l = PICK(LISTS) if l and #l > 2 then table.move(l, 1, math.min(#l - 1, 64), 2) end"),
    ("string.rep with separator", "ACC = ACC + #string.rep('ab', I % 50, ',')"),
    ("nested pcall", "local ok = pcall(pcall, pcall, error, 'x') ACC = ACC + (ok and 1 or 0)"),
    ("utf8", "local s = PICK(STRS) if s then ACC = ACC + (utf8.len(s:sub(1, 64)) or 0) end"),
    ("compare objects", "local a, b = PICK(TABS), PICK(TABS) if a and b then local ok, e = pcall(function() return a == b end) ACC = ACC + (e and 1 or 0) end"),
    ("insert new int", "local t = PICK(TABS) if t then pcall(function() t[I * 64 + 3] = I end) end"),
    ("length", "local t = PICK(TABS) if t then local ok, n = pcall(function() return #t end) ACC = ACC + (ok and type(n) == 'number' and 1 or 0) end"),
    ("rawlen", "local t = PICK(TABS) if t then ACC = ACC + rawlen(t) % 3 end"),
    ("next", "local t = PICK(TABS) if t then local k = next(t) local kk = PICK(KEYS) if kk ~= nil then pcall(next, t, kk) end ACC = ACC + (k ~= nil and 1 or 0) end"),
    ("pairs partial", "local t = PICK(TABS) if t then local c = 0 for k, v in next, t do c = c + 1 if c > 20 then break end end ACC = ACC + c end"),
    ("pairs full", "local t = PICK(TABS) if t then local c = 0 for k, v in pairs(t) do c = c + 1 end ACC = ACC + c end"),
    ("ipairs", "local t = PICK(TABS) if t then local c = 0 pcall(function() for i, v in ipairs(t) do c = c + 1 if c > 50 then break end end end) ACC = ACC + c end"),
    ("concat", "local l = PICK(LISTS) if l then ACC = ACC + #table.concat(l, ',', 1, math.min(#l, 64)) end"),
    ("concat whole", "local l = PICK(LISTS) if l then ACC = ACC + #table.concat(l, ',') end"),
    ("sort", "local l = PICK(LISTS) if l then pcall(table.sort, l) end"),
    ("sort comparator", "local l = PICK(LISTS) if l then pcall(table.sort, l, function(a, b) return a > b end) end"),
    ("front insert/remove", "local l = PICK(LISTS) if l then table.insert(l, 1, 'z') table.remove(l, 1) end"),
    ("unpack", "local l = PICK(LISTS) if l then ACC = ACC + select('#', table.unpack(l, 1, math.min(#l, 100))) end"),
    ("string ops", "local s = PICK(STRS) if s then ACC = ACC + #s:sub(2, 9) + (s:find('x', 1, true) or 0) + #(s:sub(1, 4) .. I) end"),
    ("string match", "local s = PICK(STRS) if s then ACC = ACC + #(s:match('^p*') or '') + select(2, s:gsub('b', 'c', 2)) end"),
    ("string compare", "local l = PICK(LISTS) if l and #l > 1 then ACC = ACC + (l[1] < l[#l] and 1 or 0) + (l[1] == l[2] and 1 or 0) end"),
    ("string as key", "local s = PICK(STRS) local t = PICK(TABS) if s and t then ACC = ACC + (rawget(t, s) and 1 or 0) end"),
    ("make short strings", "local s = 'p' .. (I % 4096) local u = string.format('%d:%s', I, s) ACC = ACC + #u"),
    ("tostring objects", "local t = PICK(TABS) local f = PICK(FNS) local c = PICK(COS) ACC = ACC + #tostring(t) + #tostring(f) + #tostring(c)"),
    ("format objects", "local t = PICK(TABS) local s = PICK(STRS) ACC = ACC + #string.format('%s %q %d', t, s and s:sub(1, 10) or '', I)"),
    ("closures", "local x = I local f = function() return x + 1 end local g = function() x = x + 1 return f() end ACC = ACC + g()"),
    ("call functions", "local f = PICK(FNS) if f then local ok, r = pcall(f, I) ACC = ACC + (ok and type(r) == 'number' and 1 or 0) end"),
    ("new coroutine", "local co = coroutine.create(function(a) local b = coroutine.yield(a) return b end) coroutine.resume(co, 1) coroutine.resume(co, 2) ACC = ACC + 1"),
    ("new coroutine closed", "local co = coroutine.create(function(a) local x <close> = setmetatable({}, {__close = function() end}) coroutine.yield(a) end) coroutine.resume(co, 1) coroutine.close(co)"),
    ("wrap", "local w = coroutine.wrap(function(...) local n = select('#', ...) coroutine.yield(n) end) ACC = ACC + w(1, 2, 3)"),
    ("resume parked", "local co = PICK(COS) if co then local ok, v = coroutine.resume(co) ACC = ACC + (ok and 1 or 0) end"),
    ("status parked", "local co = PICK(COS) if co then ACC = ACC + #coroutine.status(co) end"),
    ("close parked", "local co = PICK(COS) if co and I % 97 == 0 then coroutine.close(co) end"),
    ("error from depth", "local ok, e = pcall(DEEPERR, 40) ACC = ACC + (ok and 0 or 1)"),
    ("error with traceback", "local ok, e = xpcall(DEEPERR, function(m) return tostring(m) end, 20) ACC = ACC + (ok and 0 or 1)"),
    ("error string", "local ok, e = pcall(error, 'x' .. I) ACC = ACC + #e"),
    ("collector step", "collectgarbage('step')"),
    ("full collection", "if I % 16 == 0 then collectgarbage() end"),
    ("collector count", "ACC = ACC + (collectgarbage('count') > 0 and 1 or 0)"),
    ("garbage", "local t = {I, {}, 'g' .. I} local f = function() return t end"),
    ("setmetatable", "local t = PICK(TABS) if t then pcall(function() local m = getmetatable(t) setmetatable(t, nil) setmetatable(t, m) end) end"),
    ("weak table made", "local w = setmetatable({}, {__mode = I % 2 == 0 and 'k' or 'v'}) w[{}] = {} w[1] = {}"),
    ("load", "local f = load('return ' .. (I % 100) .. ' + 1') ACC = ACC + f()"),
    ("varargs", "ACC = ACC + select('#', ...) + (select(-1, 0, ...) or 0) % 2"),
    ("varargs packed", "if I % 8 == 0 then local t = {...} ACC = ACC + #t % 3 end"),
    ("to-be-closed", "do local x <close> = setmetatable({}, {__close = function() ACC = ACC + 1 end}) end"),
    ("deep recursion", "local ok = pcall(PARK, 100, function() return 1 end) ACC = ACC + (ok and 1 or 0)"),
    ("traceback", "local s = debug.traceback('m') ACC = ACC + #s % 7"),
    ("traceback from depth", "local ok, s = pcall(PARK, 60, function() return debug.traceback() end) ACC = ACC + #tostring(s) % 5"),
    ("traceback in handler", "local ok, e = xpcall(function() DEEPERR(20) end, debug.traceback) ACC = ACC + (ok and 0 or 1)"),
    ("traceback of a parked coroutine", "local co = PICK(COS) if co then ACC = ACC + #debug.traceback(co) % 3 end"),
    ("getinfo", "local t = debug.getinfo(1, 'Slnut') local u = debug.getinfo(2, 'n') ACC = ACC + t.currentline % 3"),
    ("getinfo of a parked coroutine", "local co = PICK(COS) if co then local t = debug.getinfo(co, 1, 'Sl') ACC = ACC + (t and 1 or 0) end"),
    ("require", "package.loaded.cm = nil package.preload.cm = package.preload.cm or function(n) return {n} end local m = require('cm') ACC = ACC + #m"),
    ("require loaded", "local m = require('string') ACC = ACC + (m and 1 or 0)"),
    ("require missing", "local ok, e = pcall(require, 'missing' .. I % 8) ACC = ACC + #e % 3"),
    ("require host module", "package.loaded.hostmod = nil local m = require('hostmod') ACC = ACC + m.n"),
];

/// Plain code: the median of these, per unit of fuel, is the baseline.
const PLAIN: &[&str] = &[
    "local x = 0 for i = 1, 10 do x = x + i end ACC = x",
    "local t = {} t[1] = I t.x = 2 ACC = t[1] + t.x",
    "local s = tostring(I % 100) .. 'a' ACC = #s",
    "local f = function() return 1 end ACC = f()",
    "local t = {I, 2, 3} ACC = #t + select('#', 1, 2)",
    "ACC = math.floor(I / 3) + string.byte('a')",
    "local s = string.format('%d', I % 100) ACC = #s",
];

#[derive(Clone, Debug)]
struct Prog {
    /// (shape index, size, choice)
    shapes: Vec<(usize, u32, u32)>,
    ops: Vec<usize>,
}

impl Prog {
    fn setup(&self) -> String {
        let mut s = String::from(PRELUDE);
        for &(i, n, a) in &self.shapes {
            s.push_str(&(SHAPES[i].2)(n, a));
            s.push('\n');
        }
        s
    }

    fn ops_text(&self) -> String {
        self.ops.iter().map(|&o| format!("do {} end\n", OPS[o].1)).collect()
    }

    /// The operation phase: the loop at the main stack's depth, with its varargs.
    fn chunk(&self, iters: Option<u32>) -> String {
        let head = match iters {
            Some(n) => format!("for _ = 1, {n} do"),
            None => "while true do".into(),
        };
        format!(
            "local function LOOP(...) MARK() {head} I = I + 1\n{}end return ACC end \
             local function DEEP(d) if d <= 0 then local a = ARGS ARGS = nil return LOOP(table.unpack(a, 1, NARGS)) end \
             local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10 = d, d, d, d, d, d, d, d, d, d local r = DEEP(d - 1) return r end \
             return DEEP(DEPTH)",
            self.ops_text()
        )
    }

    fn describe(&self) -> String {
        let shapes: Vec<String> = self.shapes.iter().map(|&(i, n, a)| format!("{}(n={n}, a={a})", SHAPES[i].0)).collect();
        let ops: Vec<&str> = self.ops.iter().map(|&o| OPS[o].0).collect();
        format!("shapes [{}] ops [{}]", shapes.join(", "), ops.join(", "))
    }

    fn generate(r: &mut Rng, scale: u32) -> Prog {
        let ns = 1 + r.below(3) as usize;
        let mut shapes = Vec::new();
        for _ in 0..ns {
            let i = r.below(SHAPES.len() as u64) as usize;
            let max = (SHAPES[i].1 >> scale).max(1);
            shapes.push((i, r.size(max), r.below(6) as u32));
        }
        let no = 1 + r.below(3) as usize;
        let ops = (0..no).map(|_| r.below(OPS.len() as u64) as usize).collect();
        Prog { shapes, ops }
    }
}

/// A fixed program: (shape name, size, choice) and operation names.
type Fixed = (&'static [(&'static str, u32, u32)], &'static [&'static str]);

/// Programs the quick run always measures: the shapes of this class found so far (by
/// shape and operation name, with sizes).
const FIXED: &[Fixed] = &[
    // S5B-COST-1: stacks of plain values walked by the collector.
    (&[("coroutine parked with big varargs", 1 << 17, 0)], &["collector step"]),
    (&[("main stack: big varargs", 1 << 17, 0)], &["collector step"]),
    (&[("open upvalues parked", 1000, 0)], &["full collection"]),
    // S5B-COST-2: a table rehashed at every new key.
    (&[("one key short of a power of two", 1 << 16, 0)], &["churn a new integer key"]),
    (&[("one key short of a power of two", 1 << 16, 0)], &["toggle a string key", "churn a new integer key"]),
    // Sweep 5's: integer collision chains, sparse slabs, traversal over dead keys.
    (&[("integer collision chains", 1 << 15, 2)], &["lookup", "length"]),
    (&[("dead objects, few survivors", 1 << 18, 0)], &["garbage"]),
    (&[("churned string keys", 1 << 15, 0)], &["pairs partial", "next"]),
];

impl Prog {
    fn named(shapes: &[(&str, u32, u32)], ops: &[&str]) -> Prog {
        let shape = |name: &str| SHAPES.iter().position(|s| s.0 == name).expect(name);
        let op = |name: &str| OPS.iter().position(|o| o.0 == name).expect(name);
        Prog { shapes: shapes.iter().map(|&(n, size, a)| (shape(n), size, a)).collect(), ops: ops.iter().map(|o| op(o)).collect() }
    }
}

const SEED: u64 = 0x5eed_7e57;
/// Fuel for the shape phase (a shape that does not finish is a bad program, not a hang).
const SETUP_FUEL: u64 = 1 << 30;

fn state(mem: usize, fast: bool) -> Lua {
    let mut lua = Lua::new(
        Limits::default().memory(mem).string_len(32 << 20).fuel(SETUP_FUEL).hash_seed(SEED),
        super::common::ALL_OPT_IN,
    );
    lua.add_module("hostmod", "local M = {n = 1} for i = 1, 20 do M[i] = function() return i end end return M");
    lua.set_fast_loop(fast);
    lua.set_print(|_| {});
    lua
}

/// Resident memory of the process in bytes (Linux; 0 elsewhere).
fn rss() -> usize {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|x| x.parse::<usize>().ok()))
        .map_or(0, |p| p * 4096)
}

/// One measurement of the operation phase.
struct Measured {
    /// CPU per unit of fuel used.
    cpu: f64,
    /// Resident memory gained over the whole run, and the most `memory_used` seen.
    rss_gain: usize,
    used: usize,
}

/// Runs the shape phase, then the operation phase until `fuel` is spent: CPU per unit of
/// fuel (`remedy_lua_oracle::thread_cpu`; only ratios are compared). `Err` when the
/// program did not end at a budget (a bad program, not a finding) or the setup failed.
fn measure(p: &Prog, fuel: u64, mem: usize, fast: bool) -> Result<Measured, String> {
    let rss0 = rss();
    let mut lua = state(mem, fast);
    // The operation phase runs in a coroutine (`MAIN`) that yields once its stack is
    // built (`MARK`): the clock and the fuel start there.
    lua.exec("MARK = coroutine.yield", "=mark").unwrap();
    lua.exec(p.setup(), "=setup").map_err(|e| format!("setup: {e}"))?;
    let chunk = format!("MAIN = coroutine.create(function() {} end) return coroutine.resume(MAIN)", p.chunk(None));
    let r = lua.exec(chunk, "=ops").map_err(|e| format!("ops: {e}"))?;
    if !matches!(r.first(), Some(remedy_lua::Value::Bool(true))) {
        return Err(format!("never reached the loop: {:?}", r.get(1).and_then(|v| lua.to_string_lossy(*v))));
    }
    lua.set_limits(lua.limits().fuel(fuel));
    let f = lua.load("return coroutine.resume(MAIN)", "=resume").unwrap();
    let start = remedy_lua_oracle::thread_cpu();
    let r = lua.call(f.value(), &[]);
    let t = remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64;
    let used_fuel = fuel - lua.fuel_left();
    let cpu = match r {
        Err(Error::Limit(LimitKind::Fuel)) => t / fuel as f64,
        // Out of memory, depth or string length: what was done is still bounded by fuel.
        Err(Error::Limit(LimitKind::Memory)) if used_fuel > fuel / 4 => t / used_fuel as f64,
        other => return Err(format!("ended with {other:?}")),
    };
    let used = lua.memory_used();
    let rss_gain = rss().saturating_sub(rss0);
    drop(lua);
    Ok(Measured { cpu, rss_gain, used })
}

fn median3(p: &Prog, fuel: u64, mem: usize) -> Result<f64, String> {
    let mut v = Vec::new();
    for fast in [true, false, true] {
        v.push(measure(p, fuel, mem, fast)?.cpu);
    }
    v.sort_by(f64::total_cmp);
    Ok(v[1])
}

/// The baseline: the median, over plain bodies, of their median of three.
fn baseline(fuel: u64) -> f64 {
    let mut all = Vec::new();
    for body in PLAIN {
        let mut v = Vec::new();
        for _ in 0..3 {
            let mut lua = state(64 << 20, true);
            lua.exec(PRELUDE, "=setup").unwrap();
            lua.set_limits(lua.limits().fuel(fuel));
            let f = lua.load(format!("while true do I = I + 1 {body} end"), "=plain").unwrap();
            let start = remedy_lua_oracle::thread_cpu();
            let r = lua.call(f.value(), &[]);
            let t = remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64;
            assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{body}: {r:?}");
            v.push(t / fuel as f64);
        }
        v.sort_by(f64::total_cmp);
        all.push(v[1]);
    }
    all.sort_by(f64::total_cmp);
    all[all.len() / 2]
}

/// The outcome of `iters` iterations with the fast loop `fast`: result, fuel used and
/// memory held.
fn outcome(p: &Prog, fast: bool, iters: u32, fuel: u64, mem: usize) -> (String, u64, usize) {
    let mut lua = state(mem, fast);
    let setup = lua.exec(p.setup(), "=setup");
    let fuel_setup = SETUP_FUEL - lua.fuel_left();
    // A setup that failed (a shape over the cap) is an outcome of its own: the operation
    // phase never ran, so it used no fuel (the fuel left is setup's, larger than `fuel`).
    if let Err(e) = setup {
        return (format!("setup: {e} (setup fuel {fuel_setup})"), 0, lua.memory_used());
    }
    lua.set_limits(lua.limits().fuel(fuel));
    let r = match lua.exec(p.chunk(Some(iters)), "=ops") {
        Ok(v) => v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default(),
        Err(e) => format!("error: {e}"),
    };
    (format!("{r} (setup fuel {fuel_setup})"), fuel.saturating_sub(lua.fuel_left()), lua.memory_used())
}

/// The fast loop only shortcuts: the same result, fuel and memory either way.
fn check_modes(p: &Prog, iters: u32, fuel: u64) -> Result<(), String> {
    let a = outcome(p, true, iters, fuel, 256 << 20);
    let b = outcome(p, false, iters, fuel, 256 << 20);
    if a == b { Ok(()) } else { Err(format!("fast loop {a:?} vs full path {b:?}")) }
}

/// The bound on CPU per unit of fuel, as a multiple of plain code (`S5B_COST_BOUND`).
fn bound() -> f64 {
    std::env::var("S5B_COST_BOUND").ok().and_then(|s| s.parse().ok()).unwrap_or(10.0)
}

/// How a program fails the property, if it does.
#[derive(Clone, Copy)]
struct Config {
    fuel: u64,
    mem: usize,
    base: f64,
    bound: f64,
}

fn verdict(p: &Prog, c: Config) -> Option<String> {
    let m = measure(p, c.fuel, c.mem, true).ok()?;
    // Memory: the process may hold a few times the cap (the allocator keeps freed
    // pages, setup peaks), but not far past it.
    let slack = 3 * c.mem + (96 << 20);
    if m.rss_gain > slack {
        return Some(format!("resident memory grew {} MB with {} MB accounted (cap {} MB)", m.rss_gain >> 20, m.used >> 20, c.mem >> 20));
    }
    if m.cpu <= c.bound * c.base {
        return None;
    }
    let med = median3(p, c.fuel, c.mem).ok()?;
    (med > c.bound * c.base).then(|| format!("{:.1}x plain code per unit of fuel ({med:.1} vs {:.1})", med / c.base, c.base))
}

/// Shrinks a failing program: drops operations and shapes, halves sizes, while it fails.
fn minimize(mut p: Prog, c: Config) -> (Prog, String) {
    let mut why = verdict(&p, c).unwrap_or_else(|| "flaky".into());
    let mut changed = true;
    let mut tries = 0;
    while changed && tries < 60 {
        changed = false;
        let mut cands = Vec::new();
        for i in 0..p.ops.len() {
            if p.ops.len() > 1 {
                let mut q = p.clone();
                q.ops.remove(i);
                cands.push(q);
            }
        }
        for i in 0..p.shapes.len() {
            let mut q = p.clone();
            q.shapes.remove(i);
            cands.push(q);
            if p.shapes[i].1 > 1 {
                let mut q = p.clone();
                q.shapes[i].1 /= 2;
                cands.push(q);
            }
        }
        for q in cands {
            tries += 1;
            if let Some(w) = verdict(&q, c) {
                p = q;
                why = w;
                changed = true;
                break;
            }
        }
    }
    (p, why)
}

/// Runs `n` programs from `seed` (or until `deadline`): the failures, minimized.
fn run(seed: u64, n: usize, scale: u32, c: Config, deadline: Option<Instant>, log: bool) -> (Vec<String>, usize) {
    let mut r = Rng::new(seed);
    let mut bad = Vec::new();
    let mut done = 0;
    for k in 0..n {
        if deadline.is_some_and(|d| Instant::now() > d) {
            break;
        }
        let p = Prog::generate(&mut r, scale);
        let t0 = Instant::now();
        done += 1;
        if log {
            println!("   ... #{k} {}", p.describe());
        }
        if let Err(e) = check_modes(&p, 8, 400_000) {
            bad.push(format!("seed {seed} #{k}: {}\n  {e}\n  setup: {}\n  ops: {}", p.describe(), p.setup(), p.ops_text()));
            continue;
        }
        match measure(&p, c.fuel, c.mem, true) {
            Err(e) => {
                // (A failed setup is reported whether or not the run logs: a shape that
                // does not fit is a generator bug to size, not a finding.)
                if log || e.starts_with("setup:") {
                    println!("skip seed {seed} #{k} {}: {e}", p.describe());
                }
                continue;
            }
            Ok(m) if log => println!("{:6.1}x #{k} ({} ms) {}", m.cpu / c.base, t0.elapsed().as_millis(), p.describe()),
            Ok(_) => {}
        }
        if verdict(&p, c).is_some() {
            let (q, why) = minimize(p.clone(), c);
            let msg = format!("seed {seed} #{k}: {why}\n  minimized: {}\n  from: {}\n  setup: {}\n  ops: {}", q.describe(), p.describe(), q.setup(), q.ops_text());
            if log {
                println!("FLAGGED {msg}");
            }
            bad.push(msg);
        }
    }
    (bad, done)
}

fn fresh_seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos() as u64)
}

/// The quick run: a fixed seed and a fresh one, small shapes, on a 64 KiB stack.
#[test]
fn cost_fuzz_quick() {
    super::common::small_stack(|| {
        let fuel = 100_000;
        let c = Config { fuel, mem: 64 << 20, base: baseline(fuel), bound: bound() };
        let seeds = match std::env::var("S5B_COST_SEED").ok().and_then(|s| s.parse().ok()) {
            Some(s) => vec![s],
            None => vec![0x5b_c057, fresh_seed()],
        };
        let mut bad = Vec::new();
        for (k, (shapes, ops)) in FIXED.iter().enumerate() {
            let p = Prog::named(shapes, ops);
            if let Err(e) = check_modes(&p, 4, 400_000) {
                bad.push(format!("fixed #{k}: {}\n  {e}", p.describe()));
            }
            match measure(&p, c.fuel, c.mem, true) {
                Ok(m) if std::env::var("S5B_COST_LOG").is_ok() => {
                    println!("{:6.1}x fixed #{k} {}", m.cpu / c.base, p.describe())
                }
                Ok(_) => {}
                Err(e) => bad.push(format!("fixed #{k}: {}: {e}", p.describe())),
            }
            if let Some(why) = verdict(&p, c) {
                bad.push(format!("fixed #{k}: {why}\n  {}\n  setup: {}\n  ops: {}", p.describe(), p.setup(), p.ops_text()));
            }
        }
        for &s in &seeds {
            bad.extend(run(s, 24, 4, c, None, std::env::var("S5B_COST_LOG").is_ok()).0);
        }
        assert!(bad.is_empty(), "seeds {seeds:?}, plain code {:.1} cpu/unit, bound {}x:\n{}", c.base, c.bound, bad.join("\n\n"));
    });
}

/// The long driver: `S5B_COST_SECS` seconds (default 300) from `S5B_COST_SEED`, bigger
/// shapes, a 256 MB cap.
#[test]
#[ignore = "long run"]
fn cost_fuzz_long() {
    super::common::small_stack(|| {
        let secs: u64 = std::env::var("S5B_COST_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(300);
        let seed: u64 = std::env::var("S5B_COST_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or_else(fresh_seed);
        let scale: u32 = std::env::var("S5B_COST_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
        let fuel = 400_000;
        let mut c = Config { fuel, mem: 256 << 20, base: baseline(fuel), bound: bound() };
        let deadline = Instant::now() + std::time::Duration::from_secs(secs);
        let mut bad = Vec::new();
        let mut total = 0;
        let mut s = seed;
        while Instant::now() < deadline {
            let (b, n) = run(s, 50, scale, c, Some(deadline), true);
            bad.extend(b);
            total += n;
            s = s.wrapping_add(1);
            c.base = baseline(fuel);
        }
        println!("{total} programs from seed {seed} to {s} in {secs} s, plain code {:.1} cpu/unit", c.base);
        assert!(bad.is_empty(), "{} flagged:\n{}", bad.len(), bad.join("\n\n"));
    });
}

/// Probe (not a test): one program's cost from `S5B_SETUP` and `S5B_BODY`.
#[test]
#[ignore = "probe"]
fn cost_probe() {
    super::common::small_stack(|| {
        let setup = std::env::var("S5B_SETUP").unwrap_or_default();
        let body = std::env::var("S5B_BODY").unwrap_or_default();
        let fuel = std::env::var("S5B_FUEL").ok().and_then(|s| s.parse().ok()).unwrap_or(300_000);
        let base = baseline(fuel);
        for fast in [true, false] {
            let mut lua = state(256 << 20, fast);
            lua.exec(format!("{PRELUDE} {setup}"), "=setup").unwrap();
            lua.set_limits(lua.limits().fuel(fuel));
            let f = lua.load(format!("while true do I = I + 1 {body} end"), "=probe").unwrap();
            let start = remedy_lua_oracle::thread_cpu();
            let r = lua.call(f.value(), &[]);
            let t = remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64;
            let used = fuel - lua.fuel_left();
            println!("fast {fast}: {:.1}x plain ({:.1} vs {base:.1} cpu/unit, {used} fuel), {r:?}", t / used.max(1) as f64 / base, t / used.max(1) as f64);
        }
    });
}

/// Each program costs at most `bound()` times plain code per unit of fuel, with the fast
/// loop on and off (the best of three when over: the machine is shared), and the same
/// result, fuel and memory either way.
fn assert_metered(progs: &[Prog], fuel: u64, mem: usize) {
    let base = baseline(fuel);
    let mut bad = Vec::new();
    for p in progs {
        if let Err(e) = check_modes(p, 4, 1 << 30) {
            bad.push(format!("{}: {e}", p.describe()));
        }
        for fast in [true, false] {
            let mut cpu = f64::MAX;
            for _ in 0..3 {
                cpu = cpu.min(measure(p, fuel, mem, fast).map_or_else(|e| panic!("{}: {e}", p.describe()), |m| m.cpu));
                if cpu <= bound() * base {
                    break;
                }
            }
            if cpu > bound() * base {
                bad.push(format!("fast {fast}: {:.1}x plain code per unit of fuel: {}", cpu / base, p.describe()));
            }
        }
    }
    assert!(bad.is_empty(), "plain code {base:.1} cpu/unit:\n{}", bad.join("\n"));
}

/// Long-run harness finding: a program whose
/// setup failed (the "dead objects" shape's weak-table kind at 2^21 is over the 256 MiB
/// cap) panicked the driver (`fuel - fuel_left` with setup's fuel left). Every kind of the
/// shape now fits alone at the largest size drawn, and a setup that does not fit is an
/// outcome: the same in both modes, skipped by `measure`, never a panic.
#[test]
fn s5b_cost_harness_failed_setup_is_an_outcome() {
    let dead = SHAPES.iter().position(|s| s.0 == "dead objects, few survivors").unwrap();
    for a in 0..6u32 {
        let p = Prog { shapes: vec![(dead, 1 << 21, a)], ops: vec![] };
        let mut lua = state(256 << 20, true);
        lua.exec(PRELUDE, "=prelude").unwrap();
        assert!(lua.exec(p.setup(), "=setup").is_ok(), "kind {a} at 2^21 does not fit the cap");
    }
    // A setup that does not fit (the weak-table kind in 8 MiB): an outcome, the same in
    // both modes, with no fuel used; `measure` skips it.
    let p = Prog { shapes: vec![(dead, 1 << 21, 5)], ops: vec![0] };
    let (a, b) = (outcome(&p, true, 4, 400_000, 8 << 20), outcome(&p, false, 4, 400_000, 8 << 20));
    assert!(a.0.starts_with("setup:") && a.1 == 0, "{a:?}");
    assert_eq!(a, b);
    let m = measure(&p, 400_000, 8 << 20, true);
    assert!(m.as_ref().is_err_and(|e| e.starts_with("setup:")), "{:?}", m.map(|m| m.cpu));
}

/// S5B-COST-1: the collector walked every slot of a thread's stack (and cleared the
/// running one's dead part) but counted only the objects it found, so a stack of plain
/// values was walked for free at every cycle: a coroutine parked with 131,072 integer
/// arguments under `collectgarbage('step')` ran at 60x to 80x plain code per unit of
/// fuel, the running thread's own stack at 30x, coroutines parked with 200 open upvalues
/// a frame at 13x to 15x under `collectgarbage()`; more with a bigger stack (the memory
/// cap's). Every slot walked is work now.
#[test]
fn s5b_cost_1_collector_charges_every_stack_slot() {
    super::common::small_stack(|| {
        assert_metered(
            &[
                Prog::named(&[("coroutine parked with big varargs", 1 << 18, 0)], &["collector step"]),
                Prog::named(&[("main stack: big varargs", 1 << 18, 0)], &["collector step"]),
                Prog::named(&[("main stack: big varargs", 1 << 17, 0)], &["full collection"]),
                Prog::named(&[("open upvalues parked", 1500, 0)], &["full collection"]),
            ],
            200_000,
            256 << 20,
        );
    });
}

/// S5B-COST-2: a table one key short of a power of two rehashes at every new key (C
/// Lua's sizes, kept so `#` finds C Lua's border), and a rehash was charged a sixteenth
/// of a unit per slot while it makes several passes and places every key: adding and
/// removing a new integer key ran at 8x plain code per unit of fuel at 4,000 keys and 30x
/// at a million, putting a string key in and out first (the integer keys move to the
/// other part and back) at 47x. Every pass and placement is charged now, and the charge
/// does not depend on the hash seed.
#[test]
fn s5b_cost_2_rehash_at_every_new_key_is_charged() {
    super::common::small_stack(|| {
        assert_metered(
            &[
                Prog::named(&[("one key short of a power of two", 1 << 18, 0)], &["churn a new integer key"]),
                Prog::named(&[("one key short of a power of two", 1 << 18, 0)], &["toggle a string key", "churn a new integer key"]),
                Prog::named(&[("one key short of a power of two", 1 << 17, 1)], &["churn a new integer key"]),
                Prog::named(&[("one key short of a power of two", 1 << 17, 2)], &["toggle a string key"]),
            ],
            2_000_000,
            256 << 20,
        );
        // The same fuel under any hash seed.
        let prog = "local t = {} for i = 1, (1 << 12) - 1 do t[i * 3] = i end \
                    for i = 1, (1 << 10) - 1 do t['s' .. i] = i end \
                    for i = 1, 40 do t.zz = 1 t.zz = nil t[-i] = 1 t[-i] = nil t[i + 0.5] = 1 t[i + 0.5] = nil end return #t";
        let fuel = |seed: u64| {
            let mut lua = Lua::new(Limits::default().fuel(1 << 40).hash_seed(seed), Libs::ALL);
            lua.exec(prog, "=t").unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        let f0 = fuel(1);
        for seed in [2, 3, 0xdead_beef, u64::MAX] {
            assert_eq!(fuel(seed), f0, "seed {seed}");
        }
    });
}

/// S5B-COST-3: an ephemeron entry was one unit of collector work, as a strong table's
/// slot is, but the atomic step looks its key up in a map twice (to wait, then when the
/// key is traversed), a cache miss each in a big map: `collectgarbage('step')` over a
/// weak-keyed chain of 800,000 entries ran at 13x plain code per unit of fuel (5x at
/// 100,000; the same chain in a strong table 5x at 800,000). Charged by entry now, and
/// by entry only (which entries wait follows the seed), so fuel stays the same under any
/// seed.
#[test]
fn s5b_cost_3_ephemeron_entries_are_charged() {
    super::common::small_stack(|| {
        let eph = Prog::named(&[("ephemeron chain", 800_000, 0)], &["collector step"]);
        assert_metered(std::slice::from_ref(&eph), 5_000_000, 256 << 20);
        // Against the same chain in a strong table (the cache misses of a big heap are
        // the same; the ephemeron's own work is what differs): at most 1.35x, the best of
        // three (it was 1.5x to 2x; now about 1x).
        let strong = Prog::named(&[("ephemeron chain", 800_000, 4)], &["collector step"]);
        for fast in [true, false] {
            let mut ratio = f64::MAX;
            for _ in 0..3 {
                // 200M units: whole collection cycles. One atomic step over this chain is
                // charged tens of millions of units now (S6GC-2d) and cannot stop on fuel
                // (S6GC-2e, open), so a 5M window measured that one step's overrun, not
                // the cost per unit.
                let cpu = |p: &Prog| measure(p, 200_000_000, 256 << 20, fast).unwrap().cpu;
                ratio = ratio.min(cpu(&eph) / cpu(&strong));
                if ratio <= 1.35 {
                    break;
                }
            }
            assert!(ratio <= 1.35, "fast {fast}: an ephemeron entry costs {ratio:.1}x a strong one per unit of fuel");
        }
        let prog = "local ks = {} for i = 1, 3001 do ks[i] = {} end local e = setmetatable({}, {__mode = 'k'}) \
                    for i = 3000, 1, -1 do e[ks[i]] = ks[i + 1] end local root = ks[1] ks = nil \
                    for i = 1, 3 do collectgarbage() end local n = 0 for k in pairs(e) do n = n + 1 end return n, root ~= nil";
        let fuel = |seed: u64| {
            let mut lua = Lua::new(Limits::default().fuel(1 << 40).hash_seed(seed), Libs::ALL);
            lua.exec(prog, "=t").unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        let f0 = fuel(1);
        for seed in [2, 3, 0xdead_beef, u64::MAX] {
            assert_eq!(fuel(seed), f0, "seed {seed}");
        }
    });
}

/// LR-8: a coroutine's stack grew to a register window at its first resume (about 4.5 KiB)
/// and to twice that when `coroutine.close` ran its `__close` handler, and a closed or
/// finished coroutine kept it until the collector freed the thread: with the collector
/// stopped (or under a long pause) every new coroutine touched about 10 KiB of fresh
/// pages, 10x to 20x plain code per unit of fuel on a process's first runs (19.6x with
/// coroutines parked in coroutines at a main stack 64 calls deep). And the handler ran
/// above the closer's stack top, not the closed coroutine's, so closing one from a
/// coroutine 15,000 slots deep grew the closed one's stack to 240 KiB, copied at every
/// `coroutine.close` (15x). Handlers run above the closed coroutine's own live slots now,
/// and a closed or finished coroutine's stack goes to the next one. One left suspended (a
/// `wrap` that yielded, 9x with the collector stopped) cannot give its stack back: the
/// window a new coroutine maps is charged.
#[test]
fn s5b_cost_4_new_coroutines_reuse_closed_stacks() {
    super::common::small_stack(|| {
        assert_metered(
            &[
                Prog::named(&[("collector settings", 1, 4)], &["new coroutine closed"]),
                Prog::named(
                    &[("main stack: deep", 64, 5), ("collector settings", 1, 4), ("coroutines parked in coroutines", 2, 2)],
                    &["new coroutine closed"],
                ),
                Prog::named(&[("collector settings", 1, 4), ("main stack: big varargs", 15165, 5)], &["new coroutine closed"]),
                Prog::named(&[("collector settings", 1, 1), ("main stack: deep", 453, 0)], &["new coroutine closed", "lookup"]),
                Prog::named(
                    &[("array of strings", 13249, 2), ("main stack: big varargs", 4, 0), ("collector settings", 1, 4)],
                    &["closures", "new coroutine closed", "status parked"],
                ),
                Prog::named(&[("collector settings", 1, 4)], &["new coroutine"]),
                // Seed 13867 #49 (scale 4) and seed 11645 #48: one `wrap` left suspended
                // per iteration.
                Prog::named(&[("collector settings", 1, 4)], &["wrap"]),
                Prog::named(&[("collector settings", 1, 4), ("array of strings", 1, 0)], &["wrap"]),
                Prog::named(&[("collector settings", 1, 4), ("main stack: deep", 64, 0)], &["wrap"]),
                // Seed 530394 #5 (scale 7).
                Prog::named(
                    &[("dead objects, few survivors", 211, 1), ("collector settings", 1, 4), ("main stack: big varargs", 212, 5)],
                    &["new coroutine closed"],
                ),
                // Seed 7043 #19 (a 1000% pause) and seed 6331 #4.
                Prog::named(
                    &[("main stack: big varargs", 9535, 3), ("weak table", 68, 3), ("collector settings", 1, 1)],
                    &["new coroutine closed", "require missing", "garbage"],
                ),
                Prog::named(
                    &[("main stack: deep", 28, 0), ("pending to-be-closed variables", 43, 1), ("collector settings", 1, 4)],
                    &["churn a new integer key", "new coroutine closed"],
                ),
            ],
            400_000,
            256 << 20,
        );
        // The memory each one leaves while the collector is stopped, from a shallow stack
        // and from one 15,000 slots deep (it was 10 KiB and 250 KiB): a coroutine's record,
        // its function and the handler's tables (C Lua: 960 bytes). Spare stacks count.
        for depth in [0, 15_000] {
            let mut lua = Lua::new(Limits::default().memory(1 << 30), Libs::ALL);
            lua.exec(
                format!(
                    "collectgarbage('stop') local a = {{}} for i = 1, {depth} do a[i] = i end \
                     local function ops(n, ...) for i = 1, n do \
                     local co = coroutine.create(function(a) local x <close> = setmetatable({{}}, {{__close = function() end}}) coroutine.yield(a) end) \
                     coroutine.resume(co, 1) coroutine.close(co) coroutine.wrap(function() end)() end end \
                     function RUN(n) local m = coroutine.create(function() ops(n, table.unpack(a)) end) assert(coroutine.resume(m)) end"
                ),
                "=setup",
            )
            .unwrap();
            lua.exec("RUN(50)", "=warm").unwrap();
            let before = lua.memory_used();
            lua.exec("RUN(2000)", "=ops").unwrap();
            let per = (lua.memory_used() - before) / 2000;
            assert!(per <= 1600, "depth {depth}: {per} bytes held per coroutine made and closed");
        }
        // A `__close` handler runs above the closed coroutine's live slots, not above the
        // closer's (it grew the closed one's stack by 240 KiB here).
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        let grew = lua
            .exec(
                "local a = {} for i = 1, 15000 do a[i] = i end local inside \
                 local function f(...) \
                 local co = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() inside = collectgarbage('count') end}) coroutine.yield() end) \
                 coroutine.resume(co) local before = collectgarbage('count') coroutine.close(co) return (inside - before) * 1024 end \
                 return coroutine.wrap(function() return f(table.unpack(a)) end)()",
                "=close",
            )
            .unwrap();
        let grew = match grew.first() {
            Some(remedy_lua::Value::Num(n)) => *n,
            Some(remedy_lua::Value::Int(n)) => *n as f64,
            other => panic!("{other:?}"),
        };
        assert!(grew < 16_384.0, "closing grew the coroutine's stack by {grew} bytes");
        // A coroutine left suspended pays for its window (it was 21 units with the
        // collector stopped: the collector's cycles had paid for the memory).
        let mut lua = Lua::new(Limits::default().fuel(1 << 40), Libs::ALL);
        lua.exec("collectgarbage('stop') for i = 1, 1000 do local w = coroutine.wrap(function(...) coroutine.yield(select('#', ...)) end) w(1, 2, 3) end", "=wrap")
            .unwrap();
        let per = ((1u64 << 40) - lua.fuel_left()) / 1000;
        assert!(per >= 45, "{per} units per coroutine made and left suspended");
        // A spare stack stays counted (resident until reused or a full collection): closing
        // a coroutine gives back its frame records only, and a full collection all of it.
        let mut lua = Lua::new(Limits::default(), Libs::ALL);
        let r = lua
            .exec(
                "collectgarbage('stop') local co = coroutine.create(function() coroutine.yield() end) coroutine.resume(co) \
                 local before = collectgarbage('count') coroutine.close(co) local after = collectgarbage('count') \
                 co = nil collectgarbage() collectgarbage('stop') local base = collectgarbage('count') \
                 return (before - after) * 1024, (after - base) * 1024",
                "=spare",
            )
            .unwrap();
        let num = |v: Option<&remedy_lua::Value>| match v {
            Some(remedy_lua::Value::Num(n)) => *n,
            Some(remedy_lua::Value::Int(n)) => *n as f64,
            other => panic!("{other:?}"),
        };
        let (gave, held) = (num(r.first()), num(r.get(1)));
        assert!(gave < 1024.0, "closing gave {gave} bytes back while its stack is kept");
        assert!(held >= 4096.0, "a full collection gave back {held} bytes (the stack and the thread)");
        // `wrap` makes a thread as `create` does (and a function): it costs no less.
        let made = |make: &str| {
            let mut lua = Lua::new(Limits::default().fuel(1 << 40), Libs::ALL);
            lua.exec(format!("local f = function() end for i = 1, 1000 do local c = {make}(f) end"), "=make").unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        assert!(made("coroutine.wrap") >= made("coroutine.create"), "wrap {} < create {}", made("coroutine.wrap"), made("coroutine.create"));
        // The fuel is the same under any hash seed.
        let prog = "collectgarbage('stop') for i = 1, 300 do \
                    local co = coroutine.create(function(a) local x <close> = setmetatable({}, {__close = function() end}) coroutine.yield(a) end) \
                    coroutine.resume(co, 1) coroutine.close(co) local w = coroutine.wrap(function(b) return b end) w(i) end";
        let fuel = |seed: u64| {
            let mut lua = Lua::new(Limits::default().fuel(1 << 40).hash_seed(seed), Libs::ALL);
            lua.exec(prog, "=t").unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        let f0 = fuel(1);
        for seed in [2, 3, 0xdead_beef, u64::MAX] {
            assert_eq!(fuel(seed), f0, "seed {seed}");
        }
    });
}
