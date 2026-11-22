-- string.byte / string.char loops (encoders, checksums).
local src = string.rep("The quick brown fox jumps over the lazy dog. ", 50)
local n = 0
for r = 1, 200 do
  local out = {}
  for i = 1, #src do
    local b = string.byte(src, i)
    n = n + b
    out[i] = string.char((b + 1) % 256)
  end
  n = n + #table.concat(out)
end
return n
