-- Deep recursion building and walking tables (a list of depth 150, many times).
local function build(d) if d == 0 then return nil end return {v = d, next = build(d - 1)} end
local function sum(l) if not l then return 0 end return l.v + sum(l.next) end
local s = 0
for r = 1, 4000 do s = s + sum(build(150)) end
return s
