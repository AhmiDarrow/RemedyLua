-- `NAME` and `NAME2` are files that do not exist yet: temporary files in C Lua, the host's
-- in-memory files in RemedyLua. Every result is shown with its type; names are scrubbed.
local out = {} local Q, Q2 = ("%q"):format(NAME):sub(2, -2), ("%q"):format(NAME2):sub(2, -2) -- as `show` writes them
local function clean(v)
  if type(v) == "string" then
    v = v:gsub(NAME2, "<f2>", 1, true) -- (plain: names are not patterns)
  end
  return v
end
local function scrub(s)
  local i = 1
  while true do
    local a, b = s:find(Q, i, true)
    if not a then break end
    s = s:sub(1, a - 1) .. "<f>" .. s:sub(b + 1)
    i = a + 3
  end
  while true do
    local a, b = s:find(Q2, 1, true)
    if not a then break end
    s = s:sub(1, a - 1) .. "<f2>" .. s:sub(b + 1)
  end
  return s
end
local function show(...)
  local n = select('#', ...)
  local t = {}
  for i = 1, n do
    local v = select(i, ...)
    local ty = type(v)
    if ty == "number" then t[i] = (math.type(v) == "integer" and "i:" or "f:") .. string.format("%.17g", v)
    elseif ty == "string" then t[i] = string.format("%q", v)
    elseif ty == "userdata" then t[i] = io.type(v) or "userdata"
    elseif ty == "table" or ty == "function" or ty == "thread" then t[i] = ty
    else t[i] = tostring(v) end
  end
  return scrub(table.concat(t, ","))
