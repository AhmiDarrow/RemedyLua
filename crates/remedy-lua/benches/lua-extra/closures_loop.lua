-- Closures made in a loop, capturing loop locals, called once or kept briefly.
local s = 0
local keep = {}
for i = 1, 300000 do
  local a, b = i, i * 2
  local f = function(x) return a + b + x end
  local g = function() a = a + 1 return a end
  s = s + f(1) + g()
  keep[i % 64 + 1] = f
end
return s
