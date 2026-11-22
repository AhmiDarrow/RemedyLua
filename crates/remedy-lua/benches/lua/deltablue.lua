-- DeltaBlue: the incremental constraint-hierarchy solver (Freeman-Benson, Maloney, Borning),
-- with its chain and projection tests. Deep class hierarchies through __index chains,
-- virtual calls, ordered collections. `failures` must be 0.
local failures = 0
local planner

local function class(parent)
  local c = {}
  c.__index = c
  if parent then setmetatable(c, { __index = parent }) end
  return c
end

local OC = class() -- OrderedCollection
function OC.new() return setmetatable({ elms = {} }, OC) end
function OC:add(e) local el = self.elms; el[#el + 1] = e end
function OC:at(i) return self.elms[i] end
function OC:size() return #self.elms end
function OC:removeFirst() return table.remove(self.elms, 1) end
function OC:remove(e)
  local el, index, skipped = self.elms, 1, 0
  local n = #el
  for i = 1, n do
    local v = el[i]
    if v ~= e then el[index] = v; index = index + 1 else skipped = skipped + 1 end
  end
  for i = 1, skipped do el[n - i + 1] = nil end
end

local Plan = class(OC)
function Plan.new() return setmetatable({ elms = {} }, Plan) end
function Plan:execute()
  local el = self.elms
  for i = 1, #el do el[i]:execute() end
end

local Strength = class()
local S = {}
for i, name in ipairs({ "required", "strongPreferred", "preferred", "strongDefault", "normal", "weakDefault", "weakest" }) do
  S[i - 1] = setmetatable({ value = i - 1, name = name }, Strength)
end
local REQUIRED, STRONG_DEFAULT, PREFERRED, NORMAL, WEAKEST = S[0], S[3], S[2], S[4], S[6]
local function stronger(a, b) return a.value < b.value end
local function weaker(a, b) return a.value > b.value end
local function weakestOf(a, b) if weaker(a, b) then return a end return b end
function Strength:nextWeaker() return S[self.value + 1] end

local Constraint = class()
function Constraint:addConstraint()
  self:addToGraph()
  planner:incrementalAdd(self)
end
function Constraint:satisfy(mark)
  self:chooseMethod(mark)
  if not self:isSatisfied() then
    if self.strength == REQUIRED then failures = failures + 1 end
    return nil
  end
  self:markInputs(mark)
  local out = self:output()
  local overridden = out.determinedBy
  if overridden ~= nil then overridden:markUnsatisfied() end
  out.determinedBy = self
  if not planner:addPropagate(self, mark) then failures = failures + 1 end
  out.mark = mark
  return overridden
end
function Constraint:destroyConstraint()
  if self:isSatisfied() then planner:incrementalRemove(self) else self:removeFromGraph() end
end
function Constraint:isInput() return false end

local Unary = class(Constraint)
function Unary.init(self, v, strength)
  self.strength = strength
  self.myOutput = v
  self.satisfied = false
  self:addConstraint()
  return self
end
function Unary:addToGraph()
  self.myOutput:addConstraint(self)
  self.satisfied = false
end
function Unary:chooseMethod(mark)
  self.satisfied = self.myOutput.mark ~= mark and stronger(self.strength, self.myOutput.walkStrength)
end
function Unary:isSatisfied() return self.satisfied end
function Unary:markInputs(_) end
function Unary:output() return self.myOutput end
function Unary:recalculate()
  self.myOutput.walkStrength = self.strength
  self.myOutput.stay = not self:isInput()
  if self.myOutput.stay then self:execute() end
end
function Unary:markUnsatisfied() self.satisfied = false end
function Unary:inputsKnown() return true end
function Unary:removeFromGraph()
  if self.myOutput ~= nil then self.myOutput:removeConstraint(self) end
  self.satisfied = false
end

local Stay = class(Unary)
function Stay.new(v, s) return Unary.init(setmetatable({}, Stay), v, s) end
function Stay:execute() end

local Edit = class(Unary)
function Edit.new(v, s) return Unary.init(setmetatable({}, Edit), v, s) end
function Edit:isInput() return true end
function Edit:execute() end

local NONE, FORWARD, BACKWARD = 0, 1, -1

local Binary = class(Constraint)
function Binary.init(self, v1, v2, strength)
  self.strength = strength
  self.v1 = v1
  self.v2 = v2
  self.direction = NONE
  self:addConstraint()
  return self
end
function Binary:chooseMethod(mark)
  if self.v1.mark == mark then
    self.direction = (self.v2.mark ~= mark and stronger(self.strength, self.v2.walkStrength)) and FORWARD or NONE
  end
  if self.v2.mark == mark then
    self.direction = (self.v1.mark ~= mark and stronger(self.strength, self.v1.walkStrength)) and BACKWARD or NONE
  end
  if weaker(self.v1.walkStrength, self.v2.walkStrength) then
    self.direction = stronger(self.strength, self.v1.walkStrength) and BACKWARD or NONE
  else
    self.direction = stronger(self.strength, self.v2.walkStrength) and FORWARD or BACKWARD
  end
end
function Binary:addToGraph()
  self.v1:addConstraint(self)
  self.v2:addConstraint(self)
  self.direction = NONE
end
function Binary:isSatisfied() return self.direction ~= NONE end
function Binary:markInputs(mark) self:input().mark = mark end
function Binary:input() if self.direction == FORWARD then return self.v1 end return self.v2 end
function Binary:output() if self.direction == FORWARD then return self.v2 end return self.v1 end
function Binary:recalculate()
  local ihn, out = self:input(), self:output()
  out.walkStrength = weakestOf(self.strength, ihn.walkStrength)
  out.stay = ihn.stay
  if out.stay then self:execute() end
end
function Binary:markUnsatisfied() self.direction = NONE end
function Binary:inputsKnown(mark)
  local i = self:input()
  return i.mark == mark or i.stay or i.determinedBy == nil
end
function Binary:removeFromGraph()
  if self.v1 ~= nil then self.v1:removeConstraint(self) end
  if self.v2 ~= nil then self.v2:removeConstraint(self) end
  self.direction = NONE
end

local Scale = class(Binary)
function Scale.new(src, scale, offset, dest, strength)
  return Binary.init(setmetatable({ direction = NONE, scale = scale, offset = offset }, Scale), src, dest, strength)
end
function Scale:addToGraph()
  Binary.addToGraph(self)
  self.scale:addConstraint(self)
  self.offset:addConstraint(self)
end
function Scale:removeFromGraph()
  Binary.removeFromGraph(self)
  if self.scale ~= nil then self.scale:removeConstraint(self) end
  if self.offset ~= nil then self.offset:removeConstraint(self) end
end
function Scale:markInputs(mark)
  Binary.markInputs(self, mark)
  self.scale.mark = mark
  self.offset.mark = mark
end
function Scale:execute()
  if self.direction == FORWARD then
    self.v2.value = self.v1.value * self.scale.value + self.offset.value
  else
    self.v1.value = (self.v2.value - self.offset.value) // self.scale.value
  end
end
function Scale:recalculate()
  local ihn, out = self:input(), self:output()
  out.walkStrength = weakestOf(self.strength, ihn.walkStrength)
  out.stay = ihn.stay and self.scale.stay and self.offset.stay
  if out.stay then self:execute() end
end

local Equality = class(Binary)
function Equality.new(v1, v2, s) return Binary.init(setmetatable({}, Equality), v1, v2, s) end
function Equality:execute()
  if self.direction == FORWARD then self.v2.value = self.v1.value else self.v1.value = self.v2.value end
end

local Variable = class()
function Variable.new(name, value)
  return setmetatable({ value = value or 0, constraints = OC.new(), mark = 0, walkStrength = WEAKEST,
    stay = true, name = name }, Variable)
end
function Variable:addConstraint(c) self.constraints:add(c) end
function Variable:removeConstraint(c)
  self.constraints:remove(c)
  if self.determinedBy == c then self.determinedBy = nil end
end

local Planner = class()
function Planner.new() return setmetatable({ currentMark = 0 }, Planner) end
function Planner:newMark()
  self.currentMark = self.currentMark + 1
  return self.currentMark
end
function Planner:incrementalAdd(c)
  local mark = self:newMark()
  local overridden = c:satisfy(mark)
  while overridden ~= nil do overridden = overridden:satisfy(mark) end
end
function Planner:incrementalRemove(c)
  local out = c:output()
  c:markUnsatisfied()
  c:removeFromGraph()
  local unsatisfied = self:removePropagateFrom(out)
  local strength = REQUIRED
  repeat
    for i = 1, unsatisfied:size() do
      local u = unsatisfied:at(i)
      if u.strength == strength then self:incrementalAdd(u) end
    end
    strength = strength:nextWeaker()
  until strength == WEAKEST
end
function Planner:makePlan(sources)
  local mark = self:newMark()
  local plan = Plan.new()
  local todo = sources
  while todo:size() > 0 do
    local c = todo:removeFirst()
    if c:output().mark ~= mark and c:inputsKnown(mark) then
      plan:add(c)
      c:output().mark = mark
      self:addConstraintsConsumingTo(c:output(), todo)
    end
  end
  return plan
end
function Planner:extractPlanFromConstraints(constraints)
  local sources = OC.new()
  for i = 1, constraints:size() do
    local c = constraints:at(i)
    if c:isInput() and c:isSatisfied() then sources:add(c) end
  end
  return self:makePlan(sources)
end
function Planner:addPropagate(c, mark)
  local todo = OC.new()
  todo:add(c)
  while todo:size() > 0 do
    local d = todo:removeFirst()
    if d:output().mark == mark then
      self:incrementalRemove(c)
      return false
    end
    d:recalculate()
    self:addConstraintsConsumingTo(d:output(), todo)
  end
  return true
end
function Planner:removePropagateFrom(out)
  out.determinedBy = nil
  out.walkStrength = WEAKEST
  out.stay = true
  local unsatisfied, todo = OC.new(), OC.new()
  todo:add(out)
  while todo:size() > 0 do
    local v = todo:removeFirst()
    for i = 1, v.constraints:size() do
      local c = v.constraints:at(i)
      if not c:isSatisfied() then unsatisfied:add(c) end
    end
    local determining = v.determinedBy
    for i = 1, v.constraints:size() do
      local nxt = v.constraints:at(i)
      if nxt ~= determining and nxt:isSatisfied() then
        nxt:recalculate()
        todo:add(nxt:output())
      end
    end
  end
  return unsatisfied
end
function Planner:addConstraintsConsumingTo(v, coll)
  local determining = v.determinedBy
  local cc = v.constraints
  for i = 1, cc:size() do
    local c = cc:at(i)
    if c ~= determining and c:isSatisfied() then coll:add(c) end
  end
end

local function chainTest(n)
  planner = Planner.new()
  local prev, first, last
  for i = 0, n do
    local v = Variable.new("v" .. i)
    if prev ~= nil then Equality.new(prev, v, REQUIRED) end
    if i == 0 then first = v end
    if i == n then last = v end
    prev = v
  end
  Stay.new(last, STRONG_DEFAULT)
  local edit = Edit.new(first, PREFERRED)
  local edits = OC.new()
  edits:add(edit)
  local plan = planner:extractPlanFromConstraints(edits)
  for i = 0, 99 do
    first.value = i
    plan:execute()
    if last.value ~= i then failures = failures + 1 end
  end
  return last.value
end

local function change(v, newValue)
  local edit = Edit.new(v, PREFERRED)
  local edits = OC.new()
  edits:add(edit)
  local plan = planner:extractPlanFromConstraints(edits)
  for _ = 1, 10 do
    v.value = newValue
    plan:execute()
  end
  edit:destroyConstraint()
end

local function projectionTest(n)
  planner = Planner.new()
  local scale, offset = Variable.new("scale", 10), Variable.new("offset", 1000)
  local src, dst
  local dests = OC.new()
  for i = 0, n - 1 do
    src = Variable.new("src" .. i, i)
    dst = Variable.new("dst" .. i, i)
    dests:add(dst)
    Stay.new(src, NORMAL)
    Scale.new(src, scale, offset, dst, REQUIRED)
  end
  change(src, 17)
  if dst.value ~= 1170 then failures = failures + 1 end
  change(dst, 1050)
  if src.value ~= 5 then failures = failures + 1 end
  change(scale, 5)
  for i = 0, n - 2 do if dests:at(i + 1).value ~= i * 5 + 1000 then failures = failures + 1 end end
  change(offset, 2000)
  for i = 0, n - 2 do if dests:at(i + 1).value ~= i * 5 + 2000 then failures = failures + 1 end end
  return dst.value
end

local acc = 0
for _ = 1, 40 do acc = acc + chainTest(100) + projectionTest(100) end
return failures .. " " .. acc
