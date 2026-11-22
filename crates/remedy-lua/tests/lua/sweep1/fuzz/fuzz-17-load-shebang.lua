-- MINOR: `load` of a string whose first line starts with '#' skips that line. In C Lua
-- only luaL_loadfile skips a '#' first line; load()/lua_load do not:
-- C Lua: nil, [string "#x..."]:1: unexpected symbol near '#'.  RemedyLua: runs it (1).
-- Cause: lexer.rs:265-270 skips the shebang for every chunk (it should be the host's
-- file-loading path only). Same code breaks interpolation (fuzz-13).
local f, e = load("#x\nreturn 1")
return f and f() or e
