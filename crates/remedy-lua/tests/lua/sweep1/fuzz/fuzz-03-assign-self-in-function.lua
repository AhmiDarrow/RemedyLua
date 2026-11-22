-- MAJOR: same miscompile as fuzz-01 inside a function: the old value of the parameter
-- is never read, so an error C Lua raises is silently skipped and the result is wrong.
-- C Lua: false, "t:7: bad argument #1 to 'find' (string expected, got nil)";
-- RemedyLua: true, "a0".
local function f(p)
  -- string.find(p) must see p == nil
  p = "a" .. (string.find(p, "x") or 0)
  return p
end
return pcall(f)
