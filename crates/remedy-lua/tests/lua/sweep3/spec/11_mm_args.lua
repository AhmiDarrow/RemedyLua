-- case: metamethod argument counts
local r = {}
local function rec(name) return function(...) r[#r+1] = name .. select("#", ...) .. tostring(rawequal(select(1, ...), select(2, ...))) return 1 end end
local mt = {__unm = rec("unm"), __bnot = rec("bnot"), __len = rec("len"), __add = rec("add"), __index = rec("index"), __newindex = rec("newindex"), __call = rec("call"), __lt = rec("lt"), __le = rec("le"), __eq = rec("eq"), __concat = rec("concat"), __close = rec("close")}
local a = setmetatable({}, mt)
local b = setmetatable({}, mt)
local _ = -a, ~a, #a, a + 1, a.x
a.y = 1
a(1, 2, 3)
local _ = a < b, a <= b, a == b, a .. "s"
do local c <close> = a end
return table.concat(r, " ")
-- case: eq uses second operand metamethod
local mt = {__eq = function() return true end}
local a = {}
local b = setmetatable({}, mt)
return a == b, b == a
-- case: wrap closes tbc on error
local log = {}
local w = coroutine.wrap(function()
  local x <close> = setmetatable({}, {__close = function(_, e) log[#log+1] = "closed:" .. tostring(e) end})
  coroutine.yield(1)
  error("boom")
end)
w()
local ok, e = pcall(w)
return ok, e, table.concat(log, ",")
-- case: wrap error replaced by close error
local w = coroutine.wrap(function()
  local x <close> = setmetatable({}, {__close = function() error("closeerr", 0) end})
  error("body", 0)
end)
return pcall(w)
-- case: coroutine close with two closers erroring
local log = {}
local co = coroutine.create(function()
  local a <close> = setmetatable({}, {__close = function(_, e) log[#log+1] = "a:" .. tostring(e) error("ea", 0) end})
  local b <close> = setmetatable({}, {__close = function(_, e) log[#log+1] = "b:" .. tostring(e) error("eb", 0) end})
  coroutine.yield()
end)
coroutine.resume(co)
local ok, e = coroutine.close(co)
return ok, e, table.concat(log, ",")
-- case: close error during normal exit propagates
local log = {}
local ok, e = pcall(function()
  local a <close> = setmetatable({}, {__close = function(_, e) log[#log+1] = "a:" .. tostring(e) end})
  local b <close> = setmetatable({}, {__close = function(_, e) error("eb", 0) end})
  return "ret"
end)
return ok, e, table.concat(log, ",")
-- case: pcall inside close
local ok, e = pcall(function()
  local a <close> = setmetatable({}, {__close = function(_, e) local ok2, e2 = pcall(error, "inner") end})
  error("outer", 0)
end)
return ok, e
-- case: close value checked at declaration
local mt = {__close = function() end}
local t = setmetatable({}, mt)
local ok, e = pcall(function() local x <close> = setmetatable({}, {}) end)
local ok2, e2 = pcall(function() local y <close> = "str" end)
return e, e2
-- case: error position inside close called from return
local ok, e = pcall(function()
  local a <close> = setmetatable({}, {__close = function() local z = nil; return z.f end})
  return 1
end)
return e
-- case: coroutine resume passing to yield in nested calls
local function deep(n) if n == 0 then return coroutine.yield("bottom") end return deep(n - 1) end
local co = coroutine.create(function() return deep(50) end)
local a, b = coroutine.resume(co)
local c, d = coroutine.resume(co, "back")
return a, b, c, d
-- case: coroutine wrap in generic for with state
local function range(n) return coroutine.wrap(function() for i = 1, n do coroutine.yield(i, i * i) end end) end
local s = 0
for i, sq in range(5) do s = s + sq end
return s
-- case: status inside wrap and of wrapped through running
local st
local co
co = coroutine.create(function() st = coroutine.status(co) coroutine.yield() end)
coroutine.resume(co)
return st, coroutine.status(co)
-- case: resume with many values and yield returns
local co = coroutine.create(function(...) local n = select("#", ...) local r = table.pack(coroutine.yield(n, ...)) return r.n end)
local r1 = table.pack(coroutine.resume(co, 1, nil, 3, nil))
local r2 = table.pack(coroutine.resume(co, nil, nil))
return r1.n, r1[2], r2[2]
-- case: error in coroutine with level 2
local co = coroutine.create(function() local function f() error("lvl", 2) end f() end)
return coroutine.resume(co)
-- case: integer for loop var is a copy
local r = {}
for i = 1, 3 do
  r[#r+1] = function() return i end
  i = 100
end
return r[1](), r[2](), r[3]()
-- case: numeric for with large float step sign
local n = 0
for i = 1, 0, -0.0 do n = n + 1 if n > 3 then break end end
return n, E(function() for i = 1, 2, 0 do end end)
