-- (plain-Lua twin of fuzz-13-interp-braces.rlua; same line numbers)
--
--
--
--
--
--
local p = 1
local t = {1}
local s = (tostring(#{1, p}) .. "x y")
local u = ("<" .. tostring(#t) .. ">")
local v = ("<" .. tostring([[}]]) .. ">")
return s, u, v