end
local function P(label, f, ...)
  out[#out + 1] = label .. " => " .. show(pcall(f, ...))
end
P("open missing", io.open, NAME)
P("open bad mode", io.open, NAME, "rw")
P("open bad mode2", io.open, NAME, "")
P("open mode r+b", function() return io.open(NAME, "r+b") end)
P("open no name", io.open)
P("open num name", function() return io.open(12345678) end)
-- Write a file.
P("write", function()
  local f = assert(io.open(NAME, "w"))
  local r = f:write("line one\n", 2, " ", 3.5, " ", 1e100, " ", -0.0, " ", 2^63, " ", math.mininteger, "\n")
  local r2 = f:write()
  local s, e = f:seek("cur")
  f:write("12 0x1F -3.5e2 .5 1e 0x1p4 nan  +7\n", "last line no newline")
  local c1 = f:close()
  return r == f, r2 == f, s, c1, io.type(f), tostring(f)
end)
P("write inf/nan", function()
  local f = assert(io.open(NAME2, "w"))
  f:write(1/0, " ", -1/0, " ", 1/3, " ", 100, " ", 1e15, " ", 123456789012345678)
  f:close()
  local g = assert(io.open(NAME2))
  local s = g:read("a")
  g:close()
  return s
end)
P("closed ops", function()
  local f = assert(io.open(NAME))
  f:close()
  return pcall(f.read, f), pcall(f.write, f, "x"), pcall(f.lines, f), pcall(f.seek, f), pcall(f.close, f), io.type(f)
end)
P("read formats", function()
  local f = assert(io.open(NAME))
  local a = {f:read("l", "L", 3, 0, "n", "n", "n", "n", "n")}
  local b = {f:read("n", "n", "n", "n")}
  local c = {f:read("l")}
  local d = {f:read("a")}
  local e = {f:read("a")}
  local g = {f:read("l")}
  local h = {f:read(0)}
  local i = {f:read(5)}
  f:close()
  local function j(t) local o = {} for k = 1, #t do o[k] = show(t[k]) end return table.concat(o, ";") .. "#" .. #t end
  return j(a), j(b), j(c), j(d), j(e), j(g), j(h), j(i)
end)
P("read star formats", function()
  local f = assert(io.open(NAME))
  local r = {f:read("*l", "*L", "*n")}
  f:close()
  return #r, r[1], r[2], r[3]
end)
P("read bad format", function() local f = assert(io.open(NAME)) return f:read("x") end)
P("read bad format2", function() local f = assert(io.open(NAME)) return f:read({}) end)
P("read float count", function() local f = assert(io.open(NAME)) return f:read(1.5) end)
P("read float count2", function() local f = assert(io.open(NAME)) return f:read(2.0) end)
P("lines", function()
  local t = {}
  for l in io.lines(NAME) do t[#t + 1] = l end
  return #t, t[1], t[3]
end)
P("lines L", function()
  local t = {}
  for l in io.lines(NAME, "L") do t[#t + 1] = l end
  return #t, t[1], t[#t]
end)
P("lines formats", function()
  local t = {}
  for a, b in io.lines(NAME, 4, "n") do t[#t + 1] = show(a, b) end
  return table.concat(t, "|")
end)
P("lines missing", function() for l in io.lines(NAME .. "nope") do end end)
P("lines bad fmt", function() for l in io.lines(NAME, "x") do end end)
P("lines 4 results", function() local a, b, c, d = io.lines(NAME) return type(a), b, c, io.type(d) end)
P("file lines", function()
  local f = assert(io.open(NAME))
  local t = {}
  for l in f:lines() do t[#t + 1] = l end
  local after = io.type(f)
  local more = f:read("a")
  f:close()
  return #t, after, more
end)
P("lines after close", function()
  local f = assert(io.open(NAME))
  local it = f:lines()
  f:close()
  return it()
end)
P("lines toomany", function() return io.lines(NAME, table.unpack((function() local t = {} for i = 1, 251 do t[i] = "l" end return t end)())) end)
P("seek", function()
  local f = assert(io.open(NAME))
  local a = f:seek("end")
  local b = f:seek("set", 5)
  local c = f:read(3)
  local d = f:seek()
  local e = f:seek("cur", -2)
  local g = f:read(2)
  local h = {f:seek("set", -1)}
  local i = {pcall(f.seek, f, "bad")}
  local j = {pcall(f.seek, f, "set", 1.5)}
  f:close()
  return a, b, c, d, e, g, show(table.unpack(h)), show(table.unpack(i)), show(table.unpack(j))
end)
P("setvbuf", function()
  local f = assert(io.open(NAME))
  local a, b, c = f:setvbuf("no"), f:setvbuf("full", 1024), f:setvbuf("line")
  local d = {pcall(f.setvbuf, f, "bad")}
  local e = {pcall(f.setvbuf, f)}
  f:close()
  return a, b, c, show(table.unpack(d)), show(table.unpack(e))
end)
P("flush", function() local f = assert(io.open(NAME, "a")) local r = f:flush() f:close() return r end)
P("append", function()
  local f = assert(io.open(NAME, "a"))
  f:write("\nappended")
  f:close()
  local g = assert(io.open(NAME))
  local s = g:read("a")
  g:close()
  return #s, s:sub(-20)
end)
P("r+ write then read", function()
  local f = assert(io.open(NAME, "r+"))
  f:write("LINE")
  f:seek("set", 0)
  local s = f:read("l")
  f:close()
  return s
end)
P("w+ ", function()
  local f = assert(io.open(NAME2, "w+"))
  f:write("abc\n123\n")
  f:seek("set")
  local a, b = f:read("l", "n")
  f:close()
  return a, b
end)
P("write to read-only", function()
  local f = assert(io.open(NAME))
  local r = {f:write("x")}
  f:close()
  return show(table.unpack(r))
end)
P("read from write-only", function()
  local f = assert(io.open(NAME2, "w"))
  local r = {f:read("l")}
  local r2 = {f:read("a")}
  f:close()
  return show(table.unpack(r)), show(table.unpack(r2))
end)
P("io.type", function() return io.type(nil), io.type(1), io.type(io.stdout), io.type({}) end)
P("io.type none", io.type)
P("tostring closed", function() local f = assert(io.open(NAME)) f:close() return tostring(f) end)
P("tostring open", function() local f = assert(io.open(NAME)) local s = tostring(f) f:close() return s:match("^file %(0x%x+%)$") ~= nil end)
P("std close", function() return io.stdout:close() end)
P("std close io.close", function() return io.close(io.stderr) end)
P("std type after", function() return io.type(io.stdout), io.type(io.stdin), io.type(io.stderr) end)
P("input/output", function()
  local old_in, old_out = io.input(), io.output()
  local o = io.output(NAME2)
  io.write("via io.write ", 42, "\n", 1.5)
  local w = io.write("x")
  io.close()
  local closed = {pcall(io.write, "y")}
  io.output(old_out)
  io.input(NAME2)
  local l1 = io.read()
  local l2 = io.read("n")
  local l3 = io.read("a")
  local l4 = io.read()
  io.close(io.input())
  local closed_in = {pcall(io.read)}
  local lines_closed = {pcall(io.lines)}
  io.input(old_in)
  return io.type(o), w == o, show(table.unpack(closed)), l1, l2, l3, l4, show(table.unpack(closed_in)), show(table.unpack(lines_closed))
end)
P("io.output missing", function() return io.output("") end)
P("io.input missing", function() return io.input(NAME .. "nope") end)
P("io.input bad", function() return io.input({}) end)
P("io.input closed", function() local f = assert(io.open(NAME)) f:close() return io.input(f) end)
P("io.output returns", function() return io.output() == io.stdout, io.input() == io.stdin end)
P("method on bad self", function() return io.stdout.write(1) end)
P("method on bad self2", function() local f = assert(io.open(NAME)) f:close() return f.read({}) end)
P("write bad arg", function() local f = assert(io.open(NAME2, "w")) local r = {pcall(f.write, f, "a", {})} f:close() return show(table.unpack(r)) end)
P("write bad arg method", function() local f = assert(io.open(NAME2, "w")) local ok, e = pcall(function() return f:write({}) end) f:close() return ok, e end)
P("io.write bad", function() return io.write(true) end)
P("close twice", function() local f = assert(io.open(NAME)) f:close() return pcall(f.close, f) end)
P("to-be-closed", function()
  local f
  do
    local g <close> = assert(io.open(NAME))
    f = g
  end
  return io.type(f)
end)
P("read number edge", function()
  local f = assert(io.open(NAME2, "w"))
  f:write("  0x  -  ++1 1.5e+  0xA.8p1 00012 .e5 1e5e 9223372036854775808 ", string.rep("1", 210), " 5")
  f:close()
  f = assert(io.open(NAME2))
  local t = {}
  for i = 1, 12 do local v = f:read("n") t[#t + 1] = show(v) if v == nil then t[#t + 1] = show(f:read(1)) end end
  f:close()
  return table.concat(t, " ")
end)
P("empty file", function()
  local f = assert(io.open(NAME2, "w")) f:close()
  f = assert(io.open(NAME2))
  local r = {f:read("l")} local r2 = {f:read("a")} local r3 = {f:read(0)} local r4 = {f:read("n")} local r5 = {f:read("L")}
  f:close()
  return show(table.unpack(r)), show(table.unpack(r2)), show(table.unpack(r3)), show(table.unpack(r4)), show(table.unpack(r5))
end)
P("long line", function()
  local f = assert(io.open(NAME2, "w")) f:write(string.rep("ab", 10000), "\n", string.rep("c", 5000)) f:close()
  f = assert(io.open(NAME2))
  local a, b, c = f:read("L", "l", "l")
  f:close()
  return #a, #b, c
end)
P("crlf", function()
  local f = assert(io.open(NAME2, "w")) f:write("a\r\nb\n\nc") f:close()
  local t = {} for l in io.lines(NAME2) do t[#t + 1] = string.format("%q", l) end
  return table.concat(t, ",")
end)
P("zero bytes", function()
  local f = assert(io.open(NAME2, "w")) f:write("a\0b\n\0") f:close()
  local t = {} for l in io.lines(NAME2, "L") do t[#t + 1] = string.format("%q", l) end
  return table.concat(t, ",")
end)
P("io.read default (stdin closed?)", function() return io.type(io.stdin) end)
P("metatable", function()
  local f = assert(io.open(NAME))
  local m = getmetatable(f)
  local r = {type(m), m.__name, type(m.__index), type(m.__close), type(m.__tostring)}
  f:close()
  local names = {} for k in pairs(m.__index) do names[#names + 1] = k end table.sort(names)
  return show(table.unpack(r)), table.concat(names, " ")
end)
P("io fields", function()
  -- (`popen` and `tmpfile` are left out by design.)
  local names = {} for k in pairs(io) do if k ~= "popen" and k ~= "tmpfile" then names[#names + 1] = k end end table.sort(names)
  return table.concat(names, " ")
end)
P("lines with closed default input", function()
  local old = io.input()
  local f = assert(io.open(NAME)) io.input(f) f:close()
  local r = {pcall(io.lines)}
  io.input(old)
  return show(table.unpack(r))
end)
return table.concat(out, "\n")
