-- Building strings from parts: tostring, table.concat, string.rep with a separator.
local parts = {}
for i = 1, 300000 do parts[#parts + 1] = tostring(i) end
local s = table.concat(parts, ",")
local buf = {}
for i = 1, 100000 do buf[#buf + 1] = "item" .. i .. ";" end
local s2 = table.concat(buf)
local rep = string.rep("ab", 100000, "-")
return string.format("%d %d %d", #s, #s2, #rep)
