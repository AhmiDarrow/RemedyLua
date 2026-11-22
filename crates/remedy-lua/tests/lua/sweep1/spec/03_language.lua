-- case: long strings and comments
local a = [[
line1
line2]]
local b = [==[a]]b]=]c]==]
--[[ block
comment ]] local c = 1
--[==[ x ]] ]==] local d = 2
return a, b, c, d, #[[

x]]
-- case: long string newline normalization
return load("return [[\r\nab\n\rcd\r\n\r\nx]]")(), load("return 'a\\\r\nb'")(), load("return 'a\\\n\rb'")()
-- case: escapes
return "\a\b\f\n\r\t\v\\\"\'", "\65\066\0671", "\x41\x4a\x4A", "\u{41}\u{7FF}\u{FFFF}\u{10FFFF}", "a\z
      b", #"\u{7FFFFFFF}", "\u{0}" == "\0"
-- case: numerals
return 0x10, 0xA.8p1, 0x.1p4, 1e2, 1E-2, .5, 5., 3e0, 0xffffffffffffffff, 0x7fffffffffffffff, 9223372036854775807, 9223372036854775808, 0x1p-1074, 1e309, 0x10000000000000001
-- case: malformed numbers
return L("return 1e"), L("return 0x"), L("return 1..2"), L("return 3x"), L("return 0xg"), L("return 1.2.3"), L("return 08"), L("return 1e+"), L("return .e1")
-- case: number followed by dots
return 1 .. 2, L("return 1..2")
-- case: varargs basics
local function f(...) return select("#", ...), ... end
local function g(...) local a, b = ... return a, b end
local function h(...) return {...}, {..., "x"}, {"x", ...} end
local t1, t2, t3 = h(1, nil, 3)
return f(), f(nil, nil), g(1), #t2, t2[1], t2[2], t3[4], select("#", table.unpack(t3, 1, 4))
-- case: varargs in main chunk
return select("#", ...), ...
-- case: vararg outside vararg function
return L("function f() return ... end")
-- case: multiple results adjustment
local function m() return 1, 2, 3 end
local t = {m(), m()}
local u = {m(), (m())}
local a, b, c, d = m()
local x, y = (m())
return #t, #u, a, b, c, d, x, y, select("#", m(), m()), select("#", (m()))
-- case: multires in middle truncated
local function m() return 1, 2 end
return m(), m(), 10
-- case: table constructor trailing multires
local function m() return 1, 2, 3 end
local t = {m(); m(), [10] = "x"; n = 1,}
local u = {[1] = "a", "b"}
return #t, t[5], u[1], t.n, t[10]
-- case: table constructor with nil holes
local t = {1, nil, 3}
local u = {nil, nil, 3}
return t[3], u[3], select("#", table.unpack(t, 1, 3))
-- case: string call syntax
local function f(x) return type(x), x end
return f"str", f[[long]], f{1}
-- case: method call on string literal
return ("%d"):format(5), #("abc"):rep(3)
-- case: goto continue and backward
local out = {}
for i = 1, 5 do
  if i % 2 == 0 then goto cont end
  out[#out + 1] = i
  ::cont::
end
local n = 0
::top::
n = n + 1
if n < 3 then goto top end
return table.concat(out, ","), n
-- case: goto jumps into scope of label at block end ok
local ok = load("do goto f; local x; ::f:: end return 1")
return ok and ok()
-- case: label visible in nested block
local i = 0
do
  ::l::
  i = i + 1
  if i < 3 then goto l end
end
return i
-- case: repeat until sees inner locals
local i = 0
repeat local j = i; i = i + 1 until j >= 3
return i
-- case: closures capture per iteration
local fs = {}
for i = 1, 3 do fs[i] = function() return i end end
local ws = {}
local k = 1
while k <= 3 do local kk = k ws[k] = function() return kk end k = k + 1 end
return fs[1](), fs[3](), ws[1](), ws[3]()
-- case: integer for loop overflow
local n = 0
for i = math.maxinteger - 2, math.maxinteger do n = n + 1 end
local m = 0
for i = math.mininteger, math.mininteger + 2 do m = m + 1 end
local k = 0
for i = math.mininteger + 2, math.mininteger, -1 do k = k + 1 end
local z = 0
for i = 1, 0 do z = z + 1 end
return n, m, k, z
-- case: integer for loop large step
local c = 0
for i = 1, math.maxinteger, math.maxinteger // 2 do c = c + 1 end
local d = 0
for i = math.maxinteger, math.mininteger, math.mininteger do d = d + 1 end
local e = 0
for i = -10, 10, math.maxinteger do e = e + 1 end
return c, d, e
-- case: for loop zero step
return pcall(function() for i = 1, 10, 0 do end end), pcall(function() for i = 1.0, 10, 0.0 do end end)
-- case: for loop float limits with integer start
local r = {}
for i = 1, 3.5 do r[#r + 1] = i end
for i = 1, 2^53 * 4, 2^60 do r[#r + 1] = math.type(i) end
for i = 3, 0.5, -1 do r[#r + 1] = i end
local c = 0
for i = 1, -math.huge do c = c + 1 end
for i = 1, math.huge do c = c + 1 if c > 5 then break end end
return table.concat(r, ","), c
-- case: for loop float loops
local r = {}
for i = 0.1, 0.5, 0.1 do r[#r + 1] = string.format("%.17g", i) end
for i = 1.0, 3 do r[#r + 1] = math.type(i) end
for i = 1, 2, 0.5 do r[#r + 1] = i end
return table.concat(r, " ")
-- case: for loop integer start float limit nan
local c = 0
for i = 1, 0/0 do c = c + 1 end
for i = 1.0, 0/0 do c = c + 1 end
return c
-- case: for loop limit clipping with float
local c = 0
for i = math.maxinteger - 1, 1e100 do c = c + 1 end
local d = 0
for i = math.mininteger, -1e100, -1 do d = d + 1 end
return c, d
-- case: loop var modification does not affect loop
local r = {}
for i = 1, 3 do r[#r + 1] = i; i = i * 10 end
return table.concat(r, ",")
-- case: operator precedence
return 2 + 3 * 4 ^ 2 / 2, -2 ^ 2, not 1 == 2, 1 .. 2 .. 3, 2 ^ 3 ^ 2, 1 + 2 .. 3 + 4, 5 - 3 - 1, 1 << 2 + 1, 1 | 2 & 3, 6 ~ 3 & 1, 1 < 2 == true, "a" .. "b" == "ab", -3 // 2, - - 2, not not nil, 2 ^ -2, -x == nil
-- case: precedence bitwise vs comparison
return 1 | 2 == 3, 7 & 3 ~ 1, 1 << 1 << 1, 256 >> 1 >> 1, 3 .. 4 << 1
-- case: unary chains
return - - -1, ~ ~ 5, not not 0, #"abc" + 1, -2 ^ -2, ~1 + 1
-- case: short-circuit values
return nil and 1, false and 1, 0 and 1, nil or false, false or nil, 1 or error("x"), nil and error("x"), "" or 1, (nil or false) and 1
-- case: multiple assignment evaluation order
local t = {}
local i = 1
i, t[i] = i + 1, 20
local a, b, c = (function() return 1, 2 end)()
local x, y = 1
return i, t[1], t[2], a, b, c, x, y
-- case: swap
local a, b = 1, 2
a, b = b, a
local t = {1, 2}
t[1], t[2] = t[2], t[1]
return a, b, t[1], t[2]
-- case: local function recursion and local scoping
local function fact(n) if n <= 1 then return 1 end return n * fact(n - 1) end
local g = function(n) return n end
local x = 1
local x = x + 1
return fact(20), fact(21), g(3), x
-- case: method definitions
local obj = {v = 5}
function obj.get(self) return self.v end
function obj:inc(n) self.v = self.v + n return self end
local a = {b = {c = {}}}
function a.b.c:f() return self == a.b.c end
return obj:inc(2):get(), a.b.c:f()
-- case: tail calls deep
local function loop(n) if n == 0 then return "done" end return loop(n - 1) end
return loop(1000000)
-- case: deep recursion error
local function r(n) return 1 + r(n + 1) end
local ok, e = pcall(r, 1)
return ok, (string.gsub(e, "^.-:%d+: ", ""))
-- case: break inside repeat with upvalue
local fs = {}
repeat
  local x = 1
  fs[1] = function() return x end
  break
until false
return fs[1]()
-- case: semicolons and empty statements
;;; local a = 1;;; return a;
-- case: integer division of strings
return "7" // 2, math.type("7" // 2), "7.0" // 2, math.type("7" + 0), math.type("0x10" * 1), "10" / "2"
-- case: chained comparisons are errors in types
return pcall(load("return 1 < 2 < 3"))
-- case: long nested function upvalues
local function mk()
  local a = 0
  return function() local function inner() a = a + 1 return a end return inner() end
end
local f = mk()
f(); f()
return f()
-- case: upvalue shared between closures
local function pair()
  local v = 0
  return function() v = v + 1 return v end, function() return v end
end
local inc, get = pair()
inc(); inc()
return get()
-- case: numeric literal integer overflow becomes float
return math.type(9223372036854775807), math.type(9223372036854775808), 9223372036854775808, -9223372036854775808, math.type(-9223372036854775808), 0x8000000000000000
-- case: line continuation in strings and long brackets level mismatch
return "a\
b", [==[]=]]==]
-- case: identifiers and keywords
local _ = 1
local _G1, goto_ = 2, 3
return _, _G1, goto_, L("local goto = 1"), L("local and = 1")
-- case: non-ascii identifiers rejected
return L("local caf\xc3\xa9 = 1")
-- case: comparisons with string escapes in decimal
return "\9" == "\t", "\0659" == "A9", "\z   abc", "\x00" == "\0"
-- case: integer division float result sign
return 0.0 // 1, -0.0 // 1, 1 // -math.huge, -1 // math.huge, 0 // -1.0
-- case: modulo float sign of zero
return string.format("%g %g %g %g", 0.0 % -1, -0.0 % 1, 4.0 % 2, -4.0 % 2)
