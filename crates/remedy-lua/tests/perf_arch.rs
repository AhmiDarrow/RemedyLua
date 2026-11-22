//! Structural speed: the paths its changes touch, checked against
//! C Lua with the fast loop on and off (same results, fuel and memory: `common::remedy`).

use crate::common::{oracle, remedy};

fn same_as_c(program: &str) {
    let ours = remedy(program, 50_000_000);
    let theirs = oracle(program, 50_000_000);
    assert_eq!(ours, theirs, "differs from C Lua:\n{program}");
}

/// `field!` (the fast loop's `GetTableK` and `GetTabUp`) evaluates its constant key once:
/// reads by every kind of constant key (short and long strings, integers in and past the
/// array part, floats with and without an integer value, booleans), hits and misses, at a
/// warm and a stale hint, through `__index` tables and functions, on globals and on
/// upvalue tables, give C Lua's results.
#[test]
fn constant_key_reads_of_every_kind() {
    same_as_c(
        r#"
local log = {}
local function add(...) for i = 1, select('#', ...) do log[#log + 1] = tostring((select(i, ...))) end end
local long = string.rep("k", 50)
local t = {10, 20, 30, x = 1, y = 2, [long] = "L", [2.5] = "f", [true] = "T", [false] = "F", [100] = "h"}
for i = 1, 3 do
  add(t.x, t.y, t.z, t[1], t[3], t[4], t[100], t[101], t[2.5], t[2.0], t[3.0], t[true], t[false])
  add(t["kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk"])
  -- move the fields: the hints are stale on the next pass
  t.x, t.y = nil, nil
  for j = 1, 20 do t["n" .. j] = j end
  t.x, t.y = i, -i
end
local base = {m = "base", [1] = "b1", [7] = "b7"}
local mid = setmetatable({n = "mid"}, {__index = base})
local obj = setmetatable({}, {__index = mid})
local fobj = setmetatable({}, {__index = function(_, k) return "fn:" .. tostring(k) end})
for i = 1, 3 do
  add(obj.m, obj.n, obj.q, obj[1], obj[7], obj[8], fobj.a, fobj[3], fobj[1.5])
end
g1, g2 = "G1", 2
for i = 1, 3 do add(g1, g2, g3, math.pi > 3, string.len("abc")) end
local up = {a = 1}
local function f() return up.a, up.b, up[1] end
for i = 1, 3 do add(f()); up.b = i; up[1] = -i end
-- a non-table operand: the full path's error, with the key's name
add(pcall(function() local s = nil; return s.field end))
add(pcall(function() local n = 5; return n.k end))
add(("x"):len(), (""):rep(3) == "")
return table.concat(log, " ")
"#,
    );
}

/// The field read's inline-cache hit (`field_hit!`) takes only a string constant held at
/// the hinted node with a value: a key left at its node with a nil value (assigned nil)
/// still asks `__index`, a table rebuilt under the same key moves the node, globals in a
/// replaced `_ENV` and a method's table read through a hint warmed on another table all
/// give C Lua's results.
#[test]
fn field_cache_hit_only_with_a_value() {
    same_as_c(
        r#"
local log = {}
local function add(...) for i = 1, select('#', ...) do log[#log + 1] = tostring((select(i, ...))) end end
local proto = {v = "from __index", w = "w"}
local t = setmetatable({v = 1, w = 2}, {__index = proto})
local function rd(x) return x.v, x.w end
for i = 1, 6 do
  add(rd(t))
  if i == 2 then t.v = nil end          -- key stays at its node, value nil: __index answers
  if i == 4 then t.v = i end            -- back in the same node
  if i == 5 then t = setmetatable({w = 0, a = 1, b = 2, v = "new"}, getmetatable(t)) end
end
-- one read site, tables of different shapes: the hint is wrong half the time
local shapes = {{v = 1}, {a = 1, b = 2, v = 2}, setmetatable({}, {__index = {v = 3}}), {v = false}, {v = nil}}
for r = 1, 3 do for _, s in ipairs(shapes) do add((rd(s))) end end
-- globals through a replaced environment
local function env_reader(e) local _ENV = e; return function() return gx, gy end end
local e1 = {gx = 1, gy = 2}
local f = env_reader(e1)
for i = 1, 3 do add(f()); e1.gx = nil end
local e2 = setmetatable({}, {__index = function(_, k) return k .. "!" end})
local g = env_reader(e2)
for i = 1, 3 do add(g()); e2.gy = i end
return table.concat(log, " ")
"#,
    );
}

/// `store!` tries the hinted node before checking that a string key is interned (a key the
/// node holds is the table's, so interned): writes by short and long string constants,
/// to live fields, dead fields (assigned nil), new keys (a long constant not interned
/// yet is interned on the full path) and through `__newindex`, give C Lua's results.
#[test]
fn constant_key_writes_with_long_and_short_keys() {
    same_as_c(
        r#"
local log = {}
local t = {}
local mt = {__newindex = function(t, k, v) rawset(t, k, "ni:" .. tostring(v)) end}
local u = setmetatable({}, mt)
for i = 1, 6 do
  t.short = i
  t.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa = i * 2
  t["bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" .. i % 2] = i
  if i == 3 then t.short = nil; t.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa = nil end
  u.short = i
  u.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc = i
  if i == 4 then u.short = nil end
  log[#log + 1] = tostring(t.short) .. "/" .. tostring(t.aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)
    .. "/" .. tostring(u.short) .. "/" .. tostring(u.cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc)
end
local n = 0
for k, v in pairs(t) do n = n + 1 end
log[#log + 1] = n
return table.concat(log, " ")
"#,
    );
}

/// A field read whose destination register is its table's (`node = node.next`, what a
/// list walk compiles to) and whose result is then used at once, on the cache's hit and
/// miss branches (each writes the register itself): C Lua's results.
#[test]
fn field_read_into_its_own_table_register() {
    same_as_c(
        r#"
local head
for i = 1, 50 do head = {v = i, next = head} end
local s, node = 0, head
while node do s = s + node.v; node = node.next end
local alt = setmetatable({}, {__index = {next = false, v = 0}})
local mixed = {v = 1, next = {v = 2, next = alt}}
local n, t = 0, mixed
for i = 1, 4 do if t then n = n + t.v; t = t.next end end
return s, n, tostring(t)
"#,
    );
}

/// `field_hit!`'s integer case (`t[1]` by a constant): a live slot inside the array part
/// only; a nil slot (a hole), a slot past Lua's border hint, an integer key in the hash
/// part, a negative or zero constant and a table with `__index` all take `field!`, with
/// C Lua's results.
#[test]
fn integer_constant_reads_in_and_out_of_the_array_part() {
    same_as_c(
        r#"
local log = {}
local function add(...) for i = 1, select('#', ...) do log[#log + 1] = tostring((select(i, ...))) end end
local t = {1, 2, nil, 4}
local h = {[1] = "a", [2] = "b", [3] = "c"}
local m = setmetatable({1}, {__index = function(_, k) return "ix" .. k end})
for r = 1, 3 do
  add(t[1], t[2], t[3], t[4], t[5], t[0], t[-1])
  add(h[1], h[2], h[3], h[4])
  add(m[1], m[2], m[3])
  t[3] = r; t[5] = r * 10; h[4] = r
  if r == 2 then t[1] = nil; m[1] = nil end
end
local grow = {}
for i = 1, 40 do grow[i] = i; add(grow[1], grow[2], grow[17], grow[33]) end
return table.concat(log, " ")
"#,
    );
}

/// A new table fills a freed slot in place (`Slab::take_free`): tables made in reused
/// slots in every collector phase (pause, mark, sweep, stepped by hand and by allocation),
/// with array and hash parts of every size and metatables, keep their contents, their
/// identity and their liveness, as in C Lua.
#[test]
fn tables_made_in_reused_slots_in_every_phase() {
    same_as_c(
        r#"
local keep, sum = {}, 0
for round = 1, 30 do
  for i = 1, 200 do
    local t = (i % 3 == 0) and {i, i + 1, i + 2} or (i % 3 == 1) and {a = i, b = {i}} or setmetatable({}, {__index = {z = i}})
    if i % 17 == 0 then keep[#keep + 1] = t end
  end
  if round % 3 == 0 then collectgarbage("step", 1) end
  if round % 10 == 0 then collectgarbage() end
end
for _, t in ipairs(keep) do
  sum = sum + (t[1] or 0) + (t[3] or 0) + (t.a or 0) + (t.b and t.b[1] or 0) + (t.z or 0)
end
local seen, distinct = {}, 0
for _, t in ipairs(keep) do if not seen[t] then seen[t] = true; distinct = distinct + 1 end end
return #keep, distinct, sum
"#,
    );
}

/// A Lua metamethod's return to the instruction that called it, in the fast loop: its
/// first result stored (`__index`, `__add`, `__len`, `__unm`, `__concat` on the full path),
/// dropped (`__newindex`) or deciding the jump (`__eq`, `__lt`, `__le`, both ways), with
/// no results, several, `nil` and `false`; with an upvalue of the metamethod open, a
/// to-be-closed variable in it, a yield inside it and an error out of it: C Lua's results.
#[test]
fn metamethod_returns_apply_their_continuation() {
    same_as_c(
        r#"
local log = {}
local function add(...) for i = 1, select('#', ...) do log[#log + 1] = tostring((select(i, ...))) end end
local M = {}
M.__index = function(t, k) if k == "none" then return end if k == "many" then return 1, 2, 3 end return k .. "!" end
M.__newindex = function(t, k, v) rawset(t, k, v and v * 2) return "ignored" end
M.__add = function(a, b) return (rawget(a, "n") or 0) + (type(b) == "table" and rawget(b, "n") or b) end
M.__len = function(a) return rawget(a, "n") end
M.__unm = function(a) return false end
M.__eq = function(a, b) return rawget(a, "n") == rawget(b, "n") end
M.__lt = function(a, b) if rawget(a, "n") == 3 then return end return rawget(a, "n") < rawget(b, "n"), "extra" end
M.__le = function(a, b) local f = function() return a end; return rawget(a, "n") <= rawget(b, "n") and f() end
M.__concat = function(a, b) return "cat" end
local function new(n) return setmetatable({n = n}, M) end
for i = 1, 5 do
  local a, b = new(i), new(5 - i)
  a.x = i
  add(a.x, a.foo, a.none, a.many, a + b, a + 1, #a, -a, a .. "s")
  if a == b then add("eq") else add("ne") end
  if a < b then add("lt") else add("nlt") end
  if a <= b then add("le") else add("nle") end
  if not (a < b) then add("!lt") end
end
-- a to-be-closed variable inside the metamethod, and an upvalue left open
local T = setmetatable({}, {__index = function(t, k)
  local c <close> = setmetatable({}, {__close = function() add("closed " .. k) end})
  local up = k
  local get = function() return up end
  return get() .. "?"
end})
for i = 1, 3 do add(T["k" .. i]) end
-- a yield inside a metamethod, and an error out of one
local Y = setmetatable({}, {__index = function(t, k) return coroutine.yield(k) end})
local co = coroutine.wrap(function() return Y.a .. Y.b end)
add(co(), co("1"), co("2"))
local E = setmetatable({}, {__lt = function() error("bad lt") end})
add(pcall(function() return E < E end))
return table.concat(log, " ")
"#,
    );
}

/// `Return0` and `Return1` (`Proto::specialize_returns`, as C Lua's `luaK_finish`): returns
/// of no value and of one value from functions that are not vararg and need nothing
/// closed, to Lua callers wanting none, one, several or all results; through `pcall`,
/// coroutines (resume, wrap, yield across), callbacks from `table.sort` and `gsub`,
/// metamethods and tail calls; and the functions that keep the generic `Return`: vararg
/// ones, ones with a to-be-closed variable and ones whose locals a closure captures. C Lua's
/// results, the same fuel and memory on both paths.
#[test]
fn return0_and_return1_in_every_context() {
    same_as_c(
        r#"
local log = {}
local function add(...) local n = select('#', ...) log[#log + 1] = n for i = 1, n do log[#log + 1] = tostring((select(i, ...))) end end
local function one(x) return x * 2 end
local function none(x) local y = x end
local function nothing() return end
add(one(3)); add(none(3)); add(nothing())
local a, b, c = one(4); add(a, b, c)
local t = {one(5), none(5), one(6)}; add(#t, t[1], t[3])
add(pcall(one, 7)); add(pcall(none, 7)); add(select('#', pcall(nothing)))
add(pcall(function() return one(nil) end))
local co = coroutine.wrap(function(x) local y = coroutine.yield(one(x)) return one(y) end)
add(co(10)); add(co(20))
local co2 = coroutine.create(none); add(coroutine.resume(co2, 1)); add(coroutine.status(co2))
local s = {5, 3, 9, 1}; table.sort(s, function(p, q) return p > q end); add(table.concat(s, ","))
add((string.gsub("abc", "%w", function(ch) return ch:upper() end)))
local M = setmetatable({}, {__index = function(_, k) return k .. "!" end, __lt = function() return true end})
add(M.key, M < M)
local function tail(x) return one(x) end; add(tail(8))
local function va(...) return ... end; add(va(1, 2)); add(va())
local function tbc() local c <close> = setmetatable({}, {__close = function() add("closed") end}) return 1 end; add(tbc())
local function cap() local u = 1 local f = function() return u end return f() end; add(cap())
local cl = setmetatable({}, {__close = function(_, e) add("for closed", e) end})
local function infor() for i in function(_, c) return (c or 0) + 1 end, nil, nil, cl do return i end end; add(infor())
local function rec(n) if n == 0 then return end return rec(n - 1) end; add(rec(50))
local function deep(n) if n == 0 then return 0 end return 1 + deep(n - 1) end; add(deep(200))
return table.concat(log, " ")
"#,
    );
}

/// The running function read from its frame where used (`GetSelf`, upvalues, closures):
/// after a call, a return, a metamethod's return, a tail call and a coroutine switch, the
/// function in the frame is the one running. C Lua's results, the same fuel and memory on
/// both paths.
#[test]
fn running_function_after_every_kind_of_switch() {
    same_as_c(
        r#"
local log = {}
local function mk(tag)
  local n = 0
  local function self(d)
    n = n + 1
    if d > 0 then self(d - 1) end
    local M = setmetatable({}, {__index = function(_, k) return tag .. k end})
    local v = M.x
    local g = function() return tag .. n end
    return v .. g()
  end
  return self
end
local a, b = mk("a"), mk("b")
log[#log + 1] = a(3); log[#log + 1] = b(2); log[#log + 1] = a(0)
local function t(f, d) return f(d) end
log[#log + 1] = t(b, 1)
local co = coroutine.wrap(function(f) local r = f(1) coroutine.yield(r) return f(0) end)
log[#log + 1] = co(a); log[#log + 1] = co()
return table.concat(log, " ")
"#,
    );
}
