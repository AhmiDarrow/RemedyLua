-- MINOR: string.format accepts '%F'; Lua 5.4.7 has no 'F' conversion (lstrlib.c
-- str_format: 'a','A','f','e','E','g','G' only) and raises
-- "invalid conversion '%F' to 'format'". RemedyLua returns "1.500000".
-- Cause: stdlib/strformat.rs conversion list.
return pcall(string.format, "%F", 1.5)
