-- case: many upvalues limit
local src = {}
for i = 1, 256 do src[#src + 1] = "local a" .. i .. " = " .. i end
src[#src + 1] = "return function() return "
for i = 1, 256 do src[#src + 1] = "a" .. i .. (i < 256 and " + " or "") end
src[#src + 1] = " end"
local s = table.concat(src, "\n")
return L(s)
-- case: many arguments and results
local function f(...) return select("#", ...) end
local t = {}
for i = 1, 250 do t[i] = i end
return f(table.unpack(t)), select("#", table.unpack(t)), (select(250, table.unpack(t)))
-- case: long concat chain
local s = "return " .. string.rep("'a' .. ", 300) .. "'b'"
local f, e = load(s)
return f and #f() or e
-- case: large table constructor
local parts = {}
for i = 1, 600 do parts[i] = tostring(i) end
local f = load("return {" .. table.concat(parts, ",") .. "}")
local t = f()
return #t, t[600], t[300]
-- case: string index with numbers
return ("abc")[1], ("abc").x, pcall(function() return ("abc").len.x end)
-- case: length border
local t = {1, 2, 3}
t[#t + 1] = 4
t[#t] = nil
local u = {n = 1}
return #t, #u, #{nil}, #{1, 2, nil}, #""
-- case: next after clearing during traversal
local t = {a = 1, b = 2, c = 3, d = 4}
local n = 0
for k in pairs(t) do t[k] = nil n = n + 1 end
return n, next(t)
-- case: integer float key equivalence in constructor
local t = {[1] = "a", [1.0] = "b", [2^53] = "c"}
return t[1], t[2^53 | 0]
-- case: float keys that are not integral
local t = {}
t[1.5] = "x"; t[-0.0] = "z"
return t[1.5], t[0], math.type(next({[-0.0] = 1}))
-- case: string.format g precision variety
local r = {}
for _, v in ipairs{0.1, 1/3, 123456789.123, 1e-7, 1e21, 2^63, -2^-1074, 100, 1e16} do
  r[#r + 1] = string.format("%g|%.1g|%.10g|%.17g|%#.3g|%e|%.0e|%12.4e", v, v, v, v, v, v, v, v)
end
return table.concat(r, "\n")
-- case: string.format f large
return string.format("%.0f", 2^63), string.format("%f", 1e20), string.format("%.3f", -0.0005), string.format("%.0f", 0.5), string.format("%.0f", 1.5), string.format("%.0f", -2.5), #string.format("%f", 1e308)
-- case: string.format d with width precision
return string.format("[%8.3d][%-8.3d][%08d][%+08d][% 8d][%x][%#o][%5.0d]", 42, 42, -42, 42, 42, 0, 0, 0)
-- case: format q round trip
local vals = {0.1, -0.0, 1e300, 2^63, math.mininteger, math.maxinteger, 1/3, "\0\1\2abc\255\n\r\t\\\""}
local out = {}
for i, v in ipairs(vals) do
  local s = string.format("%q", v)
  local back = load("return " .. s)()
  out[#out + 1] = tostring(back == v or (v ~= v and back ~= back))
end
return table.concat(out, ",")
-- case: tostring float formatting 14 digits
return tostring(3.14159265358979), tostring(1/7), tostring(1e-5), tostring(123456789012345.6), tostring(2^53), tostring(-2^63), tostring(1.0e15), tostring(1e+16)
-- case: concat number formatting
return 1 .. "", 1.0 .. "", -0.0 .. "", 1e16 .. "", 2^63 .. "", 0.1 .. "", math.pi .. ""
-- case: string.find init beyond length
return ("abc"):find("", 4), ("abc"):find("", 5), ("abc"):find("x", 5), ("abc"):match("", 4), ("abc"):match("", 5), ("abc"):find("c$", 3)
-- case: gsub pattern special chars in replacement
return ("hello"):gsub("l", "%%1"), ("a.b"):gsub("%.", "%%"), ("x"):gsub("x", "a%0b%0c")
-- case: gsub with pattern and position capture replacement
return ("abc"):gsub("()", "%1"), ("abc"):gsub("b()", "%1")
-- case: string.byte negative and large ranges
return ("hello"):byte(-3, -1), ("hello"):byte(2, -10), ("hello"):byte(math.mininteger, 2), ("hello"):byte(4, math.maxinteger)
-- case: string.rep count coercion
return ("ab"):rep("2"), ("ab"):rep(2.0), pcall(string.rep, "ab", "2.5"), pcall(string.rep, "ab", {})
-- case: math.floor ceil strings and huge
return math.floor("3.7"), math.ceil("-3.2"), math.floor(-2^63), math.type(math.floor(-2^63)), math.floor(2^63), math.type(math.floor(2^63)), math.ceil(2^62 + 0.0), math.floor(-0.5)
-- case: math max min types
return math.type(math.max(1, 2)), math.type(math.max(1.0, 2)), math.type(math.max(2, 1.0)), math.max(math.mininteger, -math.huge), math.min(math.maxinteger, math.huge), math.max(-0.0, 0.0), math.max(0.0, -0.0)
-- case: integer conversions in arithmetic with strings
return "10" + 0, math.type("10" + 0), math.type("10.0" + 0), math.type("1e1" + 0), "0x10" + 0, math.type("9223372036854775808" + 0), "  0x1p4  " + 0, pcall(function() return "1 2" + 0 end)
-- case: comparison int float boundary precision
local big = 2^53
return (2^53 | 0) + 1 == big + 1, (2^53 | 0) + 1 < big + 2, math.maxinteger + 0.0 == math.maxinteger, math.maxinteger - 1 < math.maxinteger + 0.0, -(2^63) == math.mininteger, math.mininteger + 0.0 == math.mininteger
-- case: select negative beyond
return pcall(select, -2, "a"), select(-1, "a"), select(math.maxinteger, 1)
-- case: pcall returns and error objects
return select("#", pcall(function() return end)), pcall(function() return nil, nil end), pcall(error, nil)
-- case: error with level 2 position
local function lib() error("bad thing", 2) end
local function user() lib() end
return pcall(user)
-- case: runtime error messages variable names
local t = {}
local up = nil
local function f() return up.x end
return pcall(function() return t.a.b end), pcall(function() local x; x() end), pcall(f), pcall(function() return undefinedglobal.x end), pcall(function() undefinedfn() end), pcall(function() t:nomethod() end), pcall(function() return #t.z end), pcall(function() return t.q + 1 end), pcall(function() return -{} end)
-- case: runtime error messages arithmetic
local s = "x"
return pcall(function() return s + 1 end), pcall(function() return {} .. "" end), pcall(function() return 1 & 1.5 end), pcall(function() return "a" | 1 end), pcall(function() return 2^63 // 1 | 0 end), pcall(function() return math.huge // 1 | 0 end)
-- case: comparison error messages
return pcall(function() return 1 < "x" end), pcall(function() return {} < 1 end), pcall(function() return "a" < {} end), pcall(function() return nil > 1 end), pcall(function() return {} >= {} end)
-- case: call error messages
local t = {}
return pcall(function() t.x.y() end), pcall(function() t.f() end), pcall(function() local a = 5; a() end), pcall(function() ("x")() end), pcall(function() (nil)() end)
-- case: string method error messages
return pcall(string.rep), pcall(("x").rep), pcall(string.sub), pcall(string.char, {}), pcall(string.format, "%d", {}), pcall(("x").bad), pcall(table.concat, {}, {}), pcall(math.floor, {}), pcall(setmetatable, 1)
-- case: arg errors in methods count self
local s = "abc"
return pcall(s.rep, s, {}), pcall(function() return s:rep({}) end), pcall(function() return s:sub({}) end)
-- case: integer for with float step accumulates
local r = {}
for i = 1, 2, 0.25 do r[#r + 1] = i end
for i = 0, -1, -0.5 do r[#r + 1] = i end
return table.concat(r, ",")
-- case: while and repeat with closures and break
local fs = {}
local i = 0
repeat
  i = i + 1
  local j = i
  fs[i] = function() return j end
  if i == 3 then break end
until false
return fs[1](), fs[2](), fs[3]()
-- case: goto continue in repeat legal when no local
local i = 0
repeat
  i = i + 1
  if i < 3 then goto cont end
  ::cont::
until i >= 5
return i
-- case: vararg in table and select adjust
local function f(...)
  local a = {...}
  local b = {..., n = select("#", ...)}
  return #a, b.n, (...)
end
return f(1, 2, 3)
-- case: method call with varargs
local o = {}
function o:m(...) return self == o, select("#", ...) end
return o:m(1, nil, nil)
-- case: string comparisons with embedded zeros and high bytes
local r = {}
local strs = {"", "\0", "\0\0", "a", "a\0", "a\0\0", "\255", "\127", "ab", "b"}
for i = 1, #strs do for j = 1, #strs do r[#r + 1] = (strs[i] < strs[j]) and "1" or "0" end end
return table.concat(r)
-- case: tostring of integer-valued floats in keys
local t = {}
t[2.0] = 1
for k in pairs(t) do return math.type(k), k end
-- case: integer division by negative one and mininteger
return math.mininteger // -1, math.mininteger % -1, math.mininteger * -1, math.mininteger / -1, -1 // math.mininteger, 1 % math.mininteger, -1 % math.mininteger
-- case: float modulo precision
return 5.3 % 2, -5.3 % 2, 5.3 % -2, 1e308 % 3, 3 % 1e308, -3 % 1e308, 0.0 % -2
-- case: exponent edge
return 2^1024, (-2)^0.5 ~= (-2)^0.5, 0^0, (-0.0)^-1, 1^math.huge, (-1)^math.huge, math.huge^0
-- case: bitwise with floats from strings
return "0x10" & 0xff, "1e2" | 0, pcall(function() return "1e100" | 0 end), "-0" | 0, "0.0" | 0
-- case: string.len of numbers float formatting
return string.len(1.0), string.len(-0.0), string.len(2^63), string.len(1e100)
-- case: tonumber with whitespace and base edge
return tonumber("\n12\t"), tonumber("12", 10), tonumber(" -7 ", 8), tonumber("0x10", 16), tonumber("10", 16.0), tonumber("1e1", 16), tonumber("1E1", 16)
-- case: utf8.char multi and codes lax
local r = {}
for p, c in utf8.codes("\u{7FFFFFFF}", true) do r[#r + 1] = c end
return utf8.char(65, 66), r[1], pcall(utf8.codes, 5), utf8.len("\u{7FFFFFFF}"), utf8.len("\u{7FFFFFFF}", 1, -1, true)
-- case: utf8.offset continuation errors
return pcall(utf8.offset, "\x80", 1), utf8.offset("a\x80b", -1), utf8.offset("abc", 0, 4), pcall(utf8.offset, "abc", 1, 5), utf8.offset("abc", 1, 4), utf8.offset("abc", 2, 4)
-- case: table.insert float position
local t = {1, 2, 3}
table.insert(t, 2.0, "x")
return table.concat(t, ","), pcall(table.insert, t, "2", "y")
-- case: table.remove border shrink
local t = {1, 2, 3}
t[5] = 5
return #t == 3 or #t == 5, table.remove(t, #t + 1)
-- case: table.concat sep numeric
return table.concat({1, 2, 3}, 0), table.concat({"a", "b"}, 1.5)
-- case: table.sort with nan values
local t = {3, 0/0, 1, 2}
local ok = pcall(table.sort, t)
return ok
-- case: table.sort comparator errors propagate
return pcall(table.sort, {3, 2, 1}, function(a, b) error("cmp", 0) end)
-- case: table.sort many equal elements
local t = {}
for i = 1, 500 do t[i] = i % 3 end
table.sort(t)
return t[1], t[166], t[167], t[500]
-- case: table.sort with 1 and 0 elements and sort of strings with numbers mixed error
local a, b = {}, {1}
table.sort(a); table.sort(b)
return #a, b[1], pcall(table.sort, {"a", 1})
-- case: coroutine.wrap inside wrap and status
local log = {}
local outer = coroutine.wrap(function()
  local inner = coroutine.wrap(function() coroutine.yield("i1") return "i2" end)
  log[#log + 1] = inner()
  coroutine.yield("o1")
  log[#log + 1] = inner()
  return "o2"
end)
return outer(), outer(), table.concat(log, ",")
-- case: coroutine resume passing many values
local co = coroutine.create(function(...) return select("#", ...), ... end)
return coroutine.resume(co, 1, nil, 3, nil)
-- case: coroutine error after yield status
local co = coroutine.create(function() coroutine.yield() error("x") end)
coroutine.resume(co)
local ok, e = coroutine.resume(co)
return ok, e, coroutine.status(co), coroutine.resume(co)
-- case: coroutine wrapping table with __call
local callable = setmetatable({}, {__call = function(_, x) return x * 2 end})
return pcall(coroutine.create, callable), pcall(coroutine.wrap, callable)
-- case: running inside wrap
local co = coroutine.wrap(function() local c, m = coroutine.running() return type(c), m end)
return co()
-- case: isyieldable inside pcall in coroutine
local co = coroutine.wrap(function() return pcall(coroutine.isyieldable) end)
return co()
-- case: yield inside metamethod __index chain via pcall
local co = coroutine.wrap(function()
  return pcall(function()
    local t = setmetatable({}, {__index = function(_, k) return coroutine.yield(k) end})
    return t.key
  end)
end)
return co(), co("val")
-- case: close upvalues on error in loop
local fs = {}
for i = 1, 3 do
  pcall(function() local j = i fs[i] = function() return j end error("x") end)
end
return fs[1](), fs[2](), fs[3]()
-- case: string.format %s with precision and __tostring
local o = setmetatable({}, {__tostring = function() return "abcdef" end})
return string.format("%.3s|%-8s|%8.2s", o, o, o)
-- case: string.format %c values
return pcall(string.format, "%c", 256), pcall(string.format, "%c", -1), string.format("%c", 255) == "\255"
-- case: string.format %x negative and float
return string.format("%x", -1), string.format("%X", 255.0), pcall(string.format, "%x", 1.5), string.format("%o", -1)
-- case: string.format %i alias
return string.format("%i|%5i|%-5i|", 3, 4, 5)
-- case: tostring of very small and large
return tostring(1e-300), tostring(1.7976931348623157e308), tostring(4.9e-324), tostring(-1e-7), tostring(123e-20)
-- case: math.abs of strings and integer floats
return math.abs("-3"), math.type(math.abs("-3")), math.abs(-2^63), math.type(math.abs(-2^63))
-- case: math.sqrt and exp types
return math.type(math.sqrt(4)), math.sqrt(4), math.exp(1), math.log(2^10, 2), math.log(10, 10), math.log(1000, 10), math.log(0.5, 0.5), math.log(8, 4), math.log(-1) ~= math.log(-1)
-- case: math.fmod negative zero and large
return math.fmod(-6, 3), math.fmod(6, -3), math.fmod(-6.0, 3), math.fmod(1e308, 1e-308) >= 0, math.fmod(-0.0, 1)
