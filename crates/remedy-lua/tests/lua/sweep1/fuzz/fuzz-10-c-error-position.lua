-- MINOR: errors raised by the runtime while a C function is the running frame get a
-- Lua position in RemedyLua. C Lua adds "chunk:line:" only when the erroring frame is a
-- Lua function (luaG_runerror checks isLua(ci)). Expected (C Lua):
--   false  "attempt to call a table value"     (pcall calling a table)
--   false  "invalid key to 'next'"
-- RemedyLua prefixes a "chunk:line:" of the nearest Lua frame. Cause: vm.rs builds
-- runtime-error messages from the nearest Lua frame instead of the current frame.
local a, b = pcall({})
local c, d = pcall(next, {x = 1}, "nokey")
return a, b, c, d
