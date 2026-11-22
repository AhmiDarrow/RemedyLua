-- case: arithmetic subtypes (3.4.1)
return 1 + 2, 1 + 2.0, 7 // 2, 7.0 // 2, -7 // 2, 7 % -3, -7 % 3, 7.5 % 2, 2^2, 10 / 2, 3 | 5, 6 & 3, 5 ~ 3, ~0, 1 << 63, 1 << 64, -1 >> 1
-- case: integer overflow wraps (3.4.1)
return math.maxinteger + 1 == math.mininteger, math.mininteger - 1, math.maxinteger * 2, -math.mininteger
-- case: float to integer conversions (3.4.3)
return 3.0 | 0, "10" + 1, "0x10" + 0, "1e1" * 1, 10 == 10.0, math.tointeger(3.5), 2^53 == 2^53 + 1
-- case: comparisons int float (3.4.4)
return 1 < 1.5, 2^63 > math.maxinteger, -2^63 == math.mininteger, 1 == 1.0, "a" < "b", "a" < "ab", "Z" < "a", 0.0 == -0.0
-- case: string coercion and concat (3.4.6)
return 1 .. 2, 1.5 .. "", 10 .. "x", -0.0 .. "", 1e100 .. "", 2^63 .. ""
-- case: length operator (3.4.7)
return #"abc", #{1, 2, 3}, #{}, #{n = 1}
-- case: logical operators (3.4.5)
return 10 or 20, nil or "a", nil and 1, false and nil, false or nil, 10 and 20, not nil, not 0
-- case: precedence (3.4.8)
return 2 + 3 * 4 ^ 2 / 2, -2 ^ 2, not 1 == 2, 1 .. 2 .. 3, 2 ^ 3 ^ 2, 1 + 2 < 4 and "y" or "n", 5 - 3 - 1
-- case: table constructors (3.4.9)
local function f() return 1, 2, 3 end
local t = {f(), f(), [10] = "x", y = 1; 7, f()}
return t
-- case: varargs and select (3.4.11)
local function f(...) return select("#", ...), ... end
local function g(...) local a, b = ... return a, b end
return f(1, nil, 3), g(5), (f(1, 2))
-- case: multiple assignment evaluation (3.3.3)
local a, b, c = 1
local i = 3
local t = {}
i, t[i] = i + 1, 20
return a, b, c, i, t[3], t[4]
-- case: numeric for (3.3.5)
local s = {}
for i = 1, 3 do s[#s + 1] = i end
for i = 3, 1, -1 do s[#s + 1] = i end
for i = 1, 2, 0.5 do s[#s + 1] = i end
for i = 1.0, 3 do s[#s + 1] = i end
for i = math.maxinteger - 1, math.maxinteger do s[#s + 1] = i end
for i = 1, 0 do s[#s + 1] = "never" end
return s
-- case: for loop errors -- expect error
for i = 1, 10, 0 do end
-- case: generic for (3.3.5)
local out = {}
for k, v in ipairs({"a", "b", nil, "d"}) do out[#out + 1] = k .. v end
local function iter(s, i) if i < s then return i + 1, i * i end end
for i, sq in iter, 3, 0 do out[#out + 1] = sq end
return table.concat(out, ",")
-- case: goto and labels (3.3.4)
local i = 1
local out = {}
::top::
if i <= 3 then out[#out + 1] = i; i = i + 1; goto top end
for j = 1, 3 do
  for k = 1, 3 do
    if k == 2 then goto continue end
    out[#out + 1] = j * 10 + k
    ::continue::
  end
end
return table.concat(out, " ")
-- case: closures capture per iteration (3.5)
local fs = {}
for i = 1, 3 do fs[i] = function() return i end end
local t = {}
local x = 0
while x < 3 do x = x + 1; local y = x; t[x] = function() y = y + 10 return y end end
return fs[1](), fs[2](), fs[3](), t[1](), t[1](), t[2]()
-- case: upvalues shared (3.5)
local function counter()
  local n = 0
  return function() n = n + 1 return n end, function() return n end
end
local inc, get = counter()
inc() inc()
return get()
-- case: recursion deep
local function sum(n) if n == 0 then return 0 end return n + sum(n - 1) end
return sum(150)
-- case: tail calls do not grow the stack (3.4.10)
local function loop(n) if n == 0 then return "done" end return loop(n - 1) end
return loop(100000)
-- case: integer division and modulo by zero -- expect error
return 1 // 0
-- case: float division by zero
return 1 / 0, -1 / 0, 1 // 0.0, 1 % math.huge, -1 % math.huge
-- case: error values (2.3)
local ok, e = pcall(error, {code = 1})
local ok2, e2 = pcall(error, "msg", 0)
local ok3, e3 = pcall(error)
return ok, e.code, e2, e3
-- case: error positions
local ok, e = pcall(function() error("x") end)
local ok2, e2 = pcall(function() error("y", 2) end)
local ok3, e3 = pcall(function() local t = nil; return t.x end)
local ok4, e4 = pcall(function() return 1 + {} end)
local ok5, e5 = pcall(function() return #5 end)
local ok6, e6 = pcall(function() return {} < {} end)
local ok7, e7 = pcall(function() return 1 < "x" end)
local ok8, e8 = pcall(function() undefinedfn() end)
local ok9, e9 = pcall(function() local s = "x" return s:nomethod() end)
return e, e2, e3, e4, e5, e6, e7, e8, e9
-- case: xpcall handler
return xpcall(function() error("boom") end, function(m) return "handled: " .. m end)
-- case: to-be-closed (3.3.8)
local log = {}
local function closer(name) return setmetatable({}, {__close = function(_, err) log[#log + 1] = name .. ":" .. tostring(err) end}) end
do
  local a <close> = closer("a")
  local b <close> = closer("b")
end
local ok = pcall(function()
  local c <close> = closer("c")
  error("E", 0)
end)
local function f()
  local d <close> = closer("d")
  return "ret"
end
local r = f()
return table.concat(log, " "), ok, r
