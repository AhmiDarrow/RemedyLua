-- OOP through an __index function (not a table): property lookups computed on miss.
local Base = {}
local mt = {__index = function(t, k) local v = Base[k] if v then return v end return #k end}
function Base.get(self) return self.v end
local s = 0
for i = 1, 300000 do
  local o = setmetatable({v = i}, mt)
  s = s + o:get() + o.width + o.height
end
return s
