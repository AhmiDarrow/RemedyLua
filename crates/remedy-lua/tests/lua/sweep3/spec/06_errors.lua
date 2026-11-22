-- case: variable names in errors
local up = nil
local t = {}
return E(function() return up.x end), E(function() return t.a.b end), E(function() return glob.x end), E(function() return t[1].x end), E(function() local s = "k" return t[s].x end), E(function() return t:nomethod() end), E(function() return string.nofunc() end), E(function() up() end), E(function() glob() end), E(function() t.f() end), E(function() t[1]() end)
-- case: arith operand names
local up = {}
local t = {}
return E(function() return up + 1 end), E(function() return t.x + 1 end), E(function() return 1 + glob end), E(function() local a = "x" return a * 2 end), E(function() return t.x .. "s" end), E(function() return 2 ^ {} end), E(function() return up & 1 end), E(function() return 1.5 & 1 end), E(function() local f = 1.5 return f | 0 end), E(function() return t.x < 1 end)
-- case: constant names in errors
return E(function() local s <const> = "abc" return s.x.y end), E(function() return ("abc").x.y end), E(function() return (nil)() end), E(function() return (1)() end), E(function() return ("x")() end)
-- case: upvalue call and field call
local obj = {}
local function f() return obj.method() end
local function g() return obj:method() end
return E(f), E(g)
-- case: method call line on separate lines
local obj = {}
local ok, e = pcall(function()
  return obj
    :method(
      1)
end)
local ok2, e2 = pcall(function()
  local s = nil
  return s
    :method(1)
end)
local ok3, e3 = pcall(function()
  return obj:method
  (1)
end)
return e, e2, e3
-- case: call line on separate lines
local t = {}
local ok, e = pcall(function()
  return t.f
  (
  1
  )
end)
local ok2, e2 = pcall(function()
  t.g(1,
    2)
end)
local ok3, e3 = pcall(function()
  t.h "s"
end)
return e, e2, e3
-- case: integer conversion errors
return E(function() return 1 << 1.5 end), E(function() return math.huge // 1 | 0 end), E(function() local t = {} for i = 1, 2 do t[i] = i end return ("x"):rep(1.5) end), E(string.rep, "x", 2^63), E(string.sub, "x", 2^63), E(string.char, 65.5)
-- case: bad argument messages
return E(string.rep), E(string.rep, "x"), E(("x").rep, "x", {}), E(function() return ("x"):rep({}) end), E(math.floor), E(table.concat, 1), E(setmetatable, {}), E(ipairs), E(string.byte, {}), E(string.upper), E(tostring), E(math.abs, "a"), E(table.insert, nil, 1)
-- case: bad argument via method self
local s = "x"
return E(function() return s.rep() end), E(function() return string.rep(nil) end), E(function() return ("abc"):sub({}) end), E(function() return ("abc"):find(nil) end)
-- case: error with non-string values
return select(2, pcall(error, 42)), select(2, pcall(error, nil)), select(2, pcall(error, true)), type(select(2, pcall(error, {}))), select(2, pcall(error, 42, 1)), select(2, pcall(error, 4.5, 2))
-- case: error in metamethod position
local mt = {__add = function(a, b) error("in add") end, __index = function(t, k) error("idx " .. k, 2) end}
local t = setmetatable({}, mt)
return E(function() return t + 1 end), E(function() return t.zz end)
-- case: tostring of error with level 2 from native
local function f() local x = nil; return #x end
return E(f), E(function() return #{} .. nil end)
-- case: string concat errors name
local a, b = "x", nil
local t = {}
return E(function() return a .. b end), E(function() return a .. t.q end), E(function() return a .. {} .. "z" end), E(function() return 1 .. t end)
-- case: for iterator errors
return E(function() for k in nil do end end), E(function() for k in 5 do end end), E(function() local t = {} for k in t.x do end end), E(function() for k in pairs(nil) do end end)
-- case: comparison error messages
return E(function() return {} < {} end), E(function() return 1 < "x" end), E(function() return "x" <= 1 end), E(function() return nil > 1 end), E(function() return true >= false end), E(function() return print < print end)
-- case: setting fields on non-tables
local s = "x"
return E(function() s.y = 1 end), E(function() local n = 1 n.x = 2 end), E(function() glob.x = 1 end), E(function() local t = {} t[nil] = 1 end), E(function() local t = {} t[0/0] = 1 end), (function() local t = {} return t[nil] end)()
-- case: call non-function via pcall directly
return pcall(nil), pcall({}), pcall(1, 2)
-- case: stack overflow message
local function r() return 1 + r() end
local ok, e = pcall(r)
return ok, (e:gsub("^.-:%d+: ", ""))
-- case: error object passthrough
local obj = {}
local ok, e = pcall(error, obj)
local ok2, e2 = pcall(function() error(obj) end)
return e == obj, e2 == obj
-- case: xpcall traceback-like handler receives raw value
local ok, v = xpcall(function() local x = nil; return x.y end, function(m) return "H:" .. m end)
return ok, v
-- case: error in __tostring used by tostring
local t = setmetatable({}, {__tostring = function() error("ts") end})
return E(tostring, t)
-- case: errors in nested function names
local M = {}
function M.f() error("mf") end
function M:g() local x = nil; return x.y end
return E(M.f), E(M.g, M)
-- case: global function names in bad argument
function gf(...) return string.rep(...) end
local ok, e = pcall(gf)
local lr = string.rep
local ok2, e2 = pcall(function() return lr() end)
local t = {rep = string.rep}
local ok3, e3 = pcall(function() return t.rep() end)
return e, e2, e3
-- case: method name and arguments on different lines index error
local s = nil
local ok, e = pcall(function()
  return s:method
  (1)
end)
local ok2, e2 = pcall(function()
  return s:method(
  1)
end)
local ok3, e3 = pcall(function()
  return s:method{
  }
end)
local ok4, e4 = pcall(function()
  return s
  :
  method
  "x"
end)
local ok5, e5 = pcall(function()
  local o = {}
  return o:method
  (1)
end)
return e, e2, e3, e4, e5
