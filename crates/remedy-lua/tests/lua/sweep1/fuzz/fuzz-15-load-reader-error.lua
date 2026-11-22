-- MAJOR: an error raised by a `load` reader function escapes `load`. Lua 5.4 loads
-- under protection: load returns nil plus the message. Expected (C Lua):
--   nil  "t:9: attempt to perform arithmetic on a nil value (global 'i')"
--   nil  "t:11: reader failed"
-- RemedyLua raises the error out of load. Cause: stdlib/base.rs load (reader calls
-- not protected).
local ok1, a, b = pcall(function()
  return load(function() i = i + 1 end)
end)
local n = 0
local ok2, c, d = pcall(load, function() n = n + 1 if n == 2 then error("reader failed") end return "return 1" end)
return ok1, a, b, ok2, c, d
