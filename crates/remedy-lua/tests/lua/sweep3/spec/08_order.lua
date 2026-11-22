-- case: assignment store order with newindex
local log = {}
local t = setmetatable({}, {__newindex = function(t, k, v) log[#log+1] = k .. "=" .. tostring(v) end})
t.a, t.b, t.c = 1, 2, 3
local i = 1
local u = setmetatable({}, {__newindex = function(_, k, v) log[#log+1] = "u" .. k end})
u[i], u[i + 1] = 1, 2
return table.concat(log, ",")
-- case: evaluation order of args and constructor
local log = {}
local function f(x) log[#log+1] = x return x end
local t = {f(1), f(2), k = f(3), [f(4)] = f(5), f(6)}
local g = function(...) return select("#", ...) end
g(f(7), f(8))
local _ = f(9) + f(10) * f(11)
return table.concat(log, ",")
-- case: index evaluation order with metamethods
local log = {}
local mt = {__index = function(t, k) log[#log+1] = "get" .. k return 0 end}
local a = setmetatable({}, mt)
local b = setmetatable({}, mt)
local s = a.x + b.y
local c = a.p .. "" .. b.q
return table.concat(log, ",")
-- case: collectgarbage compat options
local p = collectgarbage("setpause", 100)
local p2 = collectgarbage("setpause", p)
local m = collectgarbage("setstepmul", 200)
local m2 = collectgarbage("setstepmul", m)
collectgarbage("stop")
local r1 = collectgarbage("isrunning")
collectgarbage("restart")
local r2 = collectgarbage("isrunning")
return p, p2, m, m2, r1, r2, math.type(collectgarbage("count")), select("#", collectgarbage("count")), type(collectgarbage("step", 0)), collectgarbage("collect")
-- case: collectgarbage incremental args
return collectgarbage("incremental", 100, 100, 10), collectgarbage("generational", 20, 100), collectgarbage("incremental"), E(collectgarbage, "count", "x") ~= nil, E(collectgarbage, 1)
-- case: table.remove at zero border
local t = {[0] = "z"}
local r = table.remove(t)
local u = {}
return r, t[0], table.remove(u), select("#", table.remove(u))
-- case: table.unpack on non-table
return table.unpack("abc", 1, 2), select("#", table.unpack("abc", 1, 2)), E(table.unpack, nil), table.pack().n, #table.pack()
-- case: load BOM and shebang
return L("\xEF\xBB\xBFreturn 1"), L("#!/bin/lua\nreturn 1"), L("return 1 --[[x]]"), L("return 1\n#")
-- case: coroutine close normal and running
local outer
local r = {}
outer = coroutine.create(function()
  local inner = coroutine.create(function() r[#r+1] = E(coroutine.close, outer) end)
  coroutine.resume(inner)
  r[#r+1] = E(coroutine.close, coroutine.running())
end)
coroutine.resume(outer)
return r[1], r[2], E(coroutine.close, 5), E(coroutine.status, "x"), E(coroutine.wrap, 1), E(coroutine.create, nil)
-- case: yield inside close metamethod
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function() coroutine.yield("from close") end})
  return "done"
end)
local a, b = coroutine.resume(co)
local c, d = coroutine.resume(co)
return a, b, c, d, coroutine.status(co)
-- case: close metamethod receives error object
local got
local ok, e = pcall(function()
  local x <close> = setmetatable({}, {__close = function(_, err) got = err end})
  error({tag = "T"})
end)
local got2
pcall(function()
  local y <close> = setmetatable({}, {__close = function(_, err) got2 = err end})
  local z = nil
  return z.q
end)
return type(got), got and got.tag, e == got, got2
-- case: close order with nested blocks and errors in middle
local log = {}
local function mk(n, f) return setmetatable({}, {__close = function(_, e) log[#log+1] = n .. ":" .. tostring(e) if f then error(n, 0) end end}) end
local ok, e = pcall(function()
  local a <close> = mk("a")
  do
    local b <close> = mk("b", true)
    local c <close> = mk("c")
  end
end)
return table.concat(log, ","), e
-- case: close variable is const
return L("local x <close> = nil; x = 2"), L("local x <close> = nil; local function f() x = 1 end")
-- case: generic for closing value on error and break
local log = {}
local cl = setmetatable({}, {__close = function(_, e) log[#log+1] = "closed:" .. tostring(e) end})
pcall(function() for i in function(_, c) c = (c or 0) + 1 if c > 2 then error("it", 0) end return c end, nil, nil, cl do end end)
local function f() for i in function(_, c) return (c or 0) + 1 end, nil, nil, cl do return i end end
f()
return table.concat(log, ",")
-- case: upvalue sharing between closures and loops
local fs = {}
for i = 1, 2 do
  local shared = i * 10
  fs[#fs+1] = function() shared = shared + 1 return shared end
  fs[#fs+1] = function() return shared end
end
fs[1](); fs[1]()
return fs[2](), fs[4](), fs[3](), fs[4]()
-- case: recursive local function and upvalue of itself
local function a(n) if n == 0 then return "done" end return a(n - 1) end
local b
b = function(n) if n == 0 then return "b" end return b(n - 1) end
return a(100), b(100)
-- case: tail calls preserve results
local function t(n, acc) if n == 0 then return acc, "x" end return t(n - 1, acc + n) end
return t(100000, 0)
-- case: integer keys after float arithmetic
local t = {}
for i = 1, 3 do t[i / 1] = i end
return #t, t[1], t[2.0], math.type(next(t))
-- case: string coercion in table keys does not happen
local t = {}
t[1] = "int"
return t["1"], t[1]
-- case: concat chain with numbers and metamethod in middle
local mt = {__concat = function(a, b) local x = type(a) == "table" and "T" or a local y = type(b) == "table" and "T" or b return x .. y end}
local T = setmetatable({}, mt)
return 1 .. 2 .. T .. 3 .. 4, T .. T .. T
-- case: comparison of mixed int float strings
return 1 == 1.0, "1" == "1.0", 0.0 == -0.0, 1/0 == math.huge, math.maxinteger + 1.0 == 2^63, math.mininteger == -2^63, 2^53 == 2^53 + 1
-- case: not and comparison results are booleans
local t = setmetatable({}, {__lt = function() return "yes" end, __le = function() return nil end, __eq = function() return 0 end})
local u = setmetatable({}, getmetatable(t))
return t < u, t <= u, t == u, t ~= u, not t
-- case: method on number via string library not allowed
return E(function() local n = 5 return n:rep(2) end)
-- case: string.format on integers with float args that are integral
return string.format("%d %x %c", 3.0, 255.0, 65.0), E(string.format, "%d", 2^63), string.format("%.0f", 2^63), string.format("%5.1f", "1.25")
-- case: varargs with nils at end
local function f(...) return select("#", ...) end
local function g(...) return ... end
return f(g(1, nil, nil)), f(g()), f(nil, g()), f(g(), nil), f((g(1, 2)))
-- case: long string with CRLF normalization
local s = load("return [[\r\nab\r\ncd\n\rgh]]")()
return s, #s
-- case: string with escaped newlines CRLF
local s = load("return 'a\\\r\nb'")()
return s, #s
-- case: integer conversions in string.sub and rep
return ("abc"):sub("2"), ("abc"):sub(2.0, "3"), E(string.sub, "abc", 1.5), ("ab"):rep("2"), E(string.rep, "ab", "x")
-- case: math functions argument coercion
return math.floor("2.5"), math.sqrt("4"), math.abs("-3"), math.type(math.abs("-3")), math.max("10", 9), E(math.max, "a", 1), math.ult("1", 2)
-- case: bitwise ops on large floats
return 2^62 | 0, E(function() return 2^63 | 0 end), (-2^63) & -1, 3.0 ~ 1, E(function() return 0/0 | 0 end), E(function() return math.huge & 1 end)
-- case: integer parse of huge numerals in strings for bitwise
return "9223372036854775807" | 0, E(function() return "9223372036854775808" | 0 end), "0xffffffffffffffff" | 0, E(function() return "1e100" | 0 end), "1e2" | 0
