-- string.format-heavy: integer, hex, octal, padded string, fixed/general/exponent float, %c, %q.
local n = 0
for i = 1, 100000 do
  local s = string.format("%d %5d %-5d|%x %X %o %s %10s %.3f %g %e %5.1f %c %%",
    i, i, i, i, i, i, "abc", "def", i / 3, i * 0.1, i * 1.5, i / 7, 65 + i % 26)
  n = n + #s
end
local q = string.format("%q", "line\nbreak \"quoted\" \0 end")
return n .. " " .. q
