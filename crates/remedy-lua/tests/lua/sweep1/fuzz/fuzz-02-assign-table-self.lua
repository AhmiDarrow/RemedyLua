-- MAJOR (top priority): `p = {p[1] + 1, #p}` with p the newest local: the new table is
-- built in p's register before the fields read p. C Lua: {2, 1}; RemedyLua: error
-- "attempt to perform arithmetic on a nil value (field 'integer index')".
-- Cause: compiler.rs:1501-1502 (table built in `dst` when it is the newest register).
local p = {1}
p = {p[1] + 1, #p}
return p
