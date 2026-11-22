-- case: varargs
local function f(...) return select("#", ...), ... end
local function g(...) local a, b = ... return a, b end
local function h(...) return {...}, {..., "x"} end
local t1, t2 = h(1, 2, 3)
return f(), f(nil), f(nil, nil), g(1), #t1, #t2, (f(1, 2))
-- case: vararg in main chunk and table.pack
local f = load("return select('#', ...), ...")
return f(1, nil, 3)
-- case: goto semantics
local r = {}
for i = 1, 3 do
  for j = 1, 3 do
    if j == 2 then goto continue end
    r[#r+1] = i .. j
    ::continue::
  end
end
do goto skip; r[#r+1] = "no"; ::skip:: end
return table.concat(r, ",")
-- case: goto errors
return L("goto nowhere"), L("do ::a:: end goto a"), L("::a:: ::a::"), L("goto f; local x; ::f:: print(x)"), L("do goto f; local x; ::f:: end"), L("::a:: do ::a:: end"), L("repeat goto cont; local x ::cont:: until x")
-- case: goto continue in repeat
local i, r = 0, {}
repeat
  i = i + 1
  if i % 2 == 0 then goto cont end
  r[#r+1] = i
  ::cont::
until i >= 5
return table.concat(r, ",")
-- case: closures in loops are fresh
local fs = {}
for i = 1, 3 do fs[i] = function() return i end end
local ws = {}
local j = 0
while j < 3 do j = j + 1; local k = j; ws[j] = function() return k end end
return fs[1](), fs[3](), ws[1](), ws[3]()
-- case: repeat scope
local x = 0
repeat local y = x; x = x + 1 until y >= 2
return x
-- case: multiple assignment order
local a, b, c = 1
local t = {}
local i = 1
i, t[i] = i + 1, 20
local x, y = 1, 2
x, y = y, x
return a, b, c, i, t[1], t[2], x, y
-- case: string escapes
return "\65\066\x43\u{44}\z
       E", "a\
b", "\'\"", #"\0\00\000", "\u{7FF}\u{FFFF}"
-- case: escape errors
return L([[return "\x4"]]), L([[return "\400"]]), L([[return "\u{110000000}"]]), L([[return "\q"]]), L([[return "abc]]), L("return [==[ x ]=]"), L([[return "\u{}"]]), L([[return "\u65"]])
-- case: syntax errors
return L("x ="), L("local function"), L("return return"), L("f() = 1"), L("x, y() = 1, 2"), L("(a) = 1"), L("a.b:c = 1"), L("break"), L("local x <const>"), L("for i = 1 do end"), L("if x then"), L("x = }"), L("x = 1 1"), L("return 1;;")
-- case: syntax lexical
return L("x = 0x"), L("x = 1e"), L("x = 3..2"), L("x = 1.2.3"), L("x = @"), L("x = 'abc\ndef'"), L("--[[ unfinished"), L("x = [[ unfinished"), L("x = ~= 1"), L("x = 0xg")
-- case: long brackets and comments
local s = [==[
a]]b]=]c]==]
--[==[ comment ]] still ]==] local z = 1
return s, z, [[
]], #[[

