-- binary-trees (benchmarks game): allocate and walk many short-lived trees plus one long-lived.
local function bottomUp(d)
  if d == 0 then return {} end
  d = d - 1
  return { bottomUp(d), bottomUp(d) }
end
local function check(t)
  if t[1] then return 1 + check(t[1]) + check(t[2]) end
  return 1
end
local N, mindepth = 14, 4
local maxdepth = math.max(mindepth + 2, N)
local out = {}
out[#out + 1] = check(bottomUp(maxdepth + 1))
local long = bottomUp(maxdepth)
for d = mindepth, maxdepth, 2 do
  local iters = 2 ^ (maxdepth - d + mindepth)
  local c = 0
  for _ = 1, iters do c = c + check(bottomUp(d)) end
  out[#out + 1] = string.format("%d:%d", iters, c)
end
out[#out + 1] = check(long)
return table.concat(out, " ")
