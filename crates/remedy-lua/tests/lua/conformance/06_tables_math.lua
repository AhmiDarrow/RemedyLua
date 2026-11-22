-- case: table.insert remove (6.6)
local t = {1, 2, 3}
table.insert(t, 4)
table.insert(t, 1, 0)
local r = table.remove(t, 2)
local last = table.remove(t)
return table.concat(t, ","), r, last, table.remove({}), #t
-- case: table.move unpack pack
local t = table.move({1, 2, 3}, 1, 3, 2)
local p = table.pack(1, nil, 3)
return table.concat(t, ","), p.n, p[3], table.unpack({1, 2, 3}), table.unpack({1, 2, 3}, 2), table.unpack({1, 2}, 1, 4)
-- case: table.concat errors -- expect error
return table.concat({1, {}, 3})
-- case: table.sort with comparator
local t = {5, 3, 8, 1, 9, 2}
table.sort(t, function(a, b) return a > b end)
local words = {"pear", "Apple", "fig", "banana"}
table.sort(words)
local big = {}
for i = 1, 200 do big[i] = (i * 37) % 101 end
table.sort(big)
local ok = true
for i = 2, #big do if big[i - 1] > big[i] then ok = false end end
return table.concat(t, ","), table.concat(words, ","), ok
-- case: invalid sort order -- expect error
local t = {}
for i = 1, 100 do t[i] = i % 7 end
table.sort(t, function(a, b) return true end)
-- case: math functions (6.7)
return math.floor(3.7), math.ceil(3.2), math.floor(-3.5), math.abs(-4), math.abs(-4.5), math.max(1, 5, 3), math.min(2, -1.5), math.fmod(7, 3), math.fmod(-7, 3), math.fmod(7.5, 2), math.sqrt(16), math.huge > 1e308, math.pi
-- case: math more
return math.modf(3.7), math.modf(-3.7), math.modf(5), math.tointeger(5.0), math.tointeger("8"), math.type("1"), math.ult(1, -1), math.exp(0), math.log(8, 2), math.log(100, 10), math.log(1)
-- case: math.random deterministic seed
math.randomseed(42)
local a, b, c = math.random(1, 100), math.random(10), math.random()
math.randomseed(42)
return a == math.random(1, 100), b == math.random(10), c == math.random(), math.random(5, 5)
-- case: metatables index newindex (2.4)
local base = {greet = function(self) return "hi " .. self.name end}
local obj = setmetatable({name = "x"}, {__index = base})
local log = {}
local proxy = setmetatable({}, {__index = function(t, k) return k .. "!" end, __newindex = function(t, k, v) log[#log + 1] = k .. "=" .. v; rawset(t, k, v) end})
proxy.a = 1
proxy.a = 2
return obj:greet(), proxy.zzz, proxy.a, table.concat(log, ",")
-- case: arithmetic metamethods
local V = {}
V.__index = V
V.__add = function(a, b) return setmetatable({x = a.x + b.x}, V) end
V.__unm = function(a) return setmetatable({x = -a.x}, V) end
V.__eq = function(a, b) return a.x == b.x end
V.__lt = function(a, b) return a.x < b.x end
V.__le = function(a, b) return a.x <= b.x end
V.__len = function(a) return a.x end
V.__concat = function(a, b) return "V" .. (type(a) == "table" and a.x or a) .. (type(b) == "table" and b.x or b) end
V.__call = function(self, y) return self.x * y end
V.__tostring = function(a) return "V(" .. a.x .. ")" end
V.__idiv = function() return "idiv" end
V.__band = function() return "band" end
local a, b = setmetatable({x = 1}, V), setmetatable({x = 2}, V)
return (a + b).x, (-a).x, a == setmetatable({x = 1}, V), a < b, b <= a, #b, a .. "s", 1 .. a, a(10), tostring(b), a // b, a & 1
-- case: __index chains and rawget
local A = {a = 1}
local B = setmetatable({b = 2}, {__index = A})
local C = setmetatable({}, {__index = B})
return C.a, C.b, rawget(C, "a"), rawequal(C, C), rawlen({1, 2})
-- case: __metatable protection
local t = setmetatable({}, {__metatable = "locked"})
return getmetatable(t), pcall(setmetatable, t, {}), getmetatable("abc").__index == string
-- case: __pairs
local t = setmetatable({}, {__pairs = function(t) return function(_, k) if not k then return 1, "one" end end, t, nil end})
local out = {}
for k, v in pairs(t) do out[#out + 1] = k .. v end
return table.concat(out)
-- case: weak tables collect (2.5.4)
local w = setmetatable({}, {__mode = "k"})
do local key = {} w[key] = true end
w[1] = 1
collectgarbage()
collectgarbage()
local n = 0
for _ in pairs(w) do n = n + 1 end
return n
-- case: coroutines (2.6)
local co = coroutine.create(function(a, b)
  local c = coroutine.yield(a + b)
  local d, e = coroutine.yield(c * 2)
  return d + e
end)
local r = {}
r[#r + 1] = select(2, coroutine.resume(co, 1, 2))
r[#r + 1] = select(2, coroutine.resume(co, 10))
r[#r + 1] = select(2, coroutine.resume(co, 3, 4))
r[#r + 1] = coroutine.status(co)
r[#r + 1] = select(2, coroutine.resume(co))
return table.concat(r, ",")
-- case: coroutine wrap generator
local function gen(n) return coroutine.wrap(function() for i = 1, n do coroutine.yield(i) end end) end
local s = 0
for v in gen(5) do s = s + v end
return s
-- case: yield across pcall
local co = coroutine.wrap(function()
  local ok, v = pcall(function() local x = coroutine.yield(1) error("e" .. x, 0) end)
  coroutine.yield(v)
  return "end"
end)
return co(), co(5), co()
-- case: coroutine errors
local co = coroutine.create(function() error("oops") end)
local ok, e = coroutine.resume(co)
local ok2, e2 = coroutine.resume(co)
local ok3, e3 = pcall(coroutine.yield, 1)
return ok, e, ok2, e2, coroutine.status(co), coroutine.isyieldable(), ok3, e3
-- case: coroutine running and close
local main, ismain = coroutine.running()
local co = coroutine.create(function() local x <close> = setmetatable({}, {__close = function() closed = true end}) coroutine.yield() end)
coroutine.resume(co)
local r = coroutine.close(co)
return ismain, type(main), r, closed, coroutine.status(co)
-- case: wrap error propagates
local f = coroutine.wrap(function() error("bad") end)
local ok, e = pcall(f)
return ok, e
-- case: load (6.1)
local f = load("return 1 + ...")
local g, err = load("return +")
local env = {x = 5}
local h = load("return x", "chunk", "t", env)
local parts = {"return ", "42"}
local i = 0
local k = load(function() i = i + 1 return parts[i] end)
return f(2), g, type(err), h(), k()
-- case: _ENV (2.2)
local function f()
  local _ENV = {y = 7}
  return y
end
local _ENV = setmetatable({}, {__index = _G})
z = 3
return f(), z, _G.z