x]]
-- case: operator precedence
return 2 + 3 * 4 ^ 2 / 2, not 1 == 2, -2 ^ 2, 1 .. 2 .. 3, "a" .. 1 + 2, 1 < 2 == true, 5 & 3 | 8 ~ 1, 1 << 2 + 1, #"ab" + 1, -3 // 2, 2 ^ -1 ^ 2, 1 or error("x"), nil and error("x")
-- case: and or values
return nil or false, false or nil, 0 and 1, "" and "x", nil and 1, false == nil, 1 and nil
-- case: integer division constant folding errors
return L("return 1 // 0"), L("return 1 % 0"), E(load("return 1 // 0")), E(load("local a = 0 return 1 // a"))
-- case: method call syntax and self
local obj = {v = 5}
function obj:get(a) return self.v + (a or 0) end
function obj.static(a) return a end
return obj:get(), obj:get(2), obj.get(obj, 3), obj.static(7), ("%d"):format(4)
-- case: coroutine basics
local co = coroutine.create(function(a, b)
  local c = coroutine.yield(a + b)
  local d, e = coroutine.yield(c * 2)
  return d + e
end)
local r = {}
r[#r+1] = select(2, coroutine.resume(co, 1, 2))
r[#r+1] = select(2, coroutine.resume(co, 10))
r[#r+1] = select(2, coroutine.resume(co, 3, 4))
r[#r+1] = tostring(coroutine.resume(co))
r[#r+1] = select(2, coroutine.resume(co))
r[#r+1] = coroutine.status(co)
return table.concat(r, ",")
-- case: coroutine status and running
local main, ismain = coroutine.running()
local co
co = coroutine.create(function()
  local c, m = coroutine.running()
  return coroutine.status(co), c == co, m, coroutine.isyieldable()
end)
return type(main), ismain, coroutine.isyieldable(), coroutine.resume(co)
-- case: coroutine normal status
local co1, co2
co1 = coroutine.create(function() coroutine.resume(co2) end)
co2 = coroutine.create(function() return coroutine.status(co1) end)
local st
co2 = coroutine.create(function() st = coroutine.status(co1) end)
coroutine.resume(co1)
return st
-- case: coroutine wrap
local gen = coroutine.wrap(function() for i = 1, 3 do coroutine.yield(i) end end)
local a, b, c, d = gen(), gen(), gen(), gen()
return a, b, c, d, E(gen), E(coroutine.wrap(function() error("inner") end)), E(coroutine.wrap(function() error({}) end)) ~= nil, E(coroutine.wrap(function() error("x", 0) end))
-- case: coroutine resume errors
local co = coroutine.create(function() error("bad") end)
local ok, e = coroutine.resume(co)
return ok, e, coroutine.status(co), coroutine.resume(co), coroutine.resume(coroutine.running()), E(coroutine.resume, 1), E(coroutine.yield, 1), E(coroutine.status, nil)
-- case: coroutine close
local co = coroutine.create(function() coroutine.yield() end)
coroutine.resume(co)
local r1 = {coroutine.close(co)}
local r2 = {coroutine.close(coroutine.create(print))}
local dead = coroutine.create(function() error("e") end)
coroutine.resume(dead)
local r3 = {coroutine.close(dead)}
return r1[1], #r1, r2[1], r3[1], r3[2], E(coroutine.close, coroutine.running()), coroutine.status(co)
-- case: coroutine close running from inside
local co
co = coroutine.create(function() return E(coroutine.close, co) end)
return coroutine.resume(co)
-- case: coroutine yield across pcall and metamethods
local co = coroutine.wrap(function()
  local ok, v = pcall(function() return coroutine.yield(1) + 1 end)
  local t = setmetatable({}, {__index = function(t, k) return coroutine.yield(k) end})
  local w = t.foo
  return ok, v, w
end)
return co(), co(10), co("W")
-- case: coroutine wrap dead
local f = coroutine.wrap(function() return 1 end)
f()
return E(f)
-- case: coroutine resume self / non-suspended
local co
co = coroutine.create(function() return coroutine.resume(co) end)
return coroutine.resume(co)
-- case: yield from main
return E(coroutine.yield)
-- case: os functions
return type(os.time()), type(os.clock()), os.time({year = 2020, month = 1, day = 1, hour = 12}) - os.time({year = 2020, month = 1, day = 1, hour = 11}), os.date("!%Y-%m-%d %H:%M:%S", 0), os.date("!*t", 3600).hour, os.date("!%c", 0), os.date("!%x %X %p %j %a %A %b %B %y", 86400 * 40), E(os.date, "%Q"), type(os.difftime(2, 1)), os.difftime(5, 2)
-- case: os.time normalization
local t = os.time({year = 2021, month = 13, day = 32, hour = 0})
local d = os.date("!*t", os.time({year = 2000, month = 1, day = 1, hour = 0, isdst = false}))
return os.date("%Y-%m-%d", t), E(os.time, {year = 2020}), E(os.time, {year = 2020, month = "x", day = 1}), d.yday, d.wday, d.isdst
-- case: os missing functions
return type(os.time), type(os.date), type(os.clock), type(os.difftime)
-- case: integer keys normalization in constructors
local t = {[1.0] = "a", [2] = "b"}
return #t, t[1], math.type(next(t))
-- case: numeric for with float accumulation
local r = {}
for x = 0, 1, 0.25 do r[#r+1] = x end
for x = 1, 0, -0.5 do r[#r+1] = x end
return table.concat(r, ",")
-- case: while and nested break
local i, r = 0, 0
while true do
  i = i + 1
  for j = 1, 10 do if j > i then break end r = r + j end
  if i >= 4 then break end
end
return r
-- case: local function recursion and upvalue sharing
local function fact(n) if n <= 1 then return 1 end return n * fact(n - 1) end
local a = 0
local function inc() a = a + 1 end
local function get() return a end
inc(); inc()
return fact(20), get(), fact(21)
-- case: long concat and integer to string
local parts = {}
for i = 1, 30 do parts[i] = i end
return table.concat(parts) == ("123456789101112131415161718192021222324252627282930"), 1 .. 2 == "12"
-- case: attribs const folding
local x <const> = 10
local s <const> = "str"
local function f() return x * 2, s .. "!" end
return f()
-- case: integer division by negative zero float
return 1 // -0.0, -1 // -0.0, 0.0 / 0.0 ~= 0.0 / 0.0, 1 % -0.0 ~= 1 % -0.0, math.type(3 // 1.0)
-- case: comparison chains of strings with numbers coerced
return E(function() return "10" < 5 end), "10" == 10, E(function() return {} < 1 end)
