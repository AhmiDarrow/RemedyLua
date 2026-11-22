-- MINOR: syntax errors at end of input say near '<eof>' (quoted); C Lua says near <eof>
-- (luaX_token2str gives "<eof>" unquoted for TK_EOS). Cause: lexer/parser error_near.
local a = select(2, load("x = "))
local b = select(2, load("local function"))
local c = select(2, load("return \"abc"))
return a, b, c
