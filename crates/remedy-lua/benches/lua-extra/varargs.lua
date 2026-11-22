-- Varargs: select('#'), select(i), table.pack/unpack through calls.
local function count(...) return select('#', ...) end
local function second(...) return (select(2, ...)) end
local function fwd(...) return count(...) + second(...) end
local function repack(...) local t = table.pack(...) return table.unpack(t, 1, t.n) end
local s = 0
for i = 1, 200000 do
  s = s + fwd(i, i + 1, i + 2) + select('#', repack(i, nil, i))
end
return s
