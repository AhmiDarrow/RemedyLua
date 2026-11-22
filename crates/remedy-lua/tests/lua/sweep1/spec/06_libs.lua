-- case: table.insert positions
local t = {1, 2, 3}
table.insert(t, 1, 0)
table.insert(t, #t + 1, 9)
table.insert(t, "x")
return table.concat(t, ","), pcall(table.insert, t, 0, 1), pcall(table.insert, t, #t + 2, 1), pcall(table.insert, t), pcall(table.insert, t, 1, 2, 3), pcall(table.insert, nil, 1), pcall(table.insert, t, "a", 1), pcall(table.insert, t, 1.5, 1)
-- case: table.insert on empty with pos
local t = {}
table.insert(t, 1, "a")
return t[1], pcall(table.insert, {}, 2, "a")
-- case: table.remove positions
local t = {1, 2, 3, 4}
local a = table.remove(t)
local b = table.remove(t, 1)
local e = {}
local c = table.remove(e)
local d = table.remove(e, 0)
local f = table.remove({}, #e + 1)
local g = {n = 1}
return a, b, table.concat(t, ","), c, d, f, pcall(table.remove, {1, 2}, 5), pcall(table.remove, {1, 2}, -1), table.remove({1, 2}, 3), pcall(table.remove, {}, -1), pcall(table.remove, {}, 2)
-- case: table.remove at size zero with pos zero and border
local t = {[0] = "z"}
return table.remove(t, 0), t[0], table.remove({[0] = "q"}), #t
-- case: table.concat errors and ranges
return table.concat({}), table.concat({1, 2, 3}, ", ", 2, 3), table.concat({1, 2}, "", 3), table.concat({"a"}, nil), pcall(table.concat, {1, {}, 3}), pcall(table.concat, {1, 2}, ",", 1, 3), table.concat({1.5, 2, "x"}), pcall(table.concat, {true}), table.concat({1, 2, 3}, ",", -1, 1)
-- case: table.concat with numbers formatting
return table.concat({1.0, 2^63, -0.0, 1e100, 3})
-- case: table.unpack ranges
return table.unpack({1, 2, 3}), table.unpack({1, 2, 3}, 2), table.unpack({1, 2, 3}, 2, 3), select("#", table.unpack({}, 1, 0)), select("#", table.unpack({}, 5, 3)), table.unpack({1, 2}, -1, 1), pcall(table.unpack, {}, math.mininteger, math.maxinteger), pcall(table.unpack)
-- case: table.unpack with __index
local t = setmetatable({}, {__index = function(_, i) return i * 2 end})
return table.unpack(t, 1, 3)
-- case: table.pack
local p = table.pack(1, nil, 3)
local q = table.pack()
return p.n, p[3], q.n, #q
-- case: table.move basic and overlap
local t = {1, 2, 3, 4, 5}
table.move(t, 1, 3, 2)
local u = {1, 2, 3, 4, 5}
table.move(u, 2, 5, 1)
local v = {1, 2, 3}
local w = table.move(v, 1, 3, 1, {})
local x = table.move({1, 2}, 1, 0, 5)
return table.concat(t, ","), table.concat(u, ","), table.concat(w, ","), #x
-- case: table.move errors and limits
return pcall(table.move, {}, 1, math.maxinteger, 2), pcall(table.move, {}, -1, math.maxinteger, 2), pcall(table.move, {}, 1, 2, math.maxinteger), pcall(table.move, {1}, 1, 1, 1, 5), pcall(table.move), pcall(table.move, {}, math.mininteger, -1, 1)
-- case: table.move with metamethods
local log = {}
local src = setmetatable({}, {__index = function(_, k) log[#log + 1] = "get" .. k return k end})
local dst = setmetatable({}, {__newindex = function(t, k, v) log[#log + 1] = "set" .. k rawset(t, k, v) end})
table.move(src, 1, 3, 1, dst)
return table.concat(log, ",")
-- case: table.sort basic and comparators
local t = {5, 2, 8, 1, 9, 3}
table.sort(t)
local u = {"b", "a", "C", "c"}
table.sort(u)
local v = {3, 1, 2}
table.sort(v, function(a, b) return a > b end)
return table.concat(t, ","), table.concat(u, ","), table.concat(v, ",")
-- case: table.sort errors
return pcall(table.sort, {1, "a", 2}), pcall(table.sort, {1, 2, 3}, 5), pcall(table.sort, {{}, {}}), pcall(table.sort)
-- case: table.sort invalid order function
local t = {}
for i = 1, 100 do t[i] = (i * 37) % 101 end
local ok, e = pcall(table.sort, t, function(a, b) return true end)
return ok, e
-- case: table.sort with nils inside length
local ok = pcall(table.sort, {3, nil, 1})
return true
-- case: table.sort large arrays stable result
local t = {}
for i = 1, 1000 do t[i] = (i * 7919) % 1009 end
table.sort(t)
local ok = true
for i = 2, #t do if t[i - 1] > t[i] then ok = false end end
return ok, t[1], t[1000]
-- case: table functions honor __len and __index
local backing = {10, 20, 30}
local proxy = setmetatable({}, {__len = function() return #backing end, __index = backing, __newindex = backing})
table.insert(proxy, 40)
return table.concat(proxy, ","), #backing, table.unpack(proxy)
-- case: table.concat uses __index
local t = setmetatable({}, {__index = function(_, i) return "v" .. i end, __len = function() return 3 end})
return table.concat(t, "-")
-- case: math integer functions
return math.floor(3.7), math.floor(-3.7), math.ceil(3.2), math.ceil(-3.2), math.type(math.floor(3.5)), math.floor(2^70), math.type(math.floor(2^70)), math.ceil(-0.5), math.floor(5), math.floor(-0.0), pcall(math.floor, "x"), math.floor("3.7")
-- case: math abs and minmax
return math.abs(-0.0), math.abs(math.mininteger), math.abs(-3), math.abs(-3.5), math.max(1, 2.0), math.max(2.0, 1), math.max(1, 1.0), math.min(1.0, 1), pcall(math.max), math.max(0/0, 1) ~= math.max(0/0, 1), math.max(1, 0/0), math.min(-0.0, 0.0), math.max(3, "4")
-- case: math tointeger
return math.tointeger(3.0), math.tointeger(3.5), math.tointeger("8"), math.tointeger(2^63), math.tointeger(-2^63), math.tointeger(0/0), math.tointeger({}), math.tointeger(7), pcall(math.tointeger), math.tointeger("x")
-- case: math ult and fmod
return math.ult(1, 2), math.ult(-1, 2), math.ult(2, -1), pcall(math.ult, 1.5, 2), math.ult(1.0, 2), math.fmod(7, 3), math.fmod(-7, 3), math.fmod(7, -3), math.fmod(7.5, 2), math.fmod(-7.5, 2), pcall(math.fmod, 1, 0), math.fmod(1, 0.0) ~= math.fmod(1, 0.0), math.fmod(math.mininteger, -1), math.fmod(5, math.huge), math.type(math.fmod(7, 3)), math.type(math.fmod(7.0, 3))
-- case: math fmod with mininteger
return math.fmod(math.mininteger, math.mininteger), math.fmod(-6, math.mininteger), math.fmod(math.mininteger, 3), math.fmod(3, math.mininteger)
-- case: math constants and misc
return math.pi, math.huge, -math.huge, math.maxinteger, math.mininteger, math.sqrt(16), math.sqrt(-1) ~= math.sqrt(-1), math.exp(0), math.log(1), math.log(8, 2), math.log(100, 10), math.log(27, 3), math.log(0), math.type(1), math.type(1.0), math.type("1"), pcall(math.type)
-- case: math trig
return math.sin(0), math.cos(0), math.tan(0), math.asin(1), math.acos(1), math.atan(1), math.atan(1, 2), math.atan(0, -1), math.atan(-0.0, -1), math.deg(math.pi), math.rad(180), math.atan(1, 0)
-- case: math modf
return math.modf(3.7), math.modf(-3.7), math.modf(5), math.modf(1/0), math.modf(-1/0), math.modf(0/0) ~= math.modf(0/0), math.type(math.modf(5)), math.type(select(2, math.modf(5))), math.modf(-0.0), math.modf(2^53)
-- case: math.random ranges
math.randomseed(42)
local ok = true
for i = 1, 200 do
  local a = math.random(); if a < 0 or a >= 1 then ok = false end
  local b = math.random(5); if b < 1 or b > 5 or math.type(b) ~= "integer" then ok = false end
  local c = math.random(-3, 3); if c < -3 or c > 3 then ok = false end
  local d = math.random(math.mininteger, math.maxinteger); if math.type(d) ~= "integer" then ok = false end
end
return ok, math.random(7, 7), pcall(math.random, 2, 1), pcall(math.random, 0), pcall(math.random, 1, 2, 3), math.type(math.random(0)), pcall(math.random, 1.5), math.random(3.0) <= 3
-- case: math.randomseed returns
local a, b = math.randomseed(7)
local x = math.random(1000)
math.randomseed(7)
local y = math.random(1000)
return math.type(a), math.type(b), x == y, select("#", math.randomseed())
-- case: math.random deterministic sequence same as C
math.randomseed(123)
local r = {}
for i = 1, 5 do r[i] = math.random(1, 1000000) end
r[6] = math.random(0)
r[7] = string.format("%.17g", math.random())
return table.concat(r, " ")
-- case: math.random with seed pair
math.randomseed(1, 2)
return math.random(1, 100000), math.random(math.mininteger, math.maxinteger)
-- case: utf8.char and charpattern
return utf8.char(), utf8.char(0x7F, 0x80, 0x7FF, 0x800, 0xFFFF, 0x10000, 0x10FFFF) == "\x7F\u{80}\u{7FF}\u{800}\u{FFFF}\u{10000}\u{10FFFF}", #utf8.char(0x7FFFFFFF), pcall(utf8.char, 0x80000000), pcall(utf8.char, -1), utf8.charpattern, pcall(utf8.char, "x")
-- case: utf8.len
return utf8.len("héllo"), utf8.len("\xffabc"), utf8.len("abc\xe4"), utf8.len("abc", 4), utf8.len("abc", 5) == nil or true, pcall(utf8.len, "abc", 5), utf8.len("héllo", -3), utf8.len("", 1), utf8.len("abc", 2, 1), pcall(utf8.len, "abc", 0), utf8.len("\u{D800}"), utf8.len("\u{D800}", 1, -1, true), utf8.len("\xC0\x80"), utf8.len("\xF4\x90\x80\x80"), utf8.len("\xF4\x90\x80\x80", 1, -1, true)
-- case: utf8.len continuation start
return utf8.len("\x80"), utf8.len("a\x80", 2), utf8.len("é", 2)
-- case: utf8.offset
local s = "aéb€c"
return utf8.offset(s, 1), utf8.offset(s, 2), utf8.offset(s, 3), utf8.offset(s, 5), utf8.offset(s, 6), utf8.offset(s, 7), utf8.offset(s, -1), utf8.offset(s, -5), utf8.offset(s, -6), utf8.offset(s, 0, 3), utf8.offset(s, 0, 4), pcall(utf8.offset, s, 1, 3), utf8.offset("", 1), utf8.offset("", 0), pcall(utf8.offset, s, 1, 100)
-- case: utf8.codes
local r = {}
for p, c in utf8.codes("aé€") do r[#r + 1] = p .. ":" .. c end
local ok, e = pcall(function() for p, c in utf8.codes("a\xffb") do end end)
local ok2, e2 = pcall(function() for p, c in utf8.codes("\u{D800}") do end end)
local r2 = {}
for p, c in utf8.codes("\u{D800}", true) do r2[#r2 + 1] = c end
return table.concat(r, ","), ok, e, ok2, e2, r2[1]
-- case: utf8.codes trailing continuation
return pcall(function() local n = 0 for p, c in utf8.codes("a\x80") do n = n + 1 end return n end), pcall(function() local n = 0 for p, c in utf8.codes("\xe4\xb8") do n = n + 1 end return n end)
-- case: utf8 overlong encodings
return utf8.len("\xC1\xBF"), utf8.len("\xE0\x80\x80"), utf8.len("\xF0\x80\x80\x80"), pcall(utf8.codepoint, "\xE0\x80\x80"), utf8.len("\xF8\x88\x80\x80\x80", 1, -1, true), utf8.len("\xFC\x84\x80\x80\x80\x80", 1, -1, true), utf8.len("\xFE", 1, -1, true)
-- case: coroutine status and running
local main, ismain = coroutine.running()
local co
co = coroutine.create(function()
  local c, m = coroutine.running()
  coroutine.yield(coroutine.status(co), c == co, m, coroutine.isyieldable())
end)
local r = {coroutine.resume(co)}
return type(main), ismain, r[2], r[3], r[4], r[5], coroutine.status(co), coroutine.isyieldable(), coroutine.resume(co), coroutine.status(co), coroutine.resume(co)
-- case: coroutine status normal
local outer
outer = coroutine.create(function()
  local inner = coroutine.create(function() return coroutine.status(outer) end)
  return coroutine.resume(inner)
end)
return coroutine.resume(outer)
-- case: resume running and self
return coroutine.resume(coroutine.running()), pcall(coroutine.resume, 1), select(2, coroutine.resume(coroutine.create(function() return coroutine.resume(coroutine.running()) end)))
-- case: wrap error with position prefix
local f = coroutine.wrap(function() local x = nil; return x.y end)
return pcall(f)
-- case: coroutine yield across pcall
local co = coroutine.create(function()
  local ok, v = pcall(function() local x = coroutine.yield(1) error("after " .. x, 0) end)
  coroutine.yield(ok, v)
  return "end"
end)
return coroutine.resume(co), coroutine.resume(co, "X"), coroutine.resume(co), coroutine.resume(co)
-- case: yield from main
return pcall(coroutine.yield, 1)
-- case: yield across metamethod and iterator
local co = coroutine.wrap(function()
  local t = setmetatable({}, {__index = function(_, k) return coroutine.yield(k) end, __add = function() return coroutine.yield("add") end, __lt = function() return coroutine.yield("lt") end, __concat = function() return coroutine.yield("cat") end, __eq = function() return coroutine.yield("eq") end})
  local a = t.foo
  local b = t + 1
  local c = t < t
  local d = t .. "x"
  local e = t == setmetatable({}, getmetatable(t))
  for i in function(_, i) if not i then return coroutine.yield("iter") end end do end
  return a, b, c, d, e
end)
return co(), co(1), co(2), co(3), co(nil), co(false), co(5)
-- case: yield across table.sort comparator
local co = coroutine.create(function() table.sort({3, 2, 1}, function(a, b) coroutine.yield() return a < b end) end)
return coroutine.resume(co)
-- case: yield across string.gsub callback
local co = coroutine.create(function() return string.gsub("ab", ".", function(c) return coroutine.yield(c) end) end)
return coroutine.resume(co)
-- case: coroutine.close states
local co = coroutine.create(function() coroutine.yield() end)
local a = {coroutine.close(co)}
local co2 = coroutine.create(function() error("x", 0) end)
coroutine.resume(co2)
local b = {coroutine.close(co2)}
local co3 = coroutine.create(function() coroutine.yield() end)
coroutine.resume(co3)
local c = {coroutine.close(co3)}
return a[1], b[1], b[2], c[1], coroutine.status(co3), pcall(coroutine.close, coroutine.running()), coroutine.resume(co3)
-- case: close running coroutine from inside
local co
co = coroutine.create(function() return pcall(coroutine.close, co) end)
return coroutine.resume(co)
-- case: close normal coroutine
local outer
outer = coroutine.create(function()
  local inner = coroutine.create(function() return pcall(coroutine.close, outer) end)
  return coroutine.resume(inner)
end)
return coroutine.resume(outer)
-- case: coroutine isyieldable with arg
local co = coroutine.create(function() return 1 end)
return coroutine.isyieldable(co), coroutine.isyieldable(coroutine.running())
-- case: coroutine passing values
local co = coroutine.create(function(a, b)
  local c, d = coroutine.yield(a + b)
  local e = coroutine.yield(c * d)
  return e, "fin"
end)
return coroutine.resume(co, 1, 2), coroutine.resume(co, 3, 4), coroutine.resume(co, 5), select("#", coroutine.resume(coroutine.create(function() coroutine.yield() end)))
-- case: error object through resume preserved
local co = coroutine.create(function() error(setmetatable({}, {__tostring = function() return "E" end})) end)
local ok, e = coroutine.resume(co)
return ok, tostring(e), type(e)
-- case: error position inside coroutine
local co = coroutine.create(function() local t = nil; return t[1] end)
return coroutine.resume(co)
-- case: os functions available
return type(os.time()), type(os.clock()), os.time({year = 2020, month = 1, day = 1, hour = 12}) - os.time({year = 2020, month = 1, day = 2, hour = 12}), os.date("!%Y-%m-%d", 0), type(os.difftime(2, 1))