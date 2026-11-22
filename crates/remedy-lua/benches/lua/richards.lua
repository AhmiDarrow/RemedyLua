-- Richards: Martin Richards' operating-system scheduler simulation (task control blocks,
-- packets, idle/worker/handler/device tasks). Polymorphic method calls and field traffic.
-- A correct run has queueCount 2322 and holdCount 928.
local ID_IDLE, ID_WORKER, ID_HANDLER_A, ID_HANDLER_B, ID_DEVICE_A, ID_DEVICE_B = 1, 2, 3, 4, 5, 6
local KIND_DEVICE, KIND_WORK = 0, 1
local DATA_SIZE = 4
local STATE_RUNNING, STATE_RUNNABLE, STATE_SUSPENDED, STATE_HELD = 0, 1, 2, 4
local STATE_SUSPENDED_RUNNABLE = STATE_SUSPENDED | STATE_RUNNABLE
local STATE_NOT_HELD = ~STATE_HELD

local function class()
  local c = {}
  c.__index = c
  return c
end

local Packet = class()
function Packet.new(link, id, kind)
  local a2 = {}
  for i = 1, DATA_SIZE do a2[i] = 0 end
  return setmetatable({ link = link, id = id, kind = kind, a1 = 0, a2 = a2 }, Packet)
end
function Packet:addTo(queue)
  self.link = nil
  if queue == nil then return self end
  local next = queue
  while true do
    local peek = next.link
    if peek == nil then break end
    next = peek
  end
  next.link = self
  return queue
end

local Tcb = class()
function Tcb.new(link, id, priority, queue, task)
  return setmetatable({ link = link, id = id, priority = priority, queue = queue, task = task,
    state = queue == nil and STATE_SUSPENDED or STATE_SUSPENDED_RUNNABLE }, Tcb)
end
function Tcb:setRunning() self.state = STATE_RUNNING end
function Tcb:markAsNotHeld() self.state = self.state & STATE_NOT_HELD end
function Tcb:markAsHeld() self.state = self.state | STATE_HELD end
function Tcb:isHeldOrSuspended() return (self.state & STATE_HELD) ~= 0 or self.state == STATE_SUSPENDED end
function Tcb:markAsSuspended() self.state = self.state | STATE_SUSPENDED end
function Tcb:markAsRunnable() self.state = self.state | STATE_RUNNABLE end
function Tcb:run()
  local packet
  if self.state == STATE_SUSPENDED_RUNNABLE then
    packet = self.queue
    self.queue = packet.link
    self.state = self.queue == nil and STATE_RUNNING or STATE_RUNNABLE
  end
  return self.task:run(packet)
end
function Tcb:checkPriorityAdd(task, packet)
  if self.queue == nil then
    self.queue = packet
    self:markAsRunnable()
    if self.priority > task.priority then return self end
  else
    self.queue = packet:addTo(self.queue)
  end
  return task
end

local Scheduler = class()
function Scheduler.new()
  return setmetatable({ queueCount = 0, holdCount = 0, blocks = {} }, Scheduler)
end
function Scheduler:addTask(id, priority, queue, task)
  self.currentTcb = Tcb.new(self.list, id, priority, queue, task)
  self.list = self.currentTcb
  self.blocks[id] = self.currentTcb
end
function Scheduler:addRunningTask(id, priority, queue, task)
  self:addTask(id, priority, queue, task)
  self.currentTcb:setRunning()
end
function Scheduler:schedule()
  self.currentTcb = self.list
  while self.currentTcb ~= nil do
    if self.currentTcb:isHeldOrSuspended() then
      self.currentTcb = self.currentTcb.link
    else
      self.currentId = self.currentTcb.id
      self.currentTcb = self.currentTcb:run()
    end
  end
end
function Scheduler:release(id)
  local tcb = self.blocks[id]
  if tcb == nil then return tcb end
  tcb:markAsNotHeld()
  if tcb.priority > self.currentTcb.priority then return tcb end
  return self.currentTcb
end
function Scheduler:holdCurrent()
  self.holdCount = self.holdCount + 1
  self.currentTcb:markAsHeld()
  return self.currentTcb.link
end
function Scheduler:suspendCurrent()
  self.currentTcb:markAsSuspended()
  return self.currentTcb
