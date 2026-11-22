-- case: sort is the same algorithm for equal keys
local recs = {}
for i = 1, 60 do recs[i] = {k = (i * 7) % 5, id = i} end
table.sort(recs, function(a, b) return a.k < b.k end)
local out = {}
for i, r in ipairs(recs) do out[i] = r.id end
return table.concat(out, ",")
-- case: sort non strict comparator
local r = {}
for _, n in ipairs{3, 5, 10, 20, 50, 100} do
  local t = {}
  for i = 1, n do t[i] = (i * 13) % 7 end
  local ok, e = pcall(table.sort, t, function(a, b) return a <= b end)
  r[#r+1] = tostring(ok) .. ":" .. (ok and table.concat(t, "") or e)
end
return table.concat(r, " ")
-- case: sort inconsistent comparator results
local r = {}
for seed = 1, 20 do
  local t = {}
  for i = 1, 30 do t[i] = (i * seed * 31) % 17 end
  local c = 0
  local ok, e = pcall(table.sort, t, function(a, b) c = c + 1 return (a + b + c) % 3 == 0 end)
  r[#r+1] = tostring(ok) .. ":" .. (ok and table.concat(t, ".") or e) .. ":" .. c
end
return table.concat(r, " ")
-- case: sort comparison count
local c = 0
local t = {}
for i = 1, 100 do t[i] = (i * 37) % 101 end
table.sort(t, function(a, b) c = c + 1 return a < b end)
return c, t[1], t[100]
-- case: sort with errors in comparator and mixed types
local t = {3, 1, "x", 2}
return E(table.sort, t), E(table.sort, {1, 2, 3}, function() error("cmp") end), E(table.sort, {{}, {}})
-- case: sort strings with embedded zeros and locale-free order
local t = {"b\0a", "b", "a\0", "a", "\0", "", "B", "ab"}
table.sort(t)
for i = 1, #t do t[i] = string.format("%q", t[i]) end
return table.concat(t, ",")
-- case: sort array length via __len
local t = setmetatable({5, 4, 3, 2, 1}, {__len = function() return 3 end})
table.sort(t)
return table.concat({t[1], t[2], t[3], t[4], t[5]}, ",")
-- case: table.concat with __index and __len
local t = setmetatable({}, {__index = function(_, i) return "v" .. i end, __len = function() return 3 end})
return table.concat(t, "-"), E(table.concat, setmetatable({}, {__len = function() return 2 end}))
-- case: table.insert and remove through metamethods
local log = {}
local store = {}
local t = setmetatable({}, {__index = store, __newindex = function(_, k, v) log[#log+1] = k .. "=" .. tostring(v) store[k] = v end, __len = function() return #store end})
table.insert(t, "a"); table.insert(t, "b"); table.insert(t, 1, "c")
local r = table.remove(t, 1)
return table.concat(log, ","), r
-- case: next with numeric keys order from array
local t = {10, 20, 30}
local k1 = next(t)
local k2 = next(t, k1)
return k1, k2, next(t, 3)
-- case: ipairs stops at nil and handles proxies
local t = {1, 2, nil, 4}
local n = 0
for i, v in ipairs(t) do n = n + 1 end
return n
-- case: pairs on table with __index does not use it
local t = setmetatable({a = 1}, {__index = {b = 2}})
local n = 0
for k in pairs(t) do n = n + 1 end
return n
-- case: tostring calls with extra args and select negative
return tostring(1, 2), select(-2, "a", "b", "c")
-- case: unpack with explicit range over nil
return select("#", table.unpack({}, 1, 3)), table.unpack({n = 2, 1, 2})
-- case: string.char of many
local t = {}
for i = 1, 255 do t[i] = i end
local s = string.char(table.unpack(t))
return #s, s:byte(255), s:sub(65, 70)
-- case: integer string conversions round trip
local r = {}
for _, v in ipairs{math.maxinteger, math.mininteger, 0, -1, 123456789} do r[#r+1] = tostring(math.tointeger(tostring(v))) end
return table.concat(r, ",")
-- case: tonumber of hex with fraction and exponent
return tonumber("0x1p4"), tonumber("0x.1"), tonumber("0X1P-1"), tonumber("0x1.8p1"), tonumber("-0x10"), tonumber("- 1"), tonumber("0x-1"), tonumber("1e+2"), tonumber("1E-2"), tonumber("0e0"), tonumber(".0"), tonumber("0."), tonumber("."), tonumber("e1")
-- case: tonumber huge exponents
return tonumber("1e400"), tonumber("-1e400"), tonumber("1e-400"), tonumber("0x1p99999"), tonumber("123456789012345678901234567890"), math.type(tonumber("9223372036854775807")), math.type(tonumber("9223372036854775808"))
-- case: tonumber base with whitespace and case
return tonumber("  Zz  ", 36), tonumber("\t10\n", 16), tonumber("1 0", 2), tonumber("-", 10), tonumber("-0", 10), tonumber("777777777777777777777777", 8), tonumber("1111111111111111111111111111111111111111111111111111111111111111", 2), tonumber("10000000000000000000000000000000000000000000000000000000000000000", 2)
-- case: string comparison of long strings at boundary
local a = ("x"):rep(40) .. "a"
local b = ("x"):rep(40) .. "b"
local c = ("x"):rep(39) .. "a"
return a < b, c < a, a == ("x"):rep(40) .. "a", #a
-- case: long string keys interned equality
local t = {}
local k1 = ("k"):rep(50)
local k2 = ("k"):rep(25) .. ("k"):rep(25)
t[k1] = 1
return t[k2], rawequal(k1, k2), next(t) == k2
-- case: table functions accept a string length from __len
local t = setmetatable({}, {__len = function() return "2" end})
table.insert(t, "x")
local u = setmetatable({1, 2, 3}, {__len = function() return " 0x2 " end})
local v = setmetatable({"a", "b", "c"}, {__len = function() return "2.0" end})
return rawget(t, 3), table.concat(u, ","), select("#", table.unpack(v)), table.remove(v), E(table.insert, setmetatable({}, {__len = function() return "2.5" end}), 1), E(table.insert, setmetatable({}, {__len = function() return "x" end}), 1), #t
-- case: hex float rounding with many digits
local r = {}
for _, s in ipairs{"0x1.00000000000008", "0x1.00000000000018", "0x1.fffffffffffff8", "0x1.fffffffffffff7ffffffff", "0x10000000000000800000000001", "0x1p-1074", "0x1p-1075", "0x1.0000000001p-1075", "0x1.8p-1074", "0x1.4p-1074", "0x0.0000000000001p-1022", "0x0.00000000000008p-1022", "0x0.000000000000080000001p-1022", "0x1.fffffffffffffp1023", "0x1.fffffffffffff8p1023", "0x1.fffffffffffff7fp1023", "0x123456789abcdef0123p-80", "0x.0000000000000000000000001p0"} do
  r[#r+1] = string.format("%.17g", tonumber(s))
end
local f = load("return 0x1.00000000000008, 0x1.0000000001p-1075")
r[#r+1] = string.format("%.17g %.17g", f())
return table.concat(r, " ")
