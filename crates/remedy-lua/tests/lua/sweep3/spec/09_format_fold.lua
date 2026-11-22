-- case: format flags allowed per conversion
local r = {}
for _, f in ipairs{"%+x", "% x", "%#d", "%+s", "%0s", "%#c", "%.3c", "%+u", "%#u", "% u", "%05s", "%-5c|", "%-#10x|", "%+ d", "%#.3o", "%#10.3e", "%.3p", "%-10p|", "%05p", "%#s", "%5q", "%-q", "%.0s", "%.s", "%.d", "%#.g", "%00d", "%-0d", "%1$d"} do
  local ok, v = pcall(string.format, f, 65)
  if f:find("s") or f:find("q") then ok, v = pcall(string.format, f, "ab") end
  if f:find("p") then ok, v = pcall(string.format, f, 1) end
  r[#r+1] = f .. "=" .. tostring(v)
end
return table.concat(r, " ")
-- case: format p
local t, u = {}, {}
return string.format("%p", 1), string.format("%p", nil), string.format("%10p|", true), string.format("%-8p|", 2.5), string.format("%p", t) == string.format("%p", t), string.format("%p", t) ~= string.format("%p", u), tostring(t) == "table: " .. string.format("%p", t), tostring(print) == "builtin: " .. string.format("%p", print) or tostring(print) == "function: builtin: " .. string.format("%p", print) or tostring(print) == "function: " .. string.format("%p", print)
-- case: format p of strings and functions
local f = function() end
return tostring(f) == "function: " .. string.format("%p", f), string.format("%p", "abc") ~= "(null)", type(string.format("%p", coroutine.running()))
-- case: constant folding preserves negative zero and errors
local a <const> = -0.0
local b <const> = 2^63
return 1 / a, 1 / (0.0 * -1), 1 / -0.0, 1 / (-0.0 + 0.0), E(function() return b | 0 end), 7 // 0.0, -7 // 0.0, 0.0 / 0.0 ~= 0.0 / 0.0, 3 % -0.0 ~= 3 % -0.0, 1 / (0 * -1.0)
-- case: folding integer ops on constants
local x <const> = math.maxinteger
local y <const> = 10
return x + 1, y // 3, y % -3, -y // 3, y / 2, y ^ 2, y << 63, y >> 1, ~y, -x - 1, y & 6, y | 5, y ~ 3
-- case: folding strings and comparisons
local s <const> = "ab"
local n <const> = 3
return s .. n, #s, s == "ab", n < 4, "10" + n, - -n, not s
-- case: math with nan
local nan = 0/0
return math.max(nan, 1) ~= math.max(nan, 1), math.max(1, nan), math.min(nan, 1) ~= math.min(nan, 1), math.min(1, nan), math.tointeger(nan), E(string.format, "%d", nan), math.floor(nan) ~= math.floor(nan), math.type(math.floor(nan)), E(math.random, nan), math.fmod(nan, 1) ~= math.fmod(nan, 1), math.ult(0, 0)
-- case: math huge to integer
return math.floor(math.huge), math.ceil(-math.huge), math.type(math.floor(2^63)), math.floor(-2^63), math.type(math.floor(-2^63)), math.tointeger(-2^63)
-- case: pattern recursion depth
local s = ("a"):rep(300)
local ok, e = pcall(string.match, s, ("a?"):rep(300))
local ok2, e2 = pcall(string.match, s, ("a?"):rep(150) .. ("a"):rep(150))
return ok, ok and #e or e, ok2, ok2 and #e2 or e2
-- case: too many captures
return E(string.match, ("a"):rep(40), ("(a)"):rep(33)), select("#", string.match(("a"):rep(40), ("(a)"):rep(32)))
-- case: pattern classes all
local all = {}
for i = 0, 255 do all[#all+1] = string.char(i) end
all = table.concat(all)
local r = {}
for _, c in ipairs{"a", "c", "d", "g", "l", "p", "s", "u", "w", "x", "A", "C", "D", "G", "L", "P", "S", "U", "W", "X"} do
  local _, n = all:gsub("%" .. c, "")
  r[#r+1] = c .. n
end
return table.concat(r, " ")
-- case: pattern sets with classes and ranges
return ("a-z%"):gsub("[%a-]", "X"), ("a5-"):gsub("[%d-z]", "Y"), ("]x^"):gsub("[]^]", "Z"), ("abc"):gsub("[^%a]", "Q"), ("a%b"):gsub("[a-%%]", "R"), ("-"):gsub("[a-]", "S"), ("b"):gsub("[a-c-]", "T"), E(string.gsub, "x", "[a-", "")
-- case: pattern balanced and frontier edge
return ("(a(b)c)"):match("%b()"), ("((("):match("%b()"), ("x"):match("%bxx"), E(string.match, "x", "%b("), ("hello world"):gsub("%f[%w]%w+%f[%W]", "<%0>"), ("abc"):find("%f[%l]"), ("ABC"):find("%f[%l]"), ("a"):match("%f[%z]()")
-- case: gsub with pattern anchors and empty matches
return ("abc"):gsub(".-", "-"), ("abc"):gsub("x*", "-"), ("abc"):gsub("b*", "-"), (""):gsub("", "x"), (""):gsub("^$", "E"), ("aaa"):gsub("a-$", "!"), ("hello"):gsub("l+", function(m) return #m end)
-- case: gsub replacement escapes
return ("abc"):gsub("b", "%%1"), ("abc"):gsub("(b)", "[%1%1]"), ("abc"):gsub("b", "%0%0"), E(string.gsub, "abc", "b", "%9"), ("abc"):gsub("()b", "%1"), E(string.gsub, "abc", "b", "%x")
-- case: find returns captures and positions
return ("hello"):find("l(l)(o)"), ("hello"):find("()ll()"), ("key = value"):find("(%w+)%s*=%s*(%w+)"), ("x"):find("(x)", 1, true)
-- case: gmatch with captures and position empty
local r = {}
for a, b in ("k1=v1;k2=v2"):gmatch("(%w+)=(%w+)") do r[#r+1] = a .. b end
for e in ("a,,b"):gmatch("([^,]*)") do r[#r+1] = "[" .. e .. "]" end
return table.concat(r)
-- case: gmatch init negative
local r = {}
for w in ("one two three"):gmatch("%a+", -5) do r[#r+1] = w end
for w in ("abc"):gmatch(".", 10) do r[#r+1] = w end
return table.concat(r, ",")
-- case: string.format q roundtrip of all bytes
local all = {}
for i = 0, 255 do all[#all+1] = string.char(i) end
all = table.concat(all)
local q = string.format("%q", all)
return load("return " .. q)() == all, #q, q:sub(1, 60)
-- case: format q integers and floats
return string.format("%q", 0), string.format("%q", -1), string.format("%q", math.maxinteger), string.format("%q", 1e100), string.format("%q", -0.0), string.format("%q", 2^63), string.format("%q", 0.5)
-- case: tostring of integer floats beyond 2^63
return tostring(2^63), tostring(-2^63), tostring(2^64), tostring(1e19), 2^63 == math.maxinteger + 1.0, math.tointeger(2^62) == 2^62
-- case: string.len with embedded zeros and string.char roundtrip
local s = string.char(0, 1, 2, 0)
return #s, s:byte(1, -1)
-- case: string.rep with separators and huge counts error text
return ("a"):rep(3, ", "), E(string.rep, "a", 1e10, "b") ~= nil, (""):rep(1e8)
-- case: format u with zero precision
return string.format("%.u|%0.u|%18.u|%.0u|%.0d|%.u", 7, 7, 2^53, 0, 0, -1)
-- case: chunk names at the 45 byte limit
return L("x = = 1", ("a"):rep(44)), L("x = = 1", ("b"):rep(45)), L("x = = 1", ("c"):rep(46)), L("x = = 1", "ab\r\ncd"), L("x = = 1", "ab\rcd"), L("x = = 1", "=" .. ("d"):rep(59)), L("x = = 1", "=" .. ("e"):rep(60)), L("x = = 1", "@" .. ("f"):rep(59)), L("x = = 1", "@" .. ("g"):rep(60))
