-- MINOR: wording of errors found by the compiler. C Lua 5.4.7:
--   [string "goto nowhere"]:1: no visible label 'nowhere' for <goto> at line 1
--   [string "break"]:1: break outside loop at line 1
-- RemedyLua: "... for goto at line 1", "... break outside a loop at line 1".
-- Cause: compiler.rs:309-316 wording. (During the run these errors also used the raw
-- chunk name instead of [string "..."]; that part is fixed in the current tree.)
return select(2, load("goto nowhere")), select(2, load("::a:: ::a::")), select(2, load("break"))
