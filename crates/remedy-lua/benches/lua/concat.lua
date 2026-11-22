-- The `..` operator: a growing accumulator, short multi-part keys, numbers coerced.
local s = ""
for _ = 1, 20000 do s = s .. "x" end
local n = 0
for i = 1, 300000 do
  local a = "k" .. i
  local b = a .. ":" .. i .. ":" .. "v"
  n = n + #b
end
local line = ""
for i = 1, 3000 do line = line .. i .. "," end
return string.format("%d %d %d", #s, n, #line)
