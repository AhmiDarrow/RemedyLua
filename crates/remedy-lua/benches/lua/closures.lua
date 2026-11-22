-- Closure creation, upvalue reads/writes, higher-order calls.
local function counter()
  local n = 0
  return function(k) n = n + k; return n end
end
local function compose(f, g) return function(x) return f(g(x)) end end
local total = 0
for i = 1, 200000 do
  local c = counter()
  c(i)
  total = total + c(1)
  local h = compose(function(x) return x + 1 end, function(x) return x * 2 end)
  total = total + h(i)
end
local acc = counter()
for _ = 1, 1000000 do acc(1) end
return tostring(total + acc(0))
