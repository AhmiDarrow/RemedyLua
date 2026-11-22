-- case: int float division and modulo
local r = {}
for _, a in ipairs{7, -7, 0, math.maxinteger, math.mininteger, 7.5, -7.5, 0.0, -0.0, 1/0, -1/0} do
  for _, b in ipairs{2, -2, 3.0, -3.0, 1/0, -1/0, 0.5} do
    r[#r+1] = tostring(a // b) .. "," .. tostring(a % b)
  end
end
return table.concat(r, " ")
-- case: int division by zero errors
return E(function() return 1 // 0 end), E(function() return 1 % 0 end), 1 // 0.0, -1 // 0.0, 1 % 0.0 ~= 1 % 0.0, 0/0 ~= 0/0
-- case: mininteger // -1 and % -1
return math.mininteger // -1, math.mininteger % -1, math.mininteger * -1, -math.mininteger
-- case: float to int conversions in bitwise
return E(function() return 1.5 | 0 end), E(function() return 2^63 | 0 end), (-2^63) | 0, 3.0 | 0, "3" | 0, E(function() return "3.5" | 0 end), E(function() return "x" | 0 end), "0x10" | 0, " 10 " & 15
-- case: string arithmetic
return "10" + 1, "10" + 1.0, "0x10" + 0, "1e2" * 1, " 5 " - 1, E(function() return "abc" + 1 end), "10" // "3", "7" % "2", -"2", -"2.0", "2" ^ "3", math.type("10" + 1), math.type("10.0" + 1)
-- case: string arith overflow
return "9223372036854775807" + 1, "9223372036854775808" + 0, math.type("9223372036854775808" + 0), "-9223372036854775808" + 0
-- case: comparisons int float
return math.maxinteger < 2^63, math.maxinteger + 0.0 == 2^63, math.maxinteger == 2^63, math.mininteger == -2^63, (2^53 + 1) == 2^53 + 1, 9007199254740993 < 9007199254740992.0, 9007199254740993 > 9007199254740992.0, 1 < 0/0, 1 > 0/0, math.mininteger <= -1/0, math.maxinteger < 1/0
-- case: shifts
return 1 << 63, 1 << 64, 1 >> -1, -1 >> 1, -1 >> 63, -1 >> 64, 1 << -64, 5 >> 1.0, ~0, ~5.0
-- case: pow and unm
return 2^2, math.type(2^2), (-2)^0.5 ~= (-2)^0.5, 0^0, -2^2, 2^-1, 2^3^2
-- case: tostring floats
return tostring(1e15), tostring(1e16), tostring(-0.0), tostring(1e100), tostring(0.1), tostring(123456789012345678), tostring(2^63), tostring(3.0), tostring(-3.0), tostring(1/3), tostring(1e-5), tostring(100.0)
-- case: integer literals
return 0xffffffffffffffff, 0x7fffffffffffffff, 9223372036854775807, 9223372036854775808, math.type(9223372036854775808), 0x1p4, 0x.8, 0xA.8p1, 1e1, .5, 5., 3e-2
-- case: integer for loop edges
local n = 0
for i = math.maxinteger - 2, math.maxinteger do n = n + 1 end
for i = math.mininteger, math.mininteger + 2 do n = n + 1 end
for i = 1, 0 do n = n + 100 end
for i = 3, 1, -1 do n = n + 1 end
for i = 1, 3.5 do n = n + 1 end
for i = 1.0, 2 do n = n + 1; assert(math.type(i) == "float") end
for i = 1, math.huge do n = n + 1; if i > 3 then break end end
for i = 1, -math.huge, -1 do n = n + 1; if i < -3 then break end end
return n
-- case: for loop errors
return E(function() for i = 1, 10, 0 do end end), E(function() for i = "a", 2 do end end), E(function() for i = 1, {} do end end), E(function() for i = 1, 2, nil do end end), E(function() for i = 1.0, 2, 0.0 do end end)
-- case: for loop float limit with int
local c = 0
for i = 1, 2.9 do c = c + 1 end
for i = 1, -0.5, -1 do c = c + 10 end
for i = math.maxinteger, 1e100 do c = c + 100 break end
for i = 1, math.mininteger * 2.0 do c = c + 1000 end
return c
-- case: float equality keys
local t = {}
t[1.0] = "a"; t[2^53] = "b"; t[-0.0] = "z"
return t[1], math.type(next({[3.0]=1})), t[2^53|0], t[0]
-- case: math.tointeger and floor
return math.tointeger(3.0), math.tointeger(3.5), math.tointeger("8"), math.tointeger(2^63), math.floor(-3.5), math.ceil(-3.5), math.floor(2^62), math.type(math.floor(1e100)), math.floor(-0.0), math.ceil(-0.5)
-- case: math functions
return math.abs(math.mininteger), math.max(1, 2.0), math.min(1.0, 1), math.fmod(7, 3), math.fmod(-7, 3), math.fmod(7, -3.0), E(math.fmod, 1, 0), math.fmod(1, 0.0) ~= math.fmod(1, 0.0), math.fmod(math.mininteger, -1), math.ult(1, -1), math.modf(3.7), math.modf(-3.7), math.modf(1/0), math.huge, -math.huge, math.pi
-- case: math more
return math.sqrt(16), math.exp(0), math.log(8, 2), math.log(100, 10), math.log(1), math.type(math.abs(-3)), math.abs(-0.0), E(math.max), E(math.floor, "x"), math.floor("3.7"), math.modf(5), math.type(math.modf(5))
-- case: tonumber bases
return tonumber("10", 2), tonumber("ff", 16), tonumber("FF", 16), tonumber("zz", 36), tonumber(" 11 ", 2), tonumber("12", 2), tonumber("-ff", 16), tonumber("", 10), tonumber("8", 8), E(tonumber, "1", 1), E(tonumber, "1", 37), E(tonumber, 1, 10), tonumber("7fffffffffffffff", 16), tonumber("ffffffffffffffffff", 16)
-- case: tonumber strings
return tonumber("0x"), tonumber("1e"), tonumber(" 0x1p-2 "), tonumber("inf"), tonumber("nan"), tonumber("1 2"), tonumber("1\0"), tonumber(""), tonumber("  "), tonumber("0x1.8"), tonumber(".e1"), tonumber("5", nil), E(tonumber), tonumber(nil)
-- case: string to number edge
return "0x7fffffffffffffff" + 0, "0xffffffffffffffff" + 0, "1" == 1, 10 .. "", 1.5 .. "", -0.0 .. "", 2^63 .. ""
-- case: math.tointeger of strings and non-numbers
return math.tointeger("3"), math.tointeger({}), math.tointeger("x"), math.tointeger(-0.0), E(math.tointeger)
-- case: integer overflow wrap
return math.maxinteger + 1 == math.mininteger, math.mininteger - 1, math.maxinteger * 2, 5 // -2, -5 // 2, 5 % -2, -5 % 2, 5.0 % -2, -5 % 2.0, 3 % math.mininteger, -3 % math.mininteger, 3 % math.maxinteger
-- case: float modulo of infinity and small
return 5.5 % math.huge, -5.5 % math.huge, 5.5 % -math.huge, -5.5 % -math.huge, math.huge % 2 ~= math.huge % 2, 1e308 % 1e-308
