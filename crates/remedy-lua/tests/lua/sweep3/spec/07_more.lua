-- case: weak tables
local wv = setmetatable({}, {__mode = "v"})
local wk = setmetatable({}, {__mode = "k"})
local keep = {}
wv[1] = {}; wv[2] = keep; wv[3] = "str" .. "ing"; wv[4] = 5
wk[{}] = 1; wk[keep] = 2; wk["s"] = 3
collectgarbage(); collectgarbage()
local n = 0
for k in pairs(wk) do n = n + 1 end
return wv[1], wv[2] == keep, wv[3], wv[4], n
-- case: index loop errors
local t = {}
setmetatable(t, {__index = t})
local u = {}
setmetatable(u, {__newindex = u})
return E(function() return t.x end), E(function() u.x = 1 end)
-- case: len non integer in table functions
local t = setmetatable({}, {__len = function() return "x" end})
local u = setmetatable({}, {__len = function() return 2.5 end})
return E(table.insert, t, 1), E(table.insert, u, 1), E(table.unpack, t), E(table.concat, u)
-- case: env not a table
local f = load("return x", "c", "t", 5)
local g = load("x = 1", "c", "t", nil)
return E(f), E(g)
-- case: vararg outside vararg function
return L("function f() return ... end"), L("local function f(a, ...) return function() return ... end end")
-- case: for loop extreme steps
local c = {}
local n = 0
for i = math.maxinteger - 1, math.maxinteger, 2 do n = n + 1 end
c[#c+1] = n; n = 0
for i = 0, math.maxinteger, math.maxinteger do n = n + 1 end
c[#c+1] = n; n = 0
for i = math.mininteger, math.maxinteger, math.maxinteger do n = n + 1 end
c[#c+1] = n; n = 0
for i = math.maxinteger, math.mininteger, math.mininteger do n = n + 1 end
c[#c+1] = n; n = 0
for i = -1, math.mininteger, math.mininteger do n = n + 1 end
c[#c+1] = n; n = 0
for i = 1, 3 do i = i * 10; n = n + i end
c[#c+1] = n; n = 0
for i = 0.1, 0.35, 0.1 do n = n + 1 end
c[#c+1] = n; n = 0
for i = 1, 2^53 + 10.0, 2^52 do n = n + 1 end
c[#c+1] = n
return table.concat(c, ",")
-- case: for loop limit float rounding to int
local r = {}
for _, lim in ipairs{2.5, -2.5, 0.5, -0.5, 2^63, -2^63, 1e300, -1e300} do
  local n = 0
  for i = 0, lim do n = n + 1 if n > 5 then break end end
  local m = 0
  for i = 0, lim, -1 do m = m + 1 if m > 5 then break end end
  r[#r+1] = n .. "/" .. m
end
return table.concat(r, " ")
-- case: math.random checks
math.randomseed(42)
local a = math.random(1, 10)
return a >= 1 and a <= 10, E(math.random, 3, 1), E(math.random, 1.5), E(math.random, 1, 2, 3), math.type(math.random(0)), math.random(5, 5), math.random(math.mininteger, math.mininteger), E(math.random, 0, -1), math.type(math.random())
-- case: math.randomseed returns
local a, b = math.randomseed(7)
local c, d = math.randomseed(7, 8)
return a, b, c, d, select("#", math.randomseed(1))
-- case: math.random sequence after seed
math.randomseed(123)
local r = {}
for i = 1, 5 do r[#r+1] = math.random(1000) end
r[#r+1] = math.random(math.mininteger, math.maxinteger)
r[#r+1] = string.format("%.17g", math.random())
return table.concat(r, ",")
-- case: math.type and min max
return math.type("1"), E(math.type), math.max(0.0, -0.0), math.min(-0.0, 0.0), math.max(1, 1.0), math.type(math.max(1, 1.0)), math.min(3), E(math.min), math.max(math.huge, 0/0) ~= math.max(math.huge, 0/0)
-- case: fmod integer edges
return math.fmod(-6, 3), math.fmod(6, -3), math.fmod(-1, math.mininteger), math.fmod(math.mininteger, math.mininteger), math.fmod(5, math.mininteger), math.fmod(5.5, 2), math.fmod(-5.5, 2), math.type(math.fmod(5, 2)), math.type(math.fmod(5, 2.0)), E(math.fmod, 5, "x")
-- case: string rep overflow
return E(string.rep, "ab", math.maxinteger), E(string.rep, "x", 2^31, "y") ~= nil, string.rep("", 5, ""), string.rep("x", 3, "")
-- case: tostring of name metafield
local t = setmetatable({}, {__name = 5})
local u = setmetatable({}, {__name = "N"})
return tostring(t):match("^table: ") ~= nil, tostring(u):match("^N: ") ~= nil, E(function() return #u end), E(function() return u < u end), E(function() return u.x.y end)
-- case: pack s size overflow and z unfinished
return E(string.pack, "s1", ("x"):rep(256)), E(string.unpack, "z", "abc"), E(string.unpack, "s1", "\5ab"), string.unpack("=I4", "\1\0\0\0"), string.unpack("<!2 i1 i3", "\1\0\2\0\0"), E(string.pack, "!3 i4", 1), E(string.pack, "!17", 1), string.packsize("!16 i1 i16"), string.unpack("j", string.pack("j", math.mininteger)), string.unpack("n", string.pack("n", 1.5)), string.unpack("T", string.pack("T", 7))
-- case: pack unpack multiple and positions
local s = string.pack("i2 z s1 b", 7, "hi", "yo", -3)
return #s, string.unpack("i2 z s1 b", s), string.unpack("i2", s, 1), select("#", string.unpack("", "")), string.unpack("", "", 1), E(string.unpack, "", "", 2), string.unpack("", "abc", 4), E(string.unpack, "i1", "abc", -10)
-- case: pack float types
return string.unpack("f", string.pack("f", 0.1)), string.unpack("d", string.pack("d", 0.1)), string.unpack(">d", string.pack(">d", -2.5)), string.pack("f", 1/0):byte(1, -1), E(string.pack, "f", "x"), string.unpack("f", string.pack("f", "2.5")), string.pack("i", 3.0):len()
-- case: utf8 char codepoint ranges
return utf8.char(), utf8.char(65.0), E(utf8.char, 65.5), E(utf8.char, -1), select("#", utf8.codepoint("abc", 3, 2)), utf8.codepoint("abc", -1), E(utf8.codepoint, "abc", 0), E(utf8.codepoint, "abc", 1, 4), utf8.codepoint("\u{10FFFF}"), E(utf8.codepoint, "\xF4\x90\x80\x80"), utf8.codepoint("\xF4\x90\x80\x80", 1, 1, true)
-- case: utf8 len invalid position returns nil and pos
return utf8.len("a\xFFb"), utf8.len("ab\xC3"), utf8.len("\xE4\xBD"), utf8.len("abc", 2, 1), utf8.len("abc", 4, 3), E(utf8.len, "abc", 1, 4), utf8.len("\u{7FFFFFFF}"), utf8.len("\u{7FFFFFFF}", 1, -1, true)
-- case: concat number formats
local t = {}
for _, v in ipairs{1, -1, 1.0, -0.0, 0.1, 1e15, 1e16, 123.456, 2^63, math.mininteger, 1/0, -1/0} do t[#t+1] = v .. "" end
return table.concat(t, " ")
-- case: format float rounding exactness
local r = {}
for _, v in ipairs{0.5, 1.5, 2.5, 2.675, 1.005, 0.125, 1e-7, 123456789.125, 5e-324, 1.7976931348623157e308} do
  r[#r+1] = string.format("%.0f %.2f %.20f %.3e %.17g %g", v, v, v, v, v, v)
end
return table.concat(r, " | ")
-- case: format big precision
return string.format("%.99f", 1/3), string.format("%.99f", 1e300):sub(1, 40), #string.format("%99.99f", 1e308), string.format("%.50e", 0.1), string.format("%.40g", 2^-1074)
-- case: tostring 14 digits
local r = {}
for _, v in ipairs{0.1 + 0.2, 1/3, 2/3, 100/3, 1e14 + 0.5, 123456789012.345, 3.14159265358979, 1e-310, 2^-1074, 1e21, 12345678901234567890} do r[#r+1] = tostring(v) end
return table.concat(r, " ")
-- case: numeral syntax edges
return L("return 0x.p1"), L("return 1e+"), L("return 0x1P+4"), L("return 3.e2"), L("return 0x"), L("return 1..2"), L("return 0xep1"), L("return 08"), L("return 1_000"), L("return 0b101"), 0x1P+4, 3.e2, 08, 0xA, 0Xa, 1E2, 0x.1, 0x1p-1074, 0x1p1024, 1e400, 0xffffffffffffffffff
-- case: negative literal folding
return -9223372036854775808, math.type(-9223372036854775808), -9223372036854775807 - 1, math.type(-9223372036854775807 - 1), - 0x8000000000000000
-- case: goto end of block with local
do goto l; local x = 1; ::l:: end
do goto m; local y; ::m:: ; ; end
local ok = L("do goto l; local x = 1; ::l:: print(x) end")
local ok2 = L("while true do goto c; local z; ::c:: end")
return ok, ok2
-- case: goto backward closes upvalues
local fs = {}
local i = 1
::top::
local x = i
fs[i] = function() return x end
i = i + 1
if i <= 3 then goto top end
return fs[1](), fs[2](), fs[3]()
-- case: integer string keys distinct
local t = {}
t["1"] = "s"; t[1] = "i"
return t["1"], t[1], t[1.0]
-- case: string index metatable change
local mt = getmetatable("")
local old = mt.__index
mt.__index = function(s, k) return "custom:" .. k end
local r = ("x").foo
mt.__index = old
return r, ("x"):upper()
-- case: string arith metamethods
local mt = getmetatable("")
return type(mt.__add), type(mt.__unm), type(mt.__idiv), type(mt.__band), E(function() return "a" + 1 end), E(function() return "1" + {} end), E(function() return {} + "1" end), "3" // 2, "3.0" // 2, "2" & 3, E(function() return "2.5" & 3 end)
-- case: string arithmetic float strings
return "0x10" * 1.0, "1e1" // 1, -"-0", math.type(-"-0"), "10" % "3", "10" % "3.0", " 0x1p4 " + 0
-- case: tostring print __name and tostring of functions
return type(tostring(print)), tostring(print):sub(1, 9), tostring(function() end):sub(1, 9), tostring({}):sub(1, 6), tostring(coroutine.create(print)):sub(1, 7)
-- case: multiple returns through pcall and select
local function many() return 1, 2, 3, nil, nil end
return select("#", many()), select("#", pcall(many)), select("#", (many())), select("#", table.unpack({many()}))
-- case: deep nesting of parens and calls
local f = function(...) return ... end
return f(f(f(f(1, 2), 3), 4)), ((f(1, 2)))
-- case: integer vs float table keys with 2^63
local t = {}
t[2^63] = "f"; t[math.mininteger] = "i"; t[-2^63] = "neg"
return t[2^63], t[math.mininteger], math.type(next({[-2^63] = 1}))
-- case: table length with border
local t = {1, 2, 3}
t[#t + 1] = 4
t[#t] = nil
return #t, #{nil}, #{1, nil}, #{n = 1}, #"", #{[1] = 1, [2] = 2}
-- case: coroutine isyieldable with arg and running in wrap
local co = coroutine.create(function() return coroutine.isyieldable() end)
local main = coroutine.running()
return coroutine.isyieldable(co), coroutine.isyieldable(main), coroutine.resume(co), coroutine.wrap(function() local c, m = coroutine.running() return type(c), m end)()
-- case: coroutine close with pending close error
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function() error("cl", 0) end})
  coroutine.yield()
end)
coroutine.resume(co)
local a, b = coroutine.close(co)
return a, b, coroutine.status(co), coroutine.close(co)
-- case: coroutine error object and traceback independence
local co = coroutine.create(function() local x = nil; return x.y end)
local ok, e = coroutine.resume(co)
local co2 = coroutine.create(function() error({code = 1}) end)
local ok2, e2 = coroutine.resume(co2)
return ok, e, ok2, type(e2), e2.code
-- case: coroutine wrap passes values both ways
local w = coroutine.wrap(function(a) local b = coroutine.yield(a * 2) local c, d = coroutine.yield(b + 1) return c .. d end)
return w(5), w(10), w("x", "y")
-- case: coroutine status of dead after error and resume dead
local co = coroutine.create(function() return 1 end)
coroutine.resume(co)
return coroutine.status(co), coroutine.resume(co), coroutine.resume(co)
-- case: yield across sort comparator
local co = coroutine.create(function() table.sort({3, 2, 1}, function(a, b) coroutine.yield() return a < b end) end)
return coroutine.resume(co)
-- case: yield across string.gsub callback
local co = coroutine.create(function() return (("abc"):gsub(".", function(c) coroutine.yield(c) return c end)) end)
return coroutine.resume(co)
-- case: yield across metamethod and tostring
local co = coroutine.create(function() return tostring(setmetatable({}, {__tostring = function() coroutine.yield("in") return "x" end})) end)
local a, b = coroutine.resume(co)
local c, d = coroutine.resume(co)
return a, b, c, d
-- case: yield inside load reader
local co = coroutine.create(function() return load(function() coroutine.yield() return nil end) end)
return coroutine.resume(co)
-- case: os.date formats
local t = 1700000000
return os.date("!%Y-%m-%dT%H:%M:%S", t), os.date("!%d/%m/%y %I %p %M %S %w %j %U %W %%", t), os.date("!%a %A %b %B", t), E(os.date, "%E")
-- case: os.date *t fields and os.time roundtrip
local t = os.date("!*t", 1700000000)
local keys = {}
for k in pairs(t) do keys[#keys+1] = k end
table.sort(keys)
return table.concat(keys, ","), t.year, t.month, t.day, t.hour, t.min, t.sec, t.wday, t.yday, t.isdst
-- case: os.time field errors
return E(os.time, {year = 2020, month = 1}), E(os.time, {year = 2020, month = 1, day = 1.5}), E(os.time, {year = "2020", month = 1, day = 1}) ~= nil, E(os.time, {year = 1e20, month = 1, day = 1}), E(os.time, 5)
-- case: os.difftime and clock
return os.difftime(10, 5), E(os.difftime, 10), E(os.difftime), E(os.difftime, 1.5, 1), math.type(os.difftime(1, 1)), math.type(os.time()), math.type(os.clock())
-- case: os.time field checks
return E(os.time, {year = 2020, month = 1, day = {}}), E(os.time, {year = 2020, month = true, day = 1}), E(os.time, {year = 2020, month = 1, day = 1, hour = print}), E(os.time, {year = -2^31 + 1899, month = 1, day = 1}), E(os.time, {year = 2^31 + 1900, month = 1, day = 1}), E(os.time, {year = 2020, month = -2^31, day = 1}), E(os.time, {year = 2020, month = 1, day = 2^31}), E(os.time, {year = 2020, month = 1, day = 1, min = -2^31 - 1})
-- case: table functions on huge lengths
local big = setmetatable({}, {__len = function() return math.maxinteger end, __newindex = function(t, k, v) rawset(t, "k", k) end})
table.insert(big, 1)
return E(table.sort, big), rawget(big, "k"), E(table.insert, big, 1, 1)
-- case: math trig and conversions
local r = {}
for _, v in ipairs{0, -0.0, 0.5, 1, -1, 2, 1e10, math.pi} do
  r[#r+1] = string.format("%.15g %.15g %.15g %.15g %.15g %.15g %.15g", math.sin(v), math.cos(v), math.atan(v), math.atan(v, -1), math.deg(v), math.rad(v), math.exp(v))
end
r[#r+1] = string.format("%.15g %.15g %.15g %.15g %.15g %.15g", math.atan(math.huge), math.exp(-math.huge), math.asin(1), math.acos(-1), math.atan(0.0, -0.0), math.atan(-0.0, -1))
r[#r+1] = tostring(math.log(0)) .. tostring(math.log(-0.0)) .. tostring(math.log(1024, 2)) .. tostring(math.log(1000, 10)) .. tostring(math.log(27, 3)) .. tostring(math.log(8, 2.0)) .. tostring(math.sqrt(-0.0))
r[#r+1] = math.type(math.deg(1)) .. math.type(math.sqrt(4)) .. math.type(math.exp(0))
return table.concat(r, " | ")
