-- table.sort: integers, a Lua comparator, strings.
local function lcg(x) return (x * 16807) % 2147483647 end
local t, x = {}, 42
for i = 1, 200000 do x = lcg(x); t[i] = x end
table.sort(t)
local ok = 0
for i = 2, #t do if t[i - 1] <= t[i] then ok = ok + 1 end end
local u = {}
for i = 1, 50000 do x = lcg(x); u[i] = x end
table.sort(u, function(a, b) return a > b end)
local w = {}
for i = 1, 50000 do x = lcg(x); w[i] = "s" .. x end
table.sort(w)
return string.format("%d %d %d %s %s", ok, t[1], u[1], w[1], w[#w])
