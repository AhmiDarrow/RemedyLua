-- Method calls through metatables: self-calls, field reads/writes, a two-level __index chain.
local Point = {}
Point.__index = Point
function Point.new(x, y) return setmetatable({ x = x, y = y }, Point) end
function Point:add(o) self.x = self.x + o.x; self.y = self.y + o.y end
function Point:len2() return self.x * self.x + self.y * self.y end
local Counter = setmetatable({}, { __index = Point })
Counter.__index = Counter
function Counter.new() return setmetatable(Point.new(0, 0), Counter) end
function Counter:tick(n) self.x = self.x + n end
local p, q, c = Point.new(1, 2), Point.new(0, 0), Counter.new()
local s = 0
for _ = 1, 1000000 do
  q:add(p)
  c:tick(1)
  s = s + c:len2() % 7
end
return tostring(s + q.x + q.y)
