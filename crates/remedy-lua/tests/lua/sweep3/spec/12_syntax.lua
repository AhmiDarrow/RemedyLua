-- case: line counting with all newline forms
local r = {}
for _, nl in ipairs{"\n", "\r", "\r\n", "\n\r", "\n\n", "\r\r", "\n\r\n", "\r\n\r"} do
  local f = load("local x = 1" .. nl .. nl .. "error('here')")
  r[#r+1] = select(2, pcall(f))
end
return table.concat(r, " ")
-- case: line counting in long strings and comments
local f = load("local s = [[\r\na\r\nb]] --[[\n\n]] error('x')")
local g = load("local s = 'a\\z\n\n   b' error('y')")
local h = load("local s = 'a\\\r\nb' error('z')")
return select(2, pcall(f)), select(2, pcall(g)), select(2, pcall(h))
-- case: chained call syntax
local log = {}
local o = {}
function o.a(x) log[#log+1] = "a" return o end
function o:b(x) log[#log+1] = "b" .. type(x) return self end
o.a "s" :b {1} :b "t" .a(1)
local t = {f = function(...) return select("#", ...) end}
return table.concat(log, ","), t.f{}, t.f"", t["f"](1, 2)
-- case: statements without separators
local a = 1 local b = 2 a, b = b, a local c = a + b;;; return a, b, c;
-- case: empty control blocks
local x = 0
if x then elseif x then else end
while false do end
repeat until true
for i = 1, 0 do end
do end
return x
-- case: comment at end without newline
return 1 --
-- case: label names and goto continue in plain lua
local r = 0
for i = 1, 5 do
  if i % 2 == 0 then goto continue end
  r = r + i
  ::continue::
end
return r
-- case: syntax messages for misc
return L("local 1 = 2"), L("local function 1() end"), L("function a.b:c.d() end"), L("x = function() end end"), L("return 1 +"), L("t = {1 2}"), L("t = {[1] 2}"), L("f(1,)"), L("local a <const> = 1, 2 a = 3"), L("goto 1"), L("::1::"), L("a.1 = 2"), L("a:b = 1"), L("x = not"), L("x = #"), L("(f)"), L("f()()"), L("a, b"), L("local t <close>")
-- case: syntax messages around strings and numbers
return L("x = 'a\\xg'"), L("x = '\\u{'"), L("x = '\\u{110000'"), L("x = '\\256'"), L("x = 1e"), L("x = 0x1p"), L("x = 3.."), L("x = 1..2"), L("x = 'abc"), L("x = \"abc\ndef\""), L("x = [=["), L("--[=[ x"), L("x = [=x")
-- case: deep but reasonable nesting matches
local s = "return " .. ("("):rep(100) .. "1" .. (")"):rep(100)
local t = "return " .. ("{"):rep(100) .. ("}"):rep(100)
local u = ("do "):rep(100) .. ("end "):rep(100) .. "return 2"
return load(s)(), type(load(t)()), load(u)()
-- case: operator precedence table exhaustive
return 1 + 2 .. 3, 2 ^ 2 ^ 3, -2 ^ -2, not true == false, 1 < 2 and 2 < 3, 1 | 2 ~ 3 & 4, 1 << 2 >> 1, "a" .. "b" == "ab", 7 // 2 * 2, 2 * 3 % 4, -3 % 5, #"abc" * 2, - - 2, ~ ~ 5, not not nil
-- case: comparison operators associativity
return 1 < 2 == true, "x" == "x" ~= false, (1 < 2) == (2 < 3)
-- case: varargs adjusted in different contexts
local function f(...) return ... end
local function g(...)
  local t = {...}
  local a, b, c = ...
  local x = (...)
  return #t, a, b, c, x, select("#", f(...)), select("#", (f(...)))
end
return g(1, 2, 3)
-- case: function definitions forms
local M = {sub = {}}
function M.sub.f(a) return a end
function M.sub:g(a) return self == M.sub, a end
local function h(...) return select("#", ...) end
local k = function(a, b, ...) return a, b, ... end
return M.sub.f(1), M.sub:g(2), h(nil, nil), k(1)
-- case: numeric literals compared to C
return 0xA.8p0, 0x.Fp4, 1e-0, 0E0, 0x10P-1, 123., .123e3, 0x1234567890abcdef, 0xFFFFFFFFFFFFFFFFF, 1e309, 2^1023 * 2, 0x7FFFFFFFFFFFFFFF + 1
-- case: integer division and modulo of constants in source
return 7 // 2, -7 // 2, 7 // -2, -7 // -2, 7 % 2, -7 % 2, 7 % -2, -7 % -2, 7.0 // 2, -7.0 % 2, 7 % 2.5, -7 % -2.5, 0 % 5, 0.0 % -5
-- case: string escapes decimal edge
return "\0651", "\9xyz", #"\00012", "\x41\x42", "\u{41}\u{0042}", "\u{10FFFF}" == "\xF4\x8F\xBF\xBF", "\u{7FFFFFFF}" == "\xFD\xBF\xBF\xBF\xBF\xBF"
-- case: method definitions with dots and colons errors
return L("function a:b:c() end"), L("function a.b.c:d() end"), L("local function a.b() end"), L("function () end")
-- case: assignments to function call results
return L("f() = 1"), L("f().x = 1"), L("f():g().y = 1"), L("(f).x = 1"), L("(1).x = 1"), L("('s'):x().y = 1")
-- case: attribs syntax
return L("local x <const>, y <close> = 1, nil"), L("local x < const > = 1"), L("local x <const> y = 1"), L("local <const> x = 1"), L("local function f <const>() end")
-- case: local _ENV and global names
local function f() local _ENV = {x = 5} return x end
local function g() local _ENV = {} return y.z end
local function h() local _ENV = {print = 1} return print() end
local env = {}
local function k() local _ENV <const> = env; q = 1 end
k()
return f(), E(g), E(h), env.q, E(function() local _ENV = nil; return w end), E(function() _ENV = nil; return w end)
-- case: format c wraps and conversions
return string.format("%c", 256) == "\0", string.format("%c", -1) == "\255", string.format("%c", "65"), string.format("%d", "  0x10  "), string.format("%f", "1e1"), string.format("%x", -0.0), string.format("%5.1f|%-5d|%5s", "2.25", "3", 4), table.concat({1, 2}, 3), ("x"):rep(2, 0), utf8.char("65"), utf8.codepoint("ab", 1, 2, "lax")
