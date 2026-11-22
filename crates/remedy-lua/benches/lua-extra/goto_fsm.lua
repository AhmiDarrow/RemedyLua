-- A goto-based state machine (a tokenizer over digits and letters).
local input = string.rep("abc123 de45 f6 ", 2000)
local count = 0
for r = 1, 20 do
  local i, n = 1, #input
  ::start::
  if i > n then goto done end
  do
    local c = input:byte(i)
    if c >= 48 and c <= 57 then goto digit end
    if c >= 97 and c <= 122 then goto alpha end
    i = i + 1
    goto start
  end
  ::digit::
  while i <= n and input:byte(i) >= 48 and input:byte(i) <= 57 do i = i + 1 end
  count = count + 1
  goto start
  ::alpha::
  while i <= n and input:byte(i) >= 97 and input:byte(i) <= 122 do i = i + 1 end
  count = count + 2
  goto start
  ::done::
end
return count
