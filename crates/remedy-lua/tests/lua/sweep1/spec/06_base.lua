-- case: assert
return pcall(assert, false), pcall(assert, nil, "msg"), pcall(assert, false, {1}), select("#", assert(1, 2, 3)), pcall(assert), pcall(assert, false, nil)
-- case: error levels and values
local function f() error("lvl1") end
local function g() error("lvl2", 2) end
local function h() g() end
return pcall(f), pcall(h), pcall(error), pcall(error, nil), pcall(error, {}), pcall(error, "x", 0), pcall(error, 42), pcall(error, "x", 3)
-- case: error with non-string at level adds no position
return pcall(error, 42, 1), pcall(error, true), select("#", pcall(error))
-- case: xpcall with traceback-like handler gets original
return xpcall(function() local x = nil; return x.y end, function(m) return (m:gsub("^t:%d+: ", "")) end)
-- case: error in __tostring during error
return pcall(error, setmetatable({}, {__tostring = function() return "obj" end}))
-- case: select
return select(2, "a", "b", "c"), select(-1, "a", "b", "c"), select(-3, "a", "b", "c"), select("#"), select(5, 1, 2), pcall(select, 0, 1), pcall(select, -4, 1, 2, 3), pcall(select, "x"), select(2.0, 1, 2), pcall(select, 1.5, 1)
-- case: type and errors
return type(nil), type(print), type(type), pcall(type), type(coroutine.create(print))
-- case: rawequal rawlen rawget rawset
local t = setmetatable({}, {__index = function() return 1 end, __newindex = function() end, __len = function() return 9 end, __eq = function() return true end})
rawset(t, "a", 2)
return rawget(t, "a"), rawget(t, "b"), rawlen(t), rawlen("abc"), pcall(rawlen, 5), rawequal(t, setmetatable({}, getmetatable(t))), pcall(rawget, "s", 1), pcall(rawset, {}, nil, 1), rawequal("a", "a"), pcall(rawequal, 1)
-- case: next and pairs
local t = {10, 20, x = 1}
local n = 0
for k, v in pairs(t) do n = n + 1 end
return n, next({}), pcall(next, {}, "nokey"), next({5}), pcall(next), pcall(pairs), pcall(pairs, nil)
-- case: ipairs stops at nil and respects __index
local t = setmetatable({1, 2}, {__index = function(t, i) if i <= 4 then return i * 10 end end})
local r = {}
for i, v in ipairs(t) do r[#r + 1] = v end
return table.concat(r, ","), pcall(ipairs), select("#", ipairs({}))
-- case: tostring
return tostring(nil), tostring(true), tostring(12), tostring(1.5), tostring("s"), pcall(tostring), tostring(-0.0), tostring(1e100)
-- case: tostring with __name non-string
local u = setmetatable({}, {__name = 42})
return (tostring(u):match("^table"))
-- case: load with function reader
local parts = {"return ", "1 ", "+ ", "2"}
local i = 0
local f = load(function() i = i + 1 return parts[i] end)
local g = load(function() return nil end)
return f(), g ~= nil, g and g()
-- case: load reader returning empty string ends
local i = 0
local f = load(function() i = i + 1 if i == 1 then return "return 7" end return "" end)
return f()
-- case: load with nil env
local f = load("return x", "c", "t", nil)
return pcall(f)
-- case: load binary chunk rejected
return (load("\27Lua")) == nil
-- case: load upvalue is _ENV
x_global = 3
local f = load("x_global = x_global + 1 return x_global")
return f(), x_global
-- case: _ENV manipulation
local function f()
  local _ENV = {y = 7}
  return y
end
local g
do
  local _ENV = setmetatable({}, {__index = _G})
  z_local_env = 1
  g = function() return z_local_env, rawget(_G, "z_local_env") end
end
return f(), g()
-- case: _G and _VERSION
return _G._G == _G, _VERSION
-- case: collectgarbage options
return type(collectgarbage("count")), collectgarbage("step") ~= nil, collectgarbage("isrunning"), collectgarbage() , pcall(collectgarbage, "bogus")
-- case: print exists
return type(print), select("#", print())
-- case: getmetatable of various
return getmetatable(1), getmetatable(nil), getmetatable(print), type(getmetatable("")), pcall(getmetatable)
-- case: tostring integer floats
return tostring(3.0), tostring(-3.0), tostring(1e15 + 0.5), tostring(2^63 + 0.0), tostring(0.1), 3 | 0, tostring(1/0), tostring(-1/0)
-- case: string to number coercion in concat uses LUAI_NUMFFORMAT
return 1.5 .. "", 10 // 1 .. "", 2^63 .. "", 0.1 .. "", -0.0 .. "", 1e100 .. ""
