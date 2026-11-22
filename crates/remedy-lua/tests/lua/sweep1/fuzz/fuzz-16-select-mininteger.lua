-- CRITICAL: select(math.mininteger, ...) panics in RemedyLua ("attempt to negate with
-- overflow", stdlib/base.rs:203 `-i > n`) in debug builds; in release builds `-i` wraps
-- and select silently returns nothing. C Lua: "bad argument #1 to 'select' (index out
-- of range)". Fix: compare without negating (e.g. `i < -n`) or use checked arithmetic.
return pcall(select, math.mininteger, 1, 2, 3)
