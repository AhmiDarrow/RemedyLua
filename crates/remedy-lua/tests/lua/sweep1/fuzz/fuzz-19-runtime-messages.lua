-- MINOR: coroutine functions given a non-thread say "coroutine expected"; Lua 5.4.7
-- says "thread expected" (lcorolib getco uses luaL_checktype -> typeerror "thread").
-- C Lua:     bad argument #1 to 'resume' (thread expected, got nil)
-- RemedyLua: bad argument #1 to 'resume' (coroutine expected, got nil)
-- Cause: stdlib/coroutine.rs argument check wording.
-- (os.date of an unrepresentable time also differed during the run; fixed in the tree.)
local a = {pcall(os.date, "!%j %a %b %p", 9223372036854775807)}
local b = {pcall(coroutine.resume, nil)}
local c = {pcall(coroutine.status, 1)}
return a[1], a[2], b[1], b[2], c[1], c[2]
