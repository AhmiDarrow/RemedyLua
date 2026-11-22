-- utf8.char, utf8.len, utf8.codepoint and utf8.codes over mixed text.
local parts = {}
for i = 1, 200 do parts[#parts + 1] = utf8.char(0x41 + i % 26, 0x3b1 + i % 20, 0x4e00 + i) end
local s = table.concat(parts)
local n = 0
for r = 1, 4000 do
  n = n + utf8.len(s)
  for _, c in utf8.codes(s) do n = n + c % 7 end
  n = n + utf8.codepoint(s, 1)
end
return n
