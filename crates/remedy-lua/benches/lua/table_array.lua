-- Array part: fill, indexed read, ipairs, #, table.insert/remove, append with #t + 1.
local t = {}
for i = 1, 1000000 do t[i] = i end
local s = 0
for i = 1, #t do s = s + t[i] end
for _, v in ipairs(t) do s = s + v end
local u = {}
for i = 1, 200000 do table.insert(u, i) end
for _ = 1, 100000 do s = s + table.remove(u) end
for r = 1, 20 do
  local a = {}
  for i = 1, 10000 do a[#a + 1] = i * r end
  s = s + #a + a[5000]
end
return tostring(s)
