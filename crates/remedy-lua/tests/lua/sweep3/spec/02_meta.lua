-- case: eq metamethod rules
local mt = {__eq = function(a, b) return "yes" end}
local a, b = setmetatable({}, mt), setmetatable({}, {})
local c = setmetatable({}, {__eq = function() return nil end})
return a == b, b == a, a ~= b, a == a, a == 1, c == a, a == c, rawequal(a, b)
-- case: lt le metamethods
local mt = {__lt = function(a, b) return 1 end, __le = function(a, b) return false end}
local a, b = setmetatable({}, mt), setmetatable({}, mt)
return a < b, a <= b, a > b, a >= b, E(function() return a < 1 end), E(function() return 1 < a end), "a" < "b", E(function() return {} < {} end), E(function() return 1 < "2" end), E(function() return {} <= {} end)
-- case: le without lt fallback
local a = setmetatable({}, {__lt = function() return true end})
return E(function() return a <= a end)
-- case: concat metamethod
local mt = {__concat = function(a, b) return (type(a) == "table" and "T" or tostring(a)) .. "+" .. (type(b) == "table" and "T" or tostring(b)) end}
local t = setmetatable({}, mt)
return t .. "x", "x" .. t, 1 .. t, t .. t, "a" .. "b" .. t, t .. "a" .. "b", E(function() return {} .. "x" end), E(function() return 1 .. nil end)
-- case: len metamethod
local t = setmetatable({1, 2, 3}, {__len = function() return "five" end})
return #t, rawlen(t), #"abc", E(function() return #5 end), E(rawlen, 5)
-- case: index chains
local base = {x = 1}
local mid = setmetatable({}, {__index = base})
local top = setmetatable({}, {__index = mid})
local f = setmetatable({}, {__index = function(t, k) return k .. "!" end})
local s = setmetatable({}, {__index = "abc"})
return top.x, top.y, f.q, f[1], E(function() return s.len end), ("x"):rep(3)
-- case: newindex chains
local store = {}
local p = setmetatable({}, {__newindex = store})
local q = setmetatable({}, {__newindex = p})
q.a = 1
local r = setmetatable({}, {__newindex = function(t, k, v) rawset(t, k, v * 2) end})
r.b = 5
local e = setmetatable({x = 1}, {__newindex = function() error("no") end})
e.x = 2
return store.a, rawget(q, "a"), rawget(p, "a"), r.b, e.x
-- case: index on nonindexable errors
local t = {}
return E(function() return t.a.b end), E(function() local x; return x.y end), E(function() return (1).x end), E(function() t.a.b = 1 end), E(function() return undefinedglobal.x end)
-- case: arith metamethods all
local function mm(name) return function(a, b) return name end end
local mt = {}
for _, n in ipairs{"add","sub","mul","div","mod","pow","unm","idiv","band","bor","bxor","shl","shr","bnot"} do mt["__" .. n] = mm(n) end
local t = setmetatable({}, mt)
return t + 1, 1 - t, t * t, t / 2, t % 2, t ^ 2, -t, t // 2, t & 1, 1 | t, t ~ 1, t << 1, t >> 1, ~t
-- case: bitwise on float strings metamethod
local t = setmetatable({}, {__band = function(a, b) return "band" end})
return E(function() return 1.5 & 1 end), E(function() return "1.5" & 1 end), t & 1.5, E(function() return {} & 1 end), E(function() return 2^64 & 1 end)
-- case: arith errors messages
local t = {}
local s = "x"
return E(function() return t + 1 end), E(function() return 1 + t.x end), E(function() return s + 1 end), E(function() return -t end), E(function() return t.y.z end), E(function() return #nil end), E(function() return ~{} end)
-- case: call metamethod
local c = setmetatable({}, {__call = function(self, a, b) return a, b end})
local cc = setmetatable({}, {__call = c})
return c(1, 2), select('#', c()), E(function() return cc(5) end), E(function() local x = {} x() end), E(function() local x = {} x.y() end), E(function() local x = {} x:z() end)
-- case: tostring and name
local t = setmetatable({}, {__tostring = function() return "TT" end})
local u = setmetatable({}, {__name = "MyType"})
local bad = setmetatable({}, {__tostring = function() return 1 end})
return tostring(t), tostring(u):match("^MyType: ") ~= nil, E(tostring, bad), E(function() return u + 1 end)
-- case: metatable field
local t = setmetatable({}, {__metatable = "locked"})
return getmetatable(t), E(setmetatable, t, {}), getmetatable("").__index == string, getmetatable("").__metatable
-- case: pairs metamethod
local t = setmetatable({}, {__pairs = function(t) return function(_, k) if not k then return 1, "a" end end, t, nil end})
local r = {}
for k, v in pairs(t) do r[#r+1] = k .. v end
return table.concat(r)
-- case: close basic
local log = {}
do
  local a <close> = setmetatable({}, {__close = function(o, e) log[#log+1] = "a" .. tostring(e) end})
  local b <close> = setmetatable({}, {__close = function(o, e) log[#log+1] = "b" .. tostring(e) end})
  local c <close> = nil
  local d <close> = false
end
local ok, err = pcall(function()
  local x <close> = setmetatable({}, {__close = function(o, e) log[#log+1] = "x" .. tostring(e) end})
  error("boom", 0)
end)
return table.concat(log, ","), err
-- case: close errors
local function mk(n, fail) return setmetatable({}, {__close = function(o, e) if fail then error(n .. ":" .. tostring(e), 0) end end}) end
local ok, err = pcall(function()
  local a <close> = mk("a", true)
  local b <close> = mk("b", true)
end)
local ok2, err2 = pcall(function() local x <close> = 42 end)
local ok3, err3 = pcall(function() local x <close> = {} end)
return err, err2, err3
-- case: close in loops and return values
local log = {}
local function f()
  local a <close> = setmetatable({}, {__close = function() log[#log+1] = "closed" end})
  return "r1", "r2"
end
local x, y = f()
for i = 1, 3 do
  local z <close> = setmetatable({}, {__close = function() log[#log+1] = i end})
  if i == 2 then break end
end
return x, y, table.concat(log, ",")
-- case: close const assign error
return L("local x <close> = nil; x = 1"), L("local x <const> = 1; x = 2"), L("local x <foo> = 1"), L("local a <close>, b <close> = 1, 2")
-- case: close with goto
local log = {}
do
  local i = 0
  ::top::
  do
    local c <close> = setmetatable({}, {__close = function() log[#log+1] = i end})
    i = i + 1
    if i < 3 then goto top end
  end
end
return table.concat(log, ",")
-- case: close in generic for
local log = {}
local closer = setmetatable({}, {__close = function() log[#log+1] = "c" end})
for i in function(s, c) if c < 3 then return c + 1 end end, nil, 0, closer do log[#log+1] = i end
for i in function(s, c) if c < 3 then return c + 1 end end, nil, 0, closer do if i == 2 then break end end
return table.concat(log, ",")
-- case: unm and len of string number
return -"3", -" 0x10 ", #"\0\0"
-- case: index number errors
return E(function() local n = 5 return n.x end), E(function() return ("x").y.z end)
-- case: __lt with mixed types
local mt = {__lt = function(a, b) return true end, __le = function() return true end}
local t = setmetatable({}, mt)
return t < 1, 1 < t, "x" <= t, t >= 2
-- case: __eq only for same types
local mt = {__eq = function() return true end}
local t = setmetatable({}, mt)
return t == "x", t == 1, t == setmetatable({}, {})
-- case: __newindex on missing vs existing
local log = {}
local t = setmetatable({a = 1}, {__newindex = function(t, k, v) log[#log+1] = k end})
t.a = 2; t.b = 3; t.a = nil; t.a = 4
return table.concat(log, ","), t.a, rawget(t, "b")
-- case: __index with float keys
local t = setmetatable({}, {__index = function(t, k) return math.type(k) end})
return t[1.0], t[1.5], t[2^63]
-- case: close value in coroutine close
local log = {}
local co = coroutine.create(function()
  local a <close> = setmetatable({}, {__close = function(_, e) log[#log+1] = tostring(e) end})
  coroutine.yield(1)
end)
coroutine.resume(co)
local ok, e = coroutine.close(co)
return ok, e, table.concat(log), coroutine.status(co)
-- case: close error replaces return
local ok, e = pcall(function()
  local a <close> = setmetatable({}, {__close = function() error("in close", 0) end})
  return 1
end)
return ok, e
-- case: tbc in tail position is not a tail call
local log = {}
local function g() log[#log+1] = "g" return "gv" end
local function f()
  local a <close> = setmetatable({}, {__close = function() log[#log+1] = "close" end})
  return g()
end
local v = f()
return v, table.concat(log, ",")
-- case: unary minus constant folding
return - -1, -(-math.mininteger), - - -2.5, not nil == true
-- case: calling a non-callable metamethod names the event
local r = {}
for _, ev in ipairs{"add", "sub", "mul", "div", "mod", "pow", "unm", "idiv", "band", "bor", "bxor", "shl", "shr", "bnot", "concat", "len", "eq", "lt", "le", "call"} do
  local mt = {}
  mt["__" .. ev] = true
  local a, b = setmetatable({}, mt), setmetatable({}, mt)
  local ops = {add = "+", sub = "-", mul = "*", div = "/", mod = "%", pow = "^", idiv = "//", band = "&", bor = "|", bxor = "~", shl = "<<", shr = ">>"}
  local f
  if ops[ev] then f = load("local a, b = ... return a " .. ops[ev] .. " b", "=x")
  elseif ev == "unm" then f = function(a) return -a end
  elseif ev == "bnot" then f = function(a) return ~a end
  elseif ev == "len" then f = function(a) return #a end
  elseif ev == "concat" then f = function(a, b) return a .. b end
  elseif ev == "eq" then f = function(a, b) return a == b end
  elseif ev == "lt" then f = function(a, b) return a < b end
  elseif ev == "le" then f = function(a, b) return a <= b end
  else f = function(a) return a() end end
  r[#r+1] = select(2, pcall(f, a, b))
end
local c = setmetatable({}, {__add = setmetatable({}, {__call = function() return "callable" end})})
r[#r+1] = c + 1
r[#r+1] = select(2, pcall(function() return c .. setmetatable({}, {__concat = 5}) end))
return table.concat(r, "\n")
