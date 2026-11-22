-- Numeric for, while and repeat loops with integer, float and bitwise arithmetic.
local s = 0
for i = 1, 5000000 do s = s + i % 7 end
local f = 0.0
for i = 1, 2000000 do f = f + i * 0.5 end
local j, w = 0, 0
while j < 1000000 do
  j = j + 1
  if j % 3 == 0 then w = w + j elseif j % 5 == 0 then w = w - 1 end
end
local k = 0
repeat k = k + 2 until k >= 1000000
local nested = 0
for a = 1, 1000 do for b = 1, 1000 do nested = nested + (a ~ b) end end
return string.format("%d %.1f %d %d %d", s, f, w, k, nested)
