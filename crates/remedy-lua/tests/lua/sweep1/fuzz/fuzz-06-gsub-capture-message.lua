-- MINOR: message text. A bad capture index in a gsub replacement string: Lua 5.4.7 says
-- "invalid capture index %2" (lstrlib.c get_onecapture); RemedyLua says
-- "invalid capture index %2 in replacement string". Cause: stdlib/pattern.rs/string.rs.
return pcall(string.gsub, "abc", "(b)", "%2")
