-- case: string to number coercion arith
return "10" + 1, "0x10" + 0, "1e1" * 1, " 3 " - 1, "10" // "3", "7" % "2", "2" ^ "3", 10 .. "", "3.0" + 0, "-0x8000000000000000" + 0
-- case: coercion failures
return pcall(function() return "abc" + 1 end), pcall(function() return {} + 1 end), pcall(function() return 1 + nil end)
-- case: string bitwise coercion
return "3" & 1, "0x10" | 0, "3.0" ~ 1, pcall(function() return "3.5" & 1 end)
-- case: float to int bitwise
return 3.0 & 1, pcall(function() return 3.5 | 0 end), pcall(function() return 2^63 | 0 end), pcall(function() return math.huge | 0 end), pcall(function() return (0/0) | 0 end)
-- case: integer overflow wraps
return math.maxinteger + 1 == math.mininteger, math.mininteger - 1, math.maxinteger * 2, -math.mininteger, math.mininteger // -1, math.mininteger % -1, math.abs(math.mininteger)
-- case: integer division and modulo signs
return 7 // 2, -7 // 2, 7 // -2, -7 // -2, 7 % 3, -7 % 3, 7 % -3, -7 % -3, 7.5 // 2, -7.5 // 2, 7.5 % 2, -7.5 % 2, 5.0 % -3, -0.0 % 1
-- case: division by zero
return pcall(function() return 1 // 0 end), pcall(function() return 1 % 0 end), 1 // 0.0, -1 // 0.0, 0/0 ~= 0/0, 1 % 0.0 ~= 1 % 0.0, 1.0 // 0, 5 % math.huge, -5 % math.huge, 5 % -math.huge, -5 % -math.huge
-- case: float modulo infinities
return -math.huge % 2 ~= -math.huge % 2, 3 % math.huge, -3 % math.huge, 0.0 % math.huge, 3 % -math.huge
-- case: exponent and division always float
return 2^2, 4/2, math.type(2^2), math.type(4/2), 0^0, 0^-1, (-2)^2, -2^2, 2^-1
-- case: shifts
return 1 << 63, 1 << 64, 1 << -1, -1 >> 1, -1 >> 63, -1 >> 64, 1 >> -63, 5 << 0, -1 << 70, 2 >> math.mininteger, 2 << math.mininteger, 1 << math.maxinteger
-- case: bitwise not and xor
return ~0, ~5, 5 ~ 3, -1 ~ math.mininteger, ~math.mininteger, ~"7", 5 & -1.0
-- case: int float comparisons
return 1 == 1.0, math.maxinteger < 2^63, math.maxinteger + 0.0 == 2^63, math.maxinteger == 2^63, math.mininteger == -2^63, 2^53 + 1 > 2^53, (2^53|0) + 1 > 2^53, 1 < 0/0, 1 > 0/0, math.mininteger < -2^63, math.maxinteger >= 2^63 - 1024
-- case: int float compare extremes
local a = math.maxinteger
return a < a + 0.0, a <= a + 0.0, a + 0.0 > a, (a - 1) < 2^63, -2^63 <= math.mininteger, -2^63 < math.mininteger, 9007199254740993 < 9007199254740992.0, 9007199254740993 > 9007199254740992.0, 9007199254740993 == 9007199254740992.0, math.mininteger <= -1e300, math.maxinteger < math.huge, math.mininteger > -math.huge, 1 <= 0/0
-- case: string comparison
return "a" < "b", "a" < "aa", "Z" < "a", "\0" < "\1", "a\0b" < "a\0c", "a\0" > "a", "" == "", pcall(function() return 1 < "2" end), "\xff" > "a", "a\0" <= "a"
-- case: mixed type compare errors
return pcall(function() return {} < {} end), pcall(function() return 1 < nil end), pcall(function() return "a" <= 1 end), pcall(function() return true < false end), pcall(function() return {} <= 1 end), pcall(function() return print < print end)
-- case: equality across types
return 1 == "1", "1" == 1, nil == false, {} == {}, 0.0 == -0.0, 1/0 == math.huge, 0/0 == 0/0
-- case: tostring of numbers
return tostring(1e100), tostring(-1e-100), tostring(2^63), tostring(-2^63), tostring(1e15), tostring(123456.789e3), tostring(0.1 + 0.2), tostring(100 / 2), tostring(-0.0), tostring(1e308 * 10), tostring(2^24), tostring(3 // 1.0), tostring(1e14), tostring(12345678901234567), tostring(-1e15), tostring(5e-324), tostring(2^-1074)
-- case: string to number edge
return tonumber("0x7fffffffffffffff"), tonumber("0xffffffffffffffff"), tonumber("0x10000000000000000"), tonumber("9223372036854775807"), tonumber("9223372036854775808"), tonumber("-9223372036854775808"), tonumber("-9223372036854775809"), tonumber("1e"), tonumber(".5"), tonumber("5."), tonumber("."), tonumber("0x.8"), tonumber("0x8p1"), tonumber("0x1P-2"), tonumber("inf"), tonumber("nan"), tonumber("  10  "), tonumber("10\0")
-- case: string to number more
return tonumber("0x"), tonumber("0xg"), tonumber("1e+"), tonumber("1e+5"), tonumber("- 1"), tonumber("-0x10"), tonumber("+5"), tonumber("+-5"), tonumber("0x1p"), tonumber("1E2"), tonumber("0X1F"), tonumber("\t\n\v\f\r 7 \t"), tonumber("1_000"), tonumber("08"), tonumber("1e400"), tonumber("-1e400"), tonumber("0x1.8p1"), tonumber("0x.1"), tonumber("0xA.8P0")
-- case: tonumber with base
return tonumber("7fffffffffffffff", 16), tonumber("ffffffffffffffff", 16), tonumber("-ff", 16), tonumber(" 11 ", 2), tonumber("1.5", 10), tonumber("Z", 36), tonumber("2", 2), tonumber("", 10), tonumber("-", 10), tonumber("10", 36), tonumber("10000000000000000000000", 16), tonumber("zzzzzzzzzzzzzzzzzz", 36), tonumber("  -  1", 10), tonumber("1 ", 10), tonumber("8", 8.0)
-- case: tonumber base errors
return pcall(tonumber, "10", 1), pcall(tonumber, "10", 37), pcall(tonumber, 10, 16), pcall(tonumber), pcall(tonumber, "10", 8.5), pcall(tonumber, "10", "16")
-- case: tonumber non-string
return tonumber(nil), tonumber(true), tonumber({}), tonumber(5), tonumber(5.5), math.type(tonumber("5")), math.type(tonumber("5.0"))
-- case: integer float key normalization
local t = {}
t[1.0] = "a"; t[2] = "b"; t[2^53] = "c"
return t[1], t[2.0], t[2^53|0], math.type(next({[3.0]=1})), #t, t[-0.0], rawget(t, 1.0)
-- case: nan and nil keys
local t = {}
return pcall(function() t[0/0] = 1 end), t[0/0], pcall(rawset, t, 0/0, 1), pcall(function() t[nil] = 1 end), t[nil], pcall(rawset, t, nil, 1)
-- case: metatable __index chain
local base = {a = 1}
local mid = setmetatable({b = 2}, {__index = base})
local top = setmetatable({}, {__index = mid})
return top.a, top.b, top.c, rawget(top, "a")
-- case: __index function receives original table
local mt = {}
mt.__index = function(t, k) return t == obj and k end
obj = setmetatable({}, mt)
return obj.x
-- case: __index on non-table via string metatable
return ("x").len == string.len, ("abc"):upper(), pcall(function() return (5).x end), pcall(function() local a; return a.b end), pcall(function() return ("x").y.z end)
-- case: __newindex chain to table
local store = {}
local t = setmetatable({}, {__newindex = store})
t.a = 1
local u = setmetatable({}, {__newindex = t})
u.b = 2
return rawget(t, "a"), store.a, store.b, rawget(u, "b")
-- case: __newindex not called for existing key
local log = {}
local t = setmetatable({x = 1}, {__newindex = function(t, k, v) log[#log + 1] = k end})
t.x = 2; t.y = 3; t.x = nil; t.x = 4
return #log, log[1], log[2], t.x, rawget(t, "y")
-- case: __call on __call table
local inner = setmetatable({}, {__call = function(self, x, y) return "inner", type(x), y end})
local outer = setmetatable({}, {__call = inner})
return pcall(outer, 1)
-- case: arithmetic metamethods all events
local mt = {}
for _, e in ipairs{"add","sub","mul","div","mod","pow","unm","idiv","band","bor","bxor","shl","shr","bnot","concat"} do mt["__" .. e] = function(a, b) return e end end
local x = setmetatable({}, mt)
return x + 1, 1 - x, x * x, x / 1, x % 1, x ^ 1, -x, x // 1, x & 1, 1 | x, x ~ 1, x << 1, x >> 1, ~x, x .. "a", "a" .. x, 1 .. x
-- case: metamethod picks first operand then second
local A = setmetatable({}, {__add = function() return "A" end})
local B = setmetatable({}, {__add = function() return "B" end})
return A + B, B + A, 1 + B, A + 1
-- case: bitwise on non-numeric string
return pcall(function() return "abc" & 1 end), pcall(function() return 1 | "1.5" end), pcall(function() return ~"x" end)
-- case: __eq only when both tables
local mt = {__eq = function() return true end}
local a, b = setmetatable({}, mt), setmetatable({}, {})
return a == b, b == a, a == {}, a == 1, a ~= b, rawequal(a, b), a == a
-- case: __eq not called for identical
local n = 0
local mt = {__eq = function() n = n + 1 return false end}
local a = setmetatable({}, mt)
return a == a, n
-- case: __eq result converted to boolean
local mt = {__eq = function() return 1 end, __lt = function() return nil end, __le = function() return "x" end}
local a, b = setmetatable({}, mt), setmetatable({}, mt)
return a == b, a < b, a <= b, a > b, a >= b, not (a == b), a ~= b
-- case: __le does not fall back to __lt in 5.4
local mt = {__lt = function() return true end}
local a, b = setmetatable({}, mt), setmetatable({}, mt)
return pcall(function() return a <= b end)
-- case: __lt with mixed operands
local mt = {__lt = function(x, y) return type(x) .. type(y) end}
local a = setmetatable({}, mt)
return a < 1, 1 < a, "s" < a, a > 2
-- case: __lt for strings and numbers via metatable
getmetatable("").__lt = function() return true end
local ok, r = pcall(function() return "a" < 1 end)
getmetatable("").__lt = nil
return ok, r
-- case: __concat with numbers and chains
local mt = {__concat = function(a, b) return (type(a) == "table" and "T" or tostring(a)) .. "+" .. (type(b) == "table" and "T" or tostring(b)) end}
local x = setmetatable({}, mt)
return "a" .. "b" .. x .. "c" .. "d", 1 .. x, x .. 2.0, x .. x
-- case: concat errors
return pcall(function() return "a" .. {} end), pcall(function() return nil .. "a" end), pcall(function() return 1 .. true end), pcall(function() local t = {} return "x" .. t.y end)
-- case: __len
local t = setmetatable({1, 2, 3}, {__len = function() return 42 end})
local u = setmetatable({}, {__len = function() return "s" end})
return #t, rawlen(t), #u, pcall(function() return #nil end)
-- case: __unm and __bnot second operand
local mt = {__unm = function(a, b) return rawequal(a, b) end, __bnot = function(a, b) return rawequal(a, b) end}
local x = setmetatable({}, mt)
return -x, ~x
-- case: __index long chain under limit
local t = {v = "end"}
for i = 1, 90 do t = setmetatable({}, {__index = t}) end
return t.v
-- case: __metatable field
local t = setmetatable({}, {__metatable = "locked"})
return getmetatable(t), pcall(setmetatable, t, {}), getmetatable("").__index == string, getmetatable("abc") == getmetatable("")
-- case: __tostring and __name
local t = setmetatable({}, {__tostring = function() return "custom" end})
local u = setmetatable({}, {__name = "MyType"})
local v = setmetatable({}, {__tostring = function() return 42 end})
return tostring(t), (tostring(u):gsub(": .*", ": ADDR")), pcall(tostring, v)
-- case: __name in error messages
local u = setmetatable({}, {__name = "MyType"})
return pcall(function() return u + 1 end), pcall(function() return #5 end), pcall(string.rep, u), pcall(function() return u < u end), pcall(function() return u() end)
-- case: __pairs
local t = setmetatable({}, {__pairs = function(t) return function(_, k) if not k then return 1, "a" end end, t, nil end})
local r = {}
for k, v in pairs(t) do r[#r + 1] = k .. v end
return table.concat(r)
-- case: __close order and error
local log = {}
local function mk(n) return setmetatable({}, {__close = function(o, e) log[#log + 1] = n .. ":" .. tostring(e) end}) end
do
  local a <close> = mk("a")
  local b <close> = mk("b")
  local c <close> = nil
  local d <close> = false
end
local ok, e = pcall(function()
  local x <close> = mk("x")
  local y <close> = setmetatable({}, {__close = function() error("in y", 0) end})
  error("orig", 0)
end)
return table.concat(log, " "), ok, e
-- case: close non-closable
return pcall(function() local x <close> = {} end), pcall(function() local x <close> = 1 end)
-- case: const folding usage
local a <const> = 10
local b <const> = a * 2
local s <const> = "x"
return a + b, s .. s, b // 3
-- case: close in loop with break and return
local log = {}
local function mk(n) return setmetatable({}, {__close = function() log[#log + 1] = n end}) end
for i = 1, 3 do
  local c <close> = mk(i)
  if i == 2 then break end
end
local function f() local z <close> = mk("z") return "ret" end
local r = f()
return table.concat(log, ","), r
-- case: close receives nil error normally
local got = "unset"
do local x <close> = setmetatable({}, {__close = function(o, e) got = select("#", o, e) .. tostring(e) end}) end
return got
-- case: close in coroutine.close
local log = {}
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function(o, e) log[#log + 1] = tostring(e) end})
  coroutine.yield(1)
end)
coroutine.resume(co)
return coroutine.close(co), coroutine.status(co), log[1]
-- case: close error replaces previous error
local ok, e = pcall(function()
  local a <close> = setmetatable({}, {__close = function(o, e) error("A:" .. tostring(e), 0) end})
  local b <close> = setmetatable({}, {__close = function(o, e) error("B:" .. tostring(e), 0) end})
  error("orig", 0)
end)
return ok, e
-- case: close with tail-ish return values preserved
local function f()
  local x <close> = setmetatable({}, {__close = function() end})
  return 1, 2, 3
end
return f()
-- case: generic for closes 4th value
local log = {}
local closing = setmetatable({}, {__close = function() log[#log + 1] = "closed" end})
for i in function(s, c) if c < 3 then return c + 1 end end, nil, 0, closing do
  if i == 2 then break end
end
return table.concat(log)
-- case: weak table basic
local t = setmetatable({}, {__mode = "k"})
t[{}] = 1
collectgarbage(); collectgarbage()
return next(t) == nil
