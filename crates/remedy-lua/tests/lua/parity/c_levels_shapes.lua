-- Call shapes for C-level and error-level parity (tests/parity_lua54.rs): each letter wraps
-- the function below it; the innermost is `runaway`, whose message handler keeps failing until
-- "C stack overflow", so its count is C Lua's count of C levels in use. SEED and ITERS are
-- substituted by the test.
local function runaway()
  local seen = 0
  local res = {xpcall(error, function(m) seen = seen + 1 if m == "C stack overflow" then return "ov" .. seen end error(m, 0) end, "first")}
  return res[2], seen
end
local W = {
  P = function(inner) return function() return pcall(inner) end end,
  Q = function(inner) return function() local r = table.pack(pcall(inner)) return table.unpack(r, 1, r.n) end end,
  X = function(inner) return function() return xpcall(inner, print) end end,
  D = function(inner) return function() return pcall(pcall, inner) end end,
  T = function(inner) return function() return inner() end end,
  N = function(inner) return function() local r = table.pack(inner()) return table.unpack(r, 1, r.n) end end,
  Y = function(inner) return function() coroutine.yield() return inner() end end,
  Z = function(inner) return function() coroutine.yield() local r = table.pack(inner()) return table.unpack(r, 1, r.n) end end,
  I = function(inner) return function() local t = setmetatable({}, {__index = function() return inner() end}) return t.x end end,
  C = function(inner) return function() local c <close> = setmetatable({}, {__close = function() end}) return inner() end end,
  K = function(inner) return function() local c <close> = setmetatable({}, {__close = function() coroutine.yield() end}) local r = table.pack(inner()) return table.unpack(r, 1, r.n) end end,
  R = function(inner) return function() return coroutine.wrap(inner)() end end,
  S = function(inner) return function() local co = coroutine.create(inner) local r repeat r = table.pack(coroutine.resume(co)) until coroutine.status(co) == "dead" return table.unpack(r, 1, r.n) end end,
  A = function(inner) return function() return pcall(coroutine.wrap(inner)) end end,
  B = function(inner) return function() local co = coroutine.wrap(function() coroutine.yield() return inner() end) co() return co() end end,
  M = function(inner) return function() return setmetatable({}, {__call = function() return inner() end})() end end,
  F = function(inner) return function() return pcall(setmetatable({}, {__call = function() return inner() end})) end end,
  V = function(inner) return function() return pcall(pairs, setmetatable({}, {__pairs = function() coroutine.yield() return inner() end})) end end,
  U = function(inner) return function() return pcall(pairs, setmetatable({}, {__pairs = function() return inner() end})) end end,
  L = function(inner) return function() local r = 1 .. setmetatable({}, {__concat = function() return (inner()) end}) return r end end,
  O = function(inner) return function() return xpcall(pcall, print, inner) end end,
  H = function(inner) return function() coroutine.yield() return pcall(inner) end end,
  J = function(inner) return function() return pcall(function() coroutine.yield() return inner() end) end end,
  a = function(inner) return function() local o = setmetatable({}, {__call = function() return inner() end}) local t = setmetatable({}, {__add = pcall}) return o + t end end,
  b = function(inner) return function() local t = setmetatable({}, {__add = pcall}) return pcall + t end end,
  c = function(inner) return function() local o = setmetatable({}, {__call = function() return inner() end}) local t = setmetatable({}, {__lt = pcall}) if o < t then return "lt" end return "nlt" end end,
  d = function(inner) return function() local o = setmetatable({}, {__call = function() return inner() end}) local t = setmetatable({}, {__concat = pcall}) return "x" .. (o .. t) end end,
  e = function(inner) return function() local p = setmetatable({}, {__pairs = function() return inner() end}) local t = setmetatable({}, {__index = pairs}) return p[t] end end,
  f = function(inner) return function() local t = setmetatable({}, {__add = pcall, __pairs = function() return inner() end}) return pairs + t end end,
  g = function(inner) return function() return pcall(pcall, pairs, setmetatable({}, {__pairs = function() return inner() end})) end end,
  h = function(inner) return function() return xpcall(pcall, print, pairs, setmetatable({}, {__pairs = function() coroutine.yield() return inner() end})) end end,
  i = function(inner) return function() local o = setmetatable({}, {__call = function() coroutine.yield() return inner() end}) local t = setmetatable({}, {__add = pcall}) return pcall + (o + t) end end,
  j = function(inner) return function() local o = setmetatable({}, {__call = function() return inner() end}) local t = setmetatable({}, {__eq = pcall}) return o == t end end,
  k = function(inner) return function() local o = setmetatable({}, {__call = function() return inner() end}) local t = setmetatable({}, {__newindex = pcall}) t[1] = 2 return "ni" end end,
  l = function(inner) return function() local w = coroutine.wrap(function() return inner() end) local t = setmetatable({}, {__add = pcall}) return w + t end end,
  m = function(inner) return function() local t = setmetatable({}, {__index = pcall}) local o = setmetatable({}, {__call = function() return inner() end}) return o[t] end end,
  E = function(inner) return function() return select(1, pcall(inner)) end end,
}
local keys = {} for k in pairs(W) do keys[#keys + 1] = k end table.sort(keys)
local seed = SEED
local function rnd(n) seed = (seed * 1103515245 + 12345) % 2147483648 return seed % n + 1 end
local seen, lines = {}, {}
for iter = 1, ITERS do
  local name = ""
  for i = 1, rnd(5) do name = name .. keys[rnd(#keys)] end
  if not seen[name] then
    seen[name] = true
    local f = runaway
    for i = #name, 1, -1 do f = W[name:sub(i, i)](f) end
    local co = coroutine.create(f)
    local outs = {}
    for step = 1, 20 do
      local r = table.pack(coroutine.resume(co))
      local t = {} for i = 1, r.n do t[i] = (tostring(r[i]):gsub("0x%x+", "")) end
      outs[#outs + 1] = table.concat(t, ",")
      if coroutine.status(co) == "dead" then break end
    end
    lines[#lines + 1] = name .. "\t" .. table.concat(outs, " | ")
  end
end
return table.concat(lines, "\n")
