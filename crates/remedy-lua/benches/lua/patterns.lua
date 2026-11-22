-- Pattern-heavy: gmatch with captures, gsub with function/replacement string, match, find.
local text = {}
for i = 1, 2000 do
  text[#text + 1] = "word" .. i .. " key" .. (i % 13) .. "=value" .. i .. "; email" .. i .. "@example.com"
end
text = table.concat(text, "\n")
local count = 0
for _ = 1, 5 do
  for k, v in text:gmatch("(%w+)=(%w+)") do count = count + #k + #v end
  local s, n = text:gsub("%d+", function(d) return tostring(#d) end)
  count = count + n + #s
  local s2, n2 = text:gsub("(%w+)@(%w+)%.com", "%2 at %1")
  count = count + n2 + #s2
  for line in text:gmatch("[^\n]+") do
    local a, b = line:match("^word(%d+) key(%d+)")
    count = count + tonumber(a) + tonumber(b)
    if line:find("%s[kK]ey1%d?=") then count = count + 1 end
  end
  local up = text:gsub("%a", string.upper)
  count = count + #up
end
return tostring(count)
