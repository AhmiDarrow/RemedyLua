-- Coroutine ping-pong: wrap with values both ways, generators in for-in, create/resume.
local co = coroutine.wrap(function(a)
  local s = 0
  while true do
    s = s + a
    a = coroutine.yield(s)
  end
end)
local last
for i = 1, 300000 do last = co(i) end
local function gen(n)
  return coroutine.wrap(function() for i = 1, n do coroutine.yield(i) end end)
end
local sum = 0
for _ = 1, 2000 do
  for v in gen(100) do sum = sum + v end
end
local c2 = coroutine.create(function(x) while true do x = coroutine.yield(x * 2) end end)
local acc = 0
for i = 1, 100000 do
  local _, v = coroutine.resume(c2, i)
  acc = acc + v
end
return string.format("%d %d %d", last, sum, acc)
