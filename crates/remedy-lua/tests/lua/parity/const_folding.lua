-- Compile-time constants as C Lua folds them (tests/parity_lua54.rs, parity_13): which
-- `<const>` locals get no register (no name in messages) and which keys are constants.
local r = {}
local cases = {"local t = {} return t[5].z", "local t = {} local k = 5 return t[k].z", "local t = {} return t[300].z", "local b <const> = 5 local t = {} return t[b].z", "local a <const> = 2 local b <const> = a + 3 local t = {} return t[b].z",
 "local a <const> = 2 local b <const> = a * 255 return b()", "local a <const> = 2 local b <const> = -a return b()", "local a <const> = 2 local b <const> = a // 0 return b()", "local a <const> = 2.0 local b <const> = a / 0 return b()", "local a <const> = \"x\" local b <const> = a .. \"y\" return b()", "local a <const> = 7 local b <const> = a & 3 return b()", "local b <const> = 2^53 return b()", "local b <const> = 1 < 2 return b()", "local b <const> = not nil return b()", "local b <const> = (5) return b()", "local a <const> = 3 local b <const> = a % 0 return b()", "local b <const> = 3 // 0.0 return b()", "local b <const> = -0.0 return b()", "local b <const> = 0/0 return b()",
 "local b <const> = 1/0 return b()", "local b <const> = 3.0 & 1 return b()", "local b <const> = 7.5 // 2 return b()", "local b <const> = 7.5 % 2 return b()", "local b <const> = ~2.0 return b()", "local b <const> = not 'x' return b()", "local a <const> = 'k' local t = {} return t[a].z", "local a <const> = 1.5 local t = {} return t[a].z", "local a <const> = 3 local t = {} return t[a * 100].z", "local a <const> = true local t = {} return t[a].z",
 "local a, b <const> = 1, 2 return b()", "local a <const>, b = 1, 2 return a()", "local a <const>, b <const> = 1, 2 return b()", "local a, b <const> = 1 return b()", "local a, b <const> = 1, 2, 3 return b()", "local a, b <const> = {}, 2 local t = {} return t[b].x", "local a <close>, b <const> = nil, 2 return b()", "local x, y <const> = (function() return 1, 9 end)(), 5 return y()",
 "local a <const> = 2 local b <const> = a ^ 0.5 return b()", "local a <const> = 5 local b <const> = a - 5 return b()", "local a <const> = 5.0 local b <const> = a - 5 return b()", "local a <const> = 1 local b <const> = a << 70 return b()", "local a <const> = 4 return (a + 1)()", "return (1 // 0)()", "return (2^1024)()", "local x = 3 return (x + 1/0)()",
}
for i, s in ipairs(cases) do
  local f, e = load(s)
  r[i] = i .. ":" .. (f and tostring(select(2, pcall(f))) or ("compile " .. e)):gsub("%[string.-%]:1: ", "")
end
return table.concat(r, "\n")
