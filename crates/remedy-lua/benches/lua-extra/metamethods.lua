-- __newindex, __len, __eq and __lt on small objects.
local V = {}
V.__index = V
V.__len = function(a) return a.n end
V.__eq = function(a, b) return a.n == b.n end
V.__lt = function(a, b) return a.n < b.n end
V.__newindex = function(t, k, v) rawset(t, k, v * 2) end
local function new(n) return setmetatable({n = n}, V) end
local c = 0
local a, b = new(1), new(2)
for i = 1, 300000 do
  a.x = i
  if a < b then c = c + 1 end
  if a == b then c = c + 2 end
  c = c + #a + a.x
end
return c
