-- case: string length ignores __len of string metatable
local smt = getmetatable("")
smt.__len = function() return 99 end
local n = #"abc"
smt.__len = nil
return n
-- case: string metatable __index replaced by function
local smt = getmetatable("")
local old = smt.__index
smt.__index = function(s, k) return k .. "!" end
local r = ("abc").foo
smt.__index = old
return r, ("x"):upper()
-- case: coroutine.wrap error closes pending tbc
local log = {}
local f = coroutine.wrap(function()
  local x <close> = setmetatable({}, {__close = function(_, e) log[#log + 1] = "closed:" .. tostring(e) end})
  coroutine.yield(1)
  error("boom", 0)
end)
f()
local ok, e = pcall(f)
return ok, e, log[1]
-- case: integer for loops with extreme steps
local r = {}
for i = math.mininteger, math.maxinteger, math.maxinteger do r[#r + 1] = i end
for i = math.maxinteger, math.mininteger, math.mininteger do r[#r + 1] = i end
for i = 0, math.maxinteger, 2^62 do r[#r + 1] = math.type(i) end
for i = -1, -3, -1 do r[#r + 1] = i end
return table.concat(r, " ")
-- case: for loop with float start and integer limit
local r = {}
for i = 0.5, 2 do r[#r + 1] = i end
for i = 3, 1.5, -0.5 do r[#r + 1] = i end
return table.concat(r, " ")
-- case: string keys and number keys are distinct
local t = {}
t["1"] = "s"; t[1] = "n"
return t["1"], t[1], #t
-- case: rawset rawget bypass and return
local t = setmetatable({}, {__newindex = function() error("no") end, __index = function() return "dflt" end})
local r = rawset(t, "k", 1)
return r == t, rawget(t, "k"), t.k, t.missing, rawget(t, "missing")
-- case: __index on table with false value present
local t = setmetatable({a = false}, {__index = function() return "meta" end})
return t.a, t.b
-- case: __newindex with nil assignment to absent key
local calls = 0
local t = setmetatable({}, {__newindex = function(t, k, v) calls = calls + 1 end})
t.x = nil
return calls
-- case: multiple returns through pcall and coroutine
local function f() return 1, nil, 3, nil end
local co = coroutine.wrap(function(...) coroutine.yield(...) return select("#", ...) end)
return select("#", pcall(f)), select("#", co(f())), co()
-- case: error rethrow keeps object identity
local e = {}
local ok, got = pcall(function() local ok2, e2 = pcall(error, e) error(e2) end)
return got == e
-- case: error with level pointing to caller of caller
local function a() error("deep", 3) end
local function b() a() end
local function c() b() end
return pcall(c)
-- case: tostring calls __tostring and __name nesting
local inner = setmetatable({}, {__tostring = function() return "in" end})
local outer = setmetatable({}, {__tostring = function() return tostring(inner) .. "+out" end})
return tostring(outer), string.format("%s", outer)
-- case: concat number formats in error-free paths
return 1 .. 2, 1.5 .. "|", (2^63) .. "", math.mininteger .. "", 1e15 .. "", -1e15 .. ""
-- case: integer division and modulo of mixed int float
return 7 // 2.0, 7 % 2.0, -7 // 2.0, -7 % 2.0, 7.0 // -2, 7.0 % -2, math.type(7 // 2.0)
-- case: comparison of int and float with huge values
return math.maxinteger < 2^63, math.maxinteger <= 2^63, math.mininteger <= -2^63, math.mininteger < -2^63, 2^63 > math.maxinteger, -2^63 >= math.mininteger, 1e308 > math.maxinteger, -1e308 < math.mininteger
-- case: equality int float huge
return math.maxinteger == 2^63, math.maxinteger == 2^63 - 1, math.mininteger == -2^63, 2^53 == (2^53 | 0), 2^53 + 1 == (2^53 | 0) + 1
-- case: string.format integer flags with float integral
return string.format("%5.1f|%-8.3e|%+g|%x|%d", 3, 4, 5, 6.0, -0.0)
-- case: gsub with pattern special at end
return ("a.b"):gsub("%.$", "!"), ("a$"):gsub("%$", "D"), ("x^"):gsub("%^", "C"), ("a.b"):gsub(".", "%0", 2)
-- case: find with captures returns positions and captures
return ("hello world"):find("(o)(r)"), ("hello"):find("l(l)()"), ("abc"):find("()")
-- case: gmatch empty matches with anchors and positions
local r = {}
for a, b in ("abc"):gmatch("()(.?)") do r[#r + 1] = a .. "=" .. b end
return table.concat(r, ",")
-- case: string.rep and sub with huge counts error before allocation
return pcall(string.rep, "ab", math.maxinteger // 2), pcall(string.rep, "a", math.maxinteger, ",")
-- case: table.concat huge range invalid value
return pcall(table.concat, {}, ",", 1, 3), pcall(table.concat, {1, 2}, ",", 2, 1)
-- case: select with string number index
return select("2", "a", "b"), pcall(select, "x", 1)
-- case: next on table with integer and float keys
local t = {[1] = "a", [2.5] = "b"}
local n = 0
for k, v in next, t do n = n + 1 end
return n
-- case: ipairs over string-keyed metatable index
local t = setmetatable({}, {__index = {"x", "y"}})
local r = {}
for i, v in ipairs(t) do r[#r + 1] = v end
return table.concat(r)
-- case: utf8.char boundary values
return #utf8.char(0x7F), #utf8.char(0x80), #utf8.char(0x7FF), #utf8.char(0x800), #utf8.char(0xFFFF), #utf8.char(0x10000), #utf8.char(0x1FFFFF), #utf8.char(0x200000), #utf8.char(0x3FFFFFF), #utf8.char(0x4000000)
-- case: utf8.offset negative from end and middle
local s = "a€b"
return utf8.offset(s, -1), utf8.offset(s, -2), utf8.offset(s, -3), utf8.offset(s, -4), utf8.offset(s, 1, -1), utf8.offset(s, 2, 2)
-- case: math.random(m) bounds integer conversion
return pcall(math.random, 1.0), pcall(math.random, "3"), pcall(math.random, 1, 2.0), pcall(math.random, 2^63)
-- case: math.abs and math.type with strings
return math.abs("-0"), math.type(math.abs("-0")), pcall(math.abs, "x"), math.max("1", 2), math.type(math.max("1", 2))
-- case: math.ceil floor of float min/max integer boundaries
return math.floor(2^62), math.ceil(-2^63), math.type(math.ceil(-2^63)), math.floor(-2^63 - 1025), math.type(math.floor(-2^63 - 1025)), math.ceil(2^63 - 512), math.type(math.ceil(2^63 - 512))
-- case: load chunk returns varargs and receives args
local f = load("return select('#', ...), ...")
return f(1, nil, 3)
-- case: load text with nul bytes inside strings
local f = load("return 'a\0b'")
return #f()
-- case: deeply nested functions upvalue chain
local function mk(n) if n == 0 then return function() return 0 end end local inner = mk(n - 1) return function() return inner() + 1 end end
return mk(100)()
-- case: closures in nested loops capture correctly
local fs = {}
for i = 1, 3 do for j = 1, 2 do fs[#fs + 1] = function() return i * 10 + j end end end
local r = {}
for _, f in ipairs(fs) do r[#r + 1] = f() end
return table.concat(r, ",")
-- case: vararg assignment beyond count and table.pack n
local function f(...) local a, b, c = ... return a, b, c, table.pack(...).n end
return f(1)
-- case: long string with level and leading newline variants
return [==[
]==], [[

]], #[[
a]]
-- case: comment forms at end of file without newline
return 1 --[==[ trailing ]==]
