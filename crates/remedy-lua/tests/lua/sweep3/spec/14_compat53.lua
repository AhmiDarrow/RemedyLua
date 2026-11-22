-- case: le falls back to not lt with swapped operands
local log = {}
local mt = {__lt = function(a, b) log[#log+1] = a.n .. "<" .. b.n return a.n < b.n end}
local a, b = setmetatable({n = 1}, mt), setmetatable({n = 2}, mt)
local r = {a <= b, b <= a, a >= b, b >= a, a <= a}
return table.concat(log, ","), r[1], r[2], r[3], r[4], r[5]
-- case: le fallback uses second operand lt and mixed types
local mt = {__lt = function(x, y) return type(x) == "number" end}
local t = setmetatable({}, mt)
return t <= 1, 1 <= t, E(function() return {} <= t end), E(function() return {} <= {} end), E(function() return setmetatable({}, {__lt = 1}) <= 1 end)
-- case: le prefers __le over __lt
local t = setmetatable({}, {__le = function() return "le" end, __lt = function() return false end})
return t <= t, t >= t
-- case: le fallback yields inside coroutine
local mt = {__lt = function(x, y) return coroutine.yield("cmp") end}
local a, b = setmetatable({}, mt), setmetatable({}, mt)
local co = coroutine.wrap(function() local r1 = a <= b local r2 = a <= b if a <= b then return r1, r2, "then" end return r1, r2, "else" end)
return co(), co(true), co(false), co(1)
-- case: le fallback errors name position
local mt = {__lt = function() error("inlt") end}
local a = setmetatable({}, mt)
return E(function() return a <= a end)
-- case: compat math functions
local r = {}
for _, v in ipairs{0, -0.0, 0.5, -1, 3, 1e300, 5e-324, 1e-310} do
  local m, e = math.frexp(v)
  r[#r+1] = string.format("%.17g %d %.15g %.15g %.15g %.15g", m, e, math.cosh(v), math.sinh(v), math.tanh(v), math.log10(math.abs(v) + 1))
end
r[#r+1] = string.format("%.17g %.17g %.17g %.17g %.17g", math.pow(2, 10), math.pow(8, 1/3), math.ldexp(1, 1023), math.ldexp(1, -1074), math.ldexp(3, -1076))
r[#r+1] = string.format("%.17g %.17g %.17g", math.ldexp(1, 2^32 + 3), math.atan2(1, -1), math.atan2(1))
r[#r+1] = math.type(math.pow(2, 2)) .. math.type(select(2, math.frexp(8))) .. math.type(math.ldexp(1, 2))
r[#r+1] = tostring(math.log10(0)) .. tostring(math.frexp(math.huge)) .. tostring(math.frexp(-math.huge))
return table.concat(r, " | "), E(math.ldexp, 1, 1.5), E(math.ldexp, 1), E(math.pow, 2), E(math.frexp, "x"), math.pow("2", "3")
