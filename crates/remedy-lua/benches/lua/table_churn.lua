-- Small mixed tables made and dropped, string-keyed hash writes, pairs, deletes.
local n = 0
for i = 1, 300000 do
  local t = { i, i + 1, x = i, y = "a", z = i * 2 }
  t[#t + 1] = t.x
  t.w = t.z + t.x
  n = n + #t + t.w
end
local m = {}
for i = 1, 100000 do m["k" .. (i % 5000)] = i end
local cnt, sum = 0, 0
for _ = 1, 10 do
  for _, v in pairs(m) do cnt = cnt + 1; sum = sum + v end
end
for i = 1, 5000, 2 do m["k" .. i] = nil end
for _ in pairs(m) do cnt = cnt + 1 end
return string.format("%d %d %d", n, cnt, sum)
