-- MINOR: syntax-error messages that differ from C Lua 5.4.7 (found by the mutation fuzzer).
-- a: C "<name> or '...' expected near '63'"         RemedyLua "<name> expected near '63'"
-- b: C reports the first error in source order ("unexpected symbol near ','"); RemedyLua
--    lexes the whole chunk before parsing, so a later lexical error wins
--    ("malformed number near '0x.p1'"). Cause: lexer.rs tokens() runs ahead of the parser.
-- c: C "decimal escape too large near '"\300"'"     RemedyLua omits the "near" part.
-- d: C "<eof> expected near '`'"                     RemedyLua "unexpected symbol near '`'"
-- e: C "<eof> expected near ')'"                      RemedyLua "'<eof>' expected near ')'"
local function m(s) return select(2, load(s)) end
return m("f = function(63"), m(", 0x.p1"), m([[x = "\300"]]), m("return 1` 2"), m("return f)")
