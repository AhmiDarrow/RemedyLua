-- MAJOR (top priority): assigning to the newest local an expression that is a concat
-- reading that same local builds the concat in the local's own register first.
-- Expected "ab" "ac" "ayes" "azb"; RemedyLua gives "aa" "aa" "ano" "aza".
-- Cause: compiler.rs:1440 (ExprKind::Concat: "dst is the newest register" reuse) is
-- also taken when dst is an active local being assigned.
local p = "b"
p = "a" .. p
local q = "c"
q = "a" .. q
local r = "b"
r = "a" .. (r == "b" and "yes" or "no")
local s = "b"
s = "a" .. "z" .. tostring(s)
return p, q, r, s
