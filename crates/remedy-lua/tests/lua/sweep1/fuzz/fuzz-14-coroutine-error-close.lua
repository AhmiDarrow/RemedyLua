-- MAJOR: when a coroutine dies with an error, RemedyLua runs its pending to-be-closed
-- variables (`<close>` locals, the generic-for closing value) during the failing
-- resume. Lua 5.4 does not unwind a coroutine's stack on error: resume returns
-- false + error first, and the variables are closed only by coroutine.close (which then
-- returns false + the original error). Expected (C Lua):
--   resumed false t:9: boom | status dead | closed t:9: boom | close false t:9: boom
-- RemedyLua: "closed ..." comes before "resumed ...". Cause: vm.rs coroutine error
-- path closes tbc variables while unwinding (should leave them for coroutine.close).
local log = {}
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function(_, e) log[#log + 1] = "closed " .. tostring(e) end})
  error("boom")
end)
local ok, e = coroutine.resume(co)
log[#log + 1] = "resumed " .. tostring(ok) .. " " .. tostring(e)
log[#log + 1] = "status " .. coroutine.status(co)
local c1, c2 = coroutine.close(co)
log[#log + 1] = "close " .. tostring(c1) .. " " .. tostring(c2)
return table.concat(log, " | ")
