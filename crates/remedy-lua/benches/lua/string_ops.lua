-- String library basics through method syntax: sub, upper, lower, byte, char, plain find, rep, reverse.
local n = 0
local base = "The quick brown fox jumps over the lazy dog"
for i = 1, 200000 do
  local s = base:sub(i % 10 + 1, i % 10 + 20)
  n = n + #s:upper() + #s:lower() + s:byte(1) + (s:find("o", 1, true) or 0)
  n = n + #string.char(65 + i % 26, 66) + #s:rep(2) + #s:reverse()
end
return tostring(n)
