-- GC-heavy allocation churn: tables, strings and closures that die young, a small retained set.
local keep = {}
local total = 0
for i = 1, 400000 do
  local t = { i, tostring(i), { x = i } }
  local f = function() return t[1] end
  total = total + f()
  if i % 100 == 0 then keep[#keep % 1000 + 1] = t end
end
local strs = 0
for i = 1, 200000 do
  local s = "str" .. i .. "x"
  strs = strs + #s
end
collectgarbage()
return string.format("%d %d %d", total, strs, #keep)
