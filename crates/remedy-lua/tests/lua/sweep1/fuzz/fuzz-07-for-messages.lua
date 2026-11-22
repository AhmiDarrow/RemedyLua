-- MINOR: numeric-for errors use the pre-5.4.7 wording. Lua 5.4.7:
-- "bad 'for' initial value (number expected, got table)", "bad 'for' limit (number
-- expected, got nil)", "bad 'for' step (number expected, got string)". RemedyLua:
-- "'for' initial value must be a number" etc. Cause: vm.rs for-prep errors.
local r = {}
r[1] = select(2, pcall(function() for i = {}, 1 do end end))
r[2] = select(2, pcall(function() for i = 1, nil do end end))
r[3] = select(2, pcall(function() for i = 1, 2, "x" do end end))
return r[1], r[2], r[3]