end
function Scheduler:queue(packet)
  local t = self.blocks[packet.id]
  if t == nil then return t end
  self.queueCount = self.queueCount + 1
  packet.link = nil
  packet.id = self.currentId
  return t:checkPriorityAdd(self.currentTcb, packet)
end

local IdleTask = class()
function IdleTask.new(s, v1, count) return setmetatable({ scheduler = s, v1 = v1, count = count }, IdleTask) end
function IdleTask:run(_)
  self.count = self.count - 1
  if self.count == 0 then return self.scheduler:holdCurrent() end
  if (self.v1 & 1) == 0 then
    self.v1 = self.v1 >> 1
    return self.scheduler:release(ID_DEVICE_A)
  end
  self.v1 = (self.v1 >> 1) ~ 0xD008
  return self.scheduler:release(ID_DEVICE_B)
end

local DeviceTask = class()
function DeviceTask.new(s) return setmetatable({ scheduler = s }, DeviceTask) end
function DeviceTask:run(packet)
  if packet == nil then
    if self.v1 == nil then return self.scheduler:suspendCurrent() end
    local v = self.v1
    self.v1 = nil
    return self.scheduler:queue(v)
  end
  self.v1 = packet
  return self.scheduler:holdCurrent()
end

local WorkerTask = class()
function WorkerTask.new(s, v1, v2) return setmetatable({ scheduler = s, v1 = v1, v2 = v2 }, WorkerTask) end
function WorkerTask:run(packet)
  if packet == nil then return self.scheduler:suspendCurrent() end
  self.v1 = self.v1 == ID_HANDLER_A and ID_HANDLER_B or ID_HANDLER_A
  packet.id = self.v1
  packet.a1 = 0
  for i = 1, DATA_SIZE do
    self.v2 = self.v2 + 1
    if self.v2 > 26 then self.v2 = 1 end
    packet.a2[i] = self.v2
  end
  return self.scheduler:queue(packet)
end

local HandlerTask = class()
function HandlerTask.new(s) return setmetatable({ scheduler = s }, HandlerTask) end
function HandlerTask:run(packet)
  if packet ~= nil then
    if packet.kind == KIND_WORK then self.v1 = packet:addTo(self.v1) else self.v2 = packet:addTo(self.v2) end
  end
  if self.v1 ~= nil then
    local count = self.v1.a1
    if count < DATA_SIZE then
      if self.v2 ~= nil then
        local v = self.v2
        self.v2 = self.v2.link
        v.a1 = self.v1.a2[count + 1]
        self.v1.a1 = count + 1
        return self.scheduler:queue(v)
      end
    else
      local v = self.v1
      self.v1 = self.v1.link
      return self.scheduler:queue(v)
    end
  end
  return self.scheduler:suspendCurrent()
end

local function runRichards()
  local s = Scheduler.new()
  s:addRunningTask(ID_IDLE, 0, nil, IdleTask.new(s, 1, 1000))
  local q = Packet.new(nil, ID_WORKER, KIND_WORK)
  q = Packet.new(q, ID_WORKER, KIND_WORK)
  s:addTask(ID_WORKER, 1000, q, WorkerTask.new(s, ID_HANDLER_A, 0))
  q = Packet.new(nil, ID_DEVICE_A, KIND_DEVICE)
  q = Packet.new(q, ID_DEVICE_A, KIND_DEVICE)
  q = Packet.new(q, ID_DEVICE_A, KIND_DEVICE)
  s:addTask(ID_HANDLER_A, 2000, q, HandlerTask.new(s))
  q = Packet.new(nil, ID_DEVICE_B, KIND_DEVICE)
  q = Packet.new(q, ID_DEVICE_B, KIND_DEVICE)
  q = Packet.new(q, ID_DEVICE_B, KIND_DEVICE)
  s:addTask(ID_HANDLER_B, 3000, q, HandlerTask.new(s))
  s:addTask(ID_DEVICE_A, 4000, nil, DeviceTask.new(s))
  s:addTask(ID_DEVICE_B, 5000, nil, DeviceTask.new(s))
  s:schedule()
  return s.queueCount, s.holdCount
end

local qc, hc = 0, 0
for _ = 1, 60 do
  local a, b = runRichards()
  qc, hc = a, b
end
return qc .. " " .. hc
