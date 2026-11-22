-- case: ephemeron table
local wk = setmetatable({}, {__mode = "k"})
do local k = {} wk[k] = {k} end
local keep = {}
wk[keep] = {keep}
collectgarbage(); collectgarbage()
local n = 0
for k in pairs(wk) do n = n + 1 end
return n, wk[keep][1] == keep
-- case: weak kv and functions and coroutines
local w = setmetatable({}, {__mode = "kv"})
w[1] = function() end
w[2] = coroutine.create(print)
w[{}] = 1
w.s = "x" .. "y"
w[3] = 3.5
collectgarbage(); collectgarbage()
local n = 0
for k in pairs(w) do n = n + 1 end
return n, w[1], w[2], w.s, w[3]
-- case: changing mode after creation
local mt = {}
local t = setmetatable({}, mt)
t[1] = {}
mt.__mode = "v"
collectgarbage(); collectgarbage()
return t[1]
-- case: weak values with upvalue reachable objects
local w = setmetatable({}, {__mode = "v"})
local obj = {}
local f = function() return obj end
w[1] = obj
obj = nil
collectgarbage()
local alive = w[1] ~= nil
f = nil
collectgarbage(); collectgarbage()
return alive, w[1]
-- case: collectgarbage count changes with allocation
local before = collectgarbage("count")
local t = {}
for i = 1, 10000 do t[i] = {i} end
local mid = collectgarbage("count")
t = nil
collectgarbage(); collectgarbage()
local after = collectgarbage("count")
return mid > before, after < mid
-- case: coroutine running in nested and isyieldable in pcall
local r = {}
local co = coroutine.create(function()
  r[#r+1] = tostring(coroutine.isyieldable())
  pcall(function() r[#r+1] = tostring(coroutine.isyieldable()) end)
  local inner = coroutine.wrap(function() r[#r+1] = tostring(coroutine.isyieldable()) end)
  inner()
  table.sort({2, 1}, function(a, b) r[#r+1] = tostring(coroutine.isyieldable()) return a < b end)
end)
coroutine.resume(co)
return table.concat(r, ",")
-- case: yield from within for iterator
local co = coroutine.wrap(function()
  local function it(_, i) if i < 3 then coroutine.yield("it" .. i) return i + 1 end end
  local s = 0
  for i in it, nil, 0 do s = s + i end
  return "sum" .. s
end)
return co(), co(), co(), co()
-- case: yield across concat and arith metamethods
local mt = {__concat = function(a, b) return coroutine.yield("cat") end, __add = function() return coroutine.yield("add") end, __eq = function() return coroutine.yield("eq") end, __lt = function() return coroutine.yield("lt") end, __len = function() return coroutine.yield("len") end, __newindex = function(t, k, v) coroutine.yield("ni") rawset(t, k, v) end, __call = function() return coroutine.yield("call") end}
local co = coroutine.wrap(function()
  local a, b = setmetatable({}, mt), setmetatable({}, mt)
  local r = {}
  r[#r+1] = a .. "x"
  r[#r+1] = a + 1
  r[#r+1] = tostring(a == b)
  r[#r+1] = tostring(a < b)
  r[#r+1] = #a
  a.k = 1
  r[#r+1] = a()
  return table.concat(r, ",") .. rawget(a, "k")
end)
local out = {}
local v = co()
while true do
  out[#out+1] = v
  if v:find(",") then break end
  v = co(v == "eq" and true or v == "lt" and false or v .. "!")
end
return table.concat(out, " ")
-- case: resume a coroutine from inside itself via wrap
local w
w = coroutine.wrap(function() return pcall(w) end)
return w()
-- case: coroutine close then resume
local co = coroutine.create(function() coroutine.yield() end)
coroutine.resume(co)
coroutine.close(co)
return coroutine.status(co), coroutine.resume(co)
-- case: many coroutines status transitions
local log = {}
local a, b
a = coroutine.create(function()
  log[#log+1] = coroutine.status(a)
  coroutine.resume(b)
  log[#log+1] = "a-back"
end)
b = coroutine.create(function()
  log[#log+1] = coroutine.status(a)
  log[#log+1] = coroutine.status(b)
  log[#log+1] = tostring(coroutine.resume(a))
end)
coroutine.resume(a)
log[#log+1] = coroutine.status(a)
log[#log+1] = coroutine.status(b)
return table.concat(log, ",")
-- case: error values through resume keep type
local r = {}
for _, v in ipairs{1, 2.5, true, "s"} do
  local co = coroutine.create(function() error(v) end)
  local ok, e = coroutine.resume(co)
  r[#r+1] = type(e) .. ":" .. tostring(e)
end
local co = coroutine.create(function() error() end)
local ok, e = coroutine.resume(co)
r[#r+1] = tostring(e)
return table.concat(r, ",")
-- case: wrap error with non string keeps value
local w = coroutine.wrap(function() error(42) end)
local ok, e = pcall(w)
local w2 = coroutine.wrap(function() local t = nil; return t.x end)
local ok2, e2 = pcall(w2)
return math.type(e), e2
-- case: xpcall handler in coroutine and message handler gets original
local co = coroutine.wrap(function()
  return xpcall(function() coroutine.yield("y") error("E", 0) end, function(m) return "H" .. m end)
end)
return co(), co()
-- case: pcall returns after yield inside
local co = coroutine.wrap(function()
  local a, b, c = pcall(function() local x = coroutine.yield(1) return x, x * 2 end)
  return a, b, c
end)
return co(), co(21)
