-- Integer division, modulo and bitwise operators (hashing, packing).
local h = 0
for i = 1, 3000000 do
  h = (h ~ (i << 5) ~ (i >> 3)) & 0xffffffff
  h = h + (i // 7) + (i % 13)
end
return h
