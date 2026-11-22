-- string.format-heavy logging lines.
local n = 0
for i = 1, 150000 do
  local line = string.format("[%05d] %-8s %6.2f%% user=%s id=%x", i, "INFO", i / 1000, "u" .. (i % 97), i)
  n = n + #line
end
return n
