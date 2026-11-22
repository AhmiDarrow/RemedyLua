-- case: table.insert remove
local t = {1, 2, 3}
table.insert(t, 4); table.insert(t, 1, 0)
local r1 = table.remove(t); local r2 = table.remove(t, 1)
local e = {}
return table.concat(t, ","), r1, r2, table.remove(e), table.remove(e, 0), #e, E(table.insert, t, 10, 1), E(table.insert, t, 0, 1), E(table.insert, t), E(table.insert, t, 1, 2, 3), E(table.remove, t, 7), table.remove(t, #t + 1)
-- case: table.remove at n+1 and 0
local t = {n = 1}
t[0] = "z"
return table.remove(t, 0), t[0], table.remove({}, 0), E(table.remove, {}, -1), E(table.remove, {1}, 3)
-- case: table.concat
return table.concat({1, 2.5, "x"}, "-"), table.concat({}, ","), table.concat({1, 2, 3}, ",", 2), table.concat({1, 2, 3}, ",", 2, 1), E(table.concat, {1, {}, 3}), E(table.concat, {1}, ",", 1, 2), table.concat({"a"}, ",", 1, 1)
-- case: table.unpack and pack
local p = table.pack(1, nil, 3)
return p.n, table.unpack({1, 2, 3}), table.unpack({1, 2, 3}, 2), table.unpack({1, 2, 3}, 2, 5), select("#", table.unpack({}, 1, 0)), table.unpack({1, 2}, -1, 1), select("#", table.unpack({}, math.maxinteger, math.maxinteger))
-- case: table.move
local a = {1, 2, 3, 4, 5}
table.move(a, 2, 4, 1)
local b = {1, 2, 3, 4, 5}
table.move(b, 1, 3, 3)
local c = table.move({1, 2}, 1, 2, 1, {})
return table.concat(a, ","), table.concat(b, ","), table.concat(c, ","), E(table.move, {}, 1, math.maxinteger, 2), E(table.move, {}, -1, math.maxinteger, 2), E(table.move, {1}, 1, 1, math.maxinteger), #table.move({}, 1, 0, 5)
-- case: table.move metamethods
local log = {}
local src = setmetatable({}, {__index = function(t, k) log[#log+1] = "r" .. k return k * 10 end})
local dst = setmetatable({}, {__newindex = function(t, k, v) log[#log+1] = "w" .. k .. "=" .. v end})
table.move(src, 1, 3, 2, dst)
return table.concat(log, ",")
-- case: table.sort
local t = {5, 2, 8, 1, 9, 3}
table.sort(t)
local s = {"b", "a", "C"}
table.sort(s)
local d = {3, 1, 2}
table.sort(d, function(x, y) return x > y end)
return table.concat(t, ","), table.concat(s, ","), table.concat(d, ","), E(table.sort, {1, "a", 2}), E(table.sort, {1, 2, 3}, 5)
-- case: table.sort invalid order
local t = {}
for i = 1, 100 do t[i] = (i * 37) % 17 end
local ok, e = pcall(table.sort, t, function(a, b) return true end)
return ok, e
-- case: table functions with metamethods
local proxy = setmetatable({}, {__index = function(t, i) if i <= 3 then return i * 2 end end, __len = function() return 3 end})
return table.concat(proxy, ","), table.unpack(proxy)
-- case: select
return select("#"), select("#", nil, nil), select(2, "a", "b", "c"), select(-1, "a", "b", "c"), select(-3, "a", "b", "c"), E(select, -4, "a", "b", "c"), E(select, 0, "a"), select(5, "a"), E(select, "x"), select(2.0, "a", "b"), E(select, 1.5, 1), select("#", select(1, 1, nil))
-- case: tostring tonumber misc
return tostring(nil), tostring(true), tostring(12), tostring("x"), E(tostring), type(tostring(print)), tostring(print):match("^builtin") ~= nil or tostring(print):match("^function") ~= nil
-- case: type errors
return type(nil), type(1), type("x"), type({}), type(print), type(coroutine.create(function() end)), E(type)
-- case: rawequal rawlen rawget rawset
local t = {}
return rawequal(1, 1.0), rawequal("a", "a"), rawequal({}, {}), rawlen({1, 2}), rawlen("abc"), E(rawlen, 1), rawget(t, "x"), rawset(t, "x", 1) == t, E(rawset, t, nil, 1), E(rawget, "x", 1), E(rawequal, 1), E(rawset, t, 0/0, 1)
-- case: next and pairs
local t = {10, 20, x = 1}
local n = 0
for k, v in pairs(t) do n = n + 1 end
return n, next({}), next({5}), E(next, {}, "nope"), type(next), ipairs({}) == ipairs({1}), select("#", ipairs({}))
-- case: ipairs with index
local t = setmetatable({}, {__index = function(t, i) if i < 4 then return i end end})
local r = {}
for i, v in ipairs(t) do r[#r+1] = v end
return table.concat(r, ","), E(ipairs), E(pairs)
-- case: assert and error
return E(assert, false), E(assert, nil, "msg"), E(assert, false, {}) ~= nil, select("#", assert(1, 2, 3)), E(assert), E(error), E(error, nil), E(error, "x", 0), E(error, "x", 1), E(error, "x", 2), E(error, 5), E(error, setmetatable({}, {__tostring = function() return "obj" end})) ~= nil
-- case: pcall xpcall
local function h(e) return "handled:" .. tostring(e) end
return pcall(error), pcall(error, nil), xpcall(error, h, "x"), xpcall(function(...) return ... end, h, 1, 2), E(xpcall, print), select("#", pcall(function() end)), xpcall(function() error({}) end, function(e) return type(e) end)
-- case: error levels positions
local function lvl1() error("e1") end
local function lvl2() error("e2", 2) end
local function call2() lvl2() end
return E(lvl1), E(call2), E(error, "e3", 3)
-- case: getmetatable setmetatable
local t = {}
return E(setmetatable, t, 1), E(setmetatable, 1, {}), setmetatable(t, nil) == t, getmetatable(1), getmetatable(nil), E(getmetatable)
-- case: load with env and mode
local env = {x = 5}
local f = load("return x", "chunk", "t", env)
local g = load("x = 10; return x", "=c", "t", env)
return f(), g(), env.x, load("return 1", "c", "b"), E(load, "return 1", "c", "q"), load("syntax error here"), load("return ...", "c")(1, 2)
-- case: load reader
local parts = {"return ", "1 ", "+ 2", nil}
local i = 0
local f = load(function() i = i + 1 return parts[i] end)
local f2, e2 = load(function() return {} end)
local f3 = load(function() return nil end)
local f4, e4 = load(function() error("rd") end)
return f(), f2, e2, f3(), f4, e4
-- case: load chunk names
return L("x = = 1", "=myname"), L("x = = 1", "@file.lua"), L("x = = 1", "a string chunk"), L("x = = 1"), L("x = = 1", "line1\nline2"), L("x = = 1", "=" .. ("n"):rep(100)), L("x = = 1", ("long"):rep(30)), L("x = = 1", "@" .. ("p"):rep(100))
-- case: load env upvalue
local f = load("local a = 1; return function() return x end")
local inner = f()
return inner(), load("return _ENV")() == _ENV, load("return _ENV", "c", "t", nil)()
-- case: load binary mode rejects text
return load("return 1", "x", "b")
-- case: collectgarbage options
return type(collectgarbage("count")), collectgarbage("isrunning"), collectgarbage(), collectgarbage("step"), E(collectgarbage, "bogus"), collectgarbage("incremental"), collectgarbage("generational"), collectgarbage("incremental")
-- case: print and tostring use
local t = setmetatable({}, {__tostring = function() return "P" end})
return E(print, setmetatable({}, {__tostring = function() return {} end}))
-- case: _G and _VERSION
return _VERSION, _G._G == _G, _G.print == print, _ENV == _G
-- case: select with negative mininteger
return E(select, math.mininteger, 1), E(select, -1)
-- case: tostring of integer-valued floats
return tostring(-0.0), tostring(1e15), tostring(2^53), tostring(1e16), 1e15 .. "", 2^24 .. ""
-- case: next after modification during traversal
local t = {a = 1, b = 2, c = 3}
local n = 0
for k in pairs(t) do t[k] = nil n = n + 1 end
return n, next(t)
-- case: table constructors
local function f() return 1, 2, 3 end
local t = {f(), f()}
local u = {f(), (f())}
local v = {[1] = "a", "b"}
local w = {f(), nil}
return #t, #u, v[1], #w, select("#", table.unpack({1, nil, 3}, 1, 3))
-- case: unpack with nil holes
return table.unpack({1, nil, 3}, 1, 3)
-- case: table.concat number formatting
return table.concat({1, 1.0, -0.0, 1e100, 2^63}, " ")
-- case: table.insert pos as float
local t = {1, 2}
table.insert(t, 1.0, "x")
return table.concat(t, ","), E(table.insert, t, 1.5, "y"), E(table.insert, t, "a", "y")
-- case: sort with nil hole errors
return E(table.sort, {3, nil, 1}), E(table.sort, {1, 2}, function(a, b) return a.x < b end)
-- case: xpcall handler must be a function
local callable = setmetatable({}, {__call = function() return "c" end})
return E(xpcall, print, 1), E(xpcall, print, nil), E(xpcall, error, callable), E(xpcall, print), select("#", xpcall(print, print))
