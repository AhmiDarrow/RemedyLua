-- case: sub edge indices
local s = "hello"
return s:sub(0), s:sub(-100, 2), s:sub(2, 100), s:sub(math.mininteger, math.maxinteger), s:sub(math.maxinteger), s:sub(-1, -2), s:sub(3, -3), s:sub(6), s:sub(5, 5), ("").sub("", 1, 1), s:sub(2.0, 3), pcall(s.sub, s, 1.5), pcall(s.sub, s, "2"), pcall(s.sub, s, "x")
-- case: byte and char edges
return ("abc"):byte(-1), ("abc"):byte(10), ("abc"):byte(0), select("#", ("abc"):byte(2, 1)), ("abc"):byte(-10, 10), string.char(), pcall(string.char, 256), pcall(string.char, -1), string.char(0, 255) == "\0\255", pcall(string.char, "65"), pcall(string.char, 65.5)
-- case: rep edges
return ("ab"):rep(0), ("ab"):rep(-1), ("ab"):rep(3, ""), (""):rep(5, ","), ("x"):rep(1, ","), ("a"):rep(2, "--"), pcall(string.rep, "x", 1e10), pcall(string.rep), pcall(string.rep, "x", 2.5)
-- case: rep huge separator overflow
return pcall(string.rep, "x", math.maxinteger), pcall(string.rep, "x", 2^40, "y")
-- case: upper lower with bytes
return ("\xe9Ab1"):upper(), ("ÀBC"):lower(), string.upper(12), string.len(12.5)
-- case: len and reverse
return #"\0\0", ("a\0b"):reverse() == "b\0a", string.reverse(""), pcall(string.len), string.len(123)
-- case: format integer specifiers
return string.format("%d|%i|%5d|%-5d|%05d|%+d|% d|%.3d|%x|%X|%#o|%o|%c", 42, -42, 7, 7, 7, 7, 7, 7, 255, 255, 8, 8, 65)
-- case: format integers from floats
return string.format("%d", 3.0), pcall(string.format, "%d", 3.5), pcall(string.format, "%d", "3"), pcall(string.format, "%d", "3.0"), pcall(string.format, "%d", "x"), pcall(string.format, "%x", -1), string.format("%d", math.mininteger), string.format("%x", math.mininteger)
-- case: format floats f e g
return string.format("%f|%.0f|%.1f|%10.4f|%-10.2f|%+.2f|%e|%.0e|%E|%g|%.3g|%#g|%g|%g|%g", 1.5, 2.5, 0.05, math.pi, -1.5, 3, 12345.6789, 15, 1e-10, 1e20, 1234567, 1.0, 100000, 1000000, 1e-4)
-- case: format g edge cases
return string.format("%g %g %g %g %g %g %g %.0g %.20g %#.3g %g", 0.0, -0.0, 1e-5, 123456789, 0.1, 1/3, 2^63, 0.5, 0.1, 1, 5e-324)
-- case: format inf nan
return string.format("%f %e %g %5.1f %-6g| %.3f", 1/0, -1/0, 1/0, 1/0, -1/0, 1/0)
-- case: format a hex floats
return string.format("%a", 1.0), string.format("%a", 0.5), string.format("%a", -0.1), string.format("%A", 255.5), string.format("%a", 0.0), string.format("%a", 1/0)
-- case: format s
return string.format("%s|%s|%s|%s|%5s|%-5s|%.1s|%s", 1, 1.5, true, nil, "ab", "ab", "xyz", setmetatable({}, {__tostring = function() return "TT" end}))
-- case: format s with embedded zero
return pcall(string.format, "%10s", "a\0b"), string.format("%s", "a\0b") == "a\0b", #string.format("%s", "a\0b")
-- case: format q
return string.format("%q", "a\nb\"c\\\0d\1\127\200\r"), string.format("%q", 1), string.format("%q", 1.5), string.format("%q", 1/0), string.format("%q", -1/0), string.format("%q", 0/0), string.format("%q", math.mininteger), string.format("%q", true), string.format("%q", nil), pcall(string.format, "%q", {}), string.format("%q", 0.1), string.format("%q", "\0001")
-- case: format q digit after escape
return string.format("%q", "\0" .. "9"), string.format("%q", "\1" .. "a"), string.format("%q", "\r\n"), string.format("%q", "\t")
-- case: format q with modifiers
return pcall(string.format, "%10q", "x"), string.format("%q", 2^63), string.format("%q", -0.0), string.format("%q", 1e100)
-- case: format errors
return pcall(string.format, "%y", 1), pcall(string.format, "%", 1), pcall(string.format, "%d"), pcall(string.format, "%100d", 1), pcall(string.format, "%.100f", 1), pcall(string.format, "%-+ #0d", 1), pcall(string.format, "%10.3q", "x"), pcall(string.format, "%#x", 1)
-- case: format flags combos
return string.format("%-+5d|%+05d|% 05d|%#5x|%#X|%-#8o|%08.3f|%+.0e|%#.0f|% g", 3, 3, 3, 255, 255, 8, 3.14159, 12345, 3, 2)
-- case: format width 99 boundary
return #string.format("%99d", 1), pcall(string.format, "%099d", 1), #string.format("%.99f", 1)
-- case: format with too many args fine
return string.format("%d", 1, 2, 3), string.format("no specs", 1), string.format("%%")
-- case: format c with zero and width
return string.format("%c", 0) == "\0", string.format("%5c|", 65), string.format("%-3c|", 66)
-- case: format i precision
return string.format("%.0d", 0), string.format("%5.3x", 10), string.format("%.10d", -5)
-- case: find special cases
return ("abc"):find(""), ("abc"):find("", 4), ("abc"):find("", 5), ("abc"):find("", -1), ("abc"):find("c", -1), ("a.b"):find(".", 1, true), ("a+b"):find("+", 1, true), ("abc"):find("b", -10), ("abc"):find("b", 100), ("a^b"):find("^b"), ("a^b"):find("%^b"), ("abc"):find("[b-]")
-- case: find init float and string
return ("abc"):find("c", 2.0), pcall(string.find, "abc", "c", 1.5), ("abc"):find("c", "2"), ("abc"):find("b", nil)
-- case: match anchors and init
return ("hello"):match("^h", 1), ("hello"):match("^e", 2), ("hello"):match("o$"), ("hello$"):match("o%$$"), ("a$b"):match("$b"), ("hello"):match(".", -2), ("hello"):match("()", 10)
-- case: pattern classes
local all = {}
for i = 0, 255 do all[#all + 1] = string.char(i) end
local s = table.concat(all)
local r = {}
for _, c in ipairs{"%a", "%c", "%d", "%g", "%l", "%p", "%s", "%u", "%w", "%x", "%A", "%S", "%W"} do r[#r + 1] = c .. "=" .. select(2, s:gsub(c, "")) end
return table.concat(r, " ")
-- case: pattern sets
return ("a-z"):match("[a%-z]+"), ("]x"):match("[]]"), ("a]"):match("[^]]"), ("x-"):match("[x-]+"), ("^a"):match("[%^a]+"), ("abc"):match("[^%a]"), ("a5_"):match("[%w_]+"), ("%"):match("[%%]"), ("b"):match("[a-c]"), ("-"):match("[a-]"), ("[]"):match("[[]"), ("z"):match("[^a-y]")
-- case: pattern quantifiers
return ("aaa"):match("a-"), ("aaa"):match("a-$"), ("aaab"):match("a*b"), ("b"):match("a?b"), ("aab"):match("a+?"), ("xay"):match("x(.?)y"), ("<<a>>"):match("<(.-)>"), ("<<a>>"):match("<(.*)>")
-- case: balanced pattern
return ("(a(b)c)"):match("%b()"), ("[[x]]"):match("%b[]"), ("(("):match("%b()"), ("x)y(z"):match("%b)("), ("abba"):match("%bab"), ("aaa"):match("%baa")
-- case: position captures and nested
return ("hello"):match("()ll()"), ("abc"):match("((a)(b))"), ("key=val"):match("(%w+)=(%w*)"), ("x"):match("()"), ("abc"):find("(b)()")
-- case: too many captures
return pcall(string.match, string.rep("a", 40), string.rep("(a)", 33)), select("#", string.match(string.rep("a", 32), string.rep("(a)", 32)))
-- case: pattern too complex recursion
local ok, e = pcall(string.match, string.rep("a", 300000), string.rep("a?", 300000) .. string.rep("a", 300000))
return ok
-- case: gsub replacement string forms
return ("abc"):gsub("b", "%0%0"), ("abc"):gsub("(b)", "[%1]"), ("abc"):gsub("b", "%%"), pcall(string.gsub, "abc", "b", "%2"), pcall(string.gsub, "abc", "b", "%x"), ("abc"):gsub("b", "%1"), ("abc"):gsub("", "-"), ("abc"):gsub(".", {a = 1, b = true}), pcall(string.gsub, "abc", "b", "%")
-- case: gsub table and function replacement values
return ("abc"):gsub("%w", {a = "A", b = false, c = nil}), ("abc"):gsub("%w", function(c) if c == "b" then return nil end return 1.5 end), pcall(string.gsub, "abc", "%w", function() return {} end), pcall(string.gsub, "abc", "%w", {a = {}}), ("abc"):gsub("(a)(b)", function(x, y) return y .. x end)
-- case: gsub max n
return ("aaaa"):gsub("a", "b", 0), ("aaaa"):gsub("a", "b", -1), ("aaaa"):gsub("a", "b", 2.0), pcall(string.gsub, "aaaa", "a", "b", 1.5), ("aaaa"):gsub("a", "b", 100)
-- case: gsub with anchor
return ("aaa"):gsub("^a", "b"), ("hello world"):gsub("^%s*", ""), ("abc"):gsub("$", "!"), ("abc"):gsub("^", ">")
-- case: gsub bad replacement type
return pcall(string.gsub, "abc", "b", true), pcall(string.gsub, "abc", "b"), ("abc"):gsub("b", 5)
-- case: gsub empty match advancing
return ("abc"):gsub("x*", "-"), ("a,b,,c"):gsub(",*", "|"), ("hello"):gsub("l*", "L")
-- case: gmatch
local r = {}
for a, b in ("k1=v1;k2=v2"):gmatch("(%w+)=(%w+)") do r[#r + 1] = a .. b end
for x in ("abc"):gmatch("") do r[#r + 1] = "e" end
for x in ("aXbX"):gmatch("^a") do r[#r + 1] = x end
for p in ("abc"):gmatch("()b") do r[#r + 1] = p end
return table.concat(r, ",")
-- case: gmatch with init
local r = {}
for w in ("one two three"):gmatch("%a+", 5) do r[#r + 1] = w end
for w in ("one two three"):gmatch("%a+", -5) do r[#r + 1] = w end
for w in ("abc"):gmatch(".", 10) do r[#r + 1] = w end
return table.concat(r, ",")
-- case: gmatch anchor is literal in 5.4
local n = 0
for x in ("^a^a"):gmatch("^a") do n = n + 1 end
return n
-- case: pack basic formats
return string.pack("b", -1) == "\255", string.pack("B", 255), string.pack("<h", 1) == "\1\0", string.pack(">h", 1) == "\0\1", string.pack("<H", 65535) == "\255\255", #string.pack("i", 1), #string.pack("j", 1), #string.pack("T", 1), #string.pack("f", 1), #string.pack("d", 1), #string.pack("n", 1)
-- case: pack integer sizes and overflow
return pcall(string.pack, "i1", 128), pcall(string.pack, "i1", -129), pcall(string.pack, "I1", 256), pcall(string.pack, "I1", -1), #string.pack("i16", 1), string.unpack("<i16", string.pack("<i16", -2)), pcall(string.pack, "i17", 1), pcall(string.pack, "i0", 1), string.unpack("<I3", "\1\2\3")
-- case: unpack integer too large for i16
return pcall(string.unpack, "<i9", "\0\0\0\0\0\0\0\0\1"), string.unpack("<i9", "\255\255\255\255\255\255\255\255\255"), pcall(string.unpack, "<I9", "\0\0\0\0\0\0\0\128\0"), string.unpack("<i9", string.pack("<i9", math.mininteger))
-- case: pack strings
return string.pack("z", "abc") == "abc\0", pcall(string.pack, "z", "a\0b"), string.pack("s1", "ab") == "\2ab", pcall(string.pack, "s1", string.rep("x", 256)), string.pack("c5", "ab") == "ab\0\0\0", pcall(string.pack, "c2", "abc"), string.pack("c0", ""), pcall(string.pack, "c", "a")
-- case: unpack strings
return string.unpack("z", "abc\0def"), pcall(string.unpack, "z", "abc"), string.unpack("s1", "\3abcd"), pcall(string.unpack, "s1", "\5ab"), string.unpack("c2", "abcd"), pcall(string.unpack, "c5", "ab")
-- case: pack alignment
return #string.pack("!4 b i4", 1, 2), #string.pack("!8 b d", 1, 2), #string.pack("! b i8", 1, 2), #string.pack("b Xi4 b", 1, 2), #string.pack("!4 b Xi8 b", 1, 2), #string.pack("<!2 b h b i", 1, 2, 3, 4), string.packsize("!8 b Xd"), string.packsize("!2 i3 b")
-- case: pack padding and spaces
return string.pack("bxb", 1, 2) == "\1\0\2", string.pack(" b  b ", 1, 2) == "\1\2", #string.pack("xxx"), string.packsize("bxXh"), pcall(string.pack, "q", 1)
-- case: pack float and endianness
return string.unpack(">d", string.pack(">d", -1.5)), string.unpack("<f", string.pack("<f", 0.5)), string.pack(">i4", 1) == "\0\0\0\1", string.pack("=i2", 1) == string.pack("<i2", 1), string.unpack("<n", string.pack("<n", 1/0)), string.unpack("<j", string.pack("<j", math.mininteger))
-- case: pack float from integer and integer from float
return string.unpack("d", string.pack("d", 3)), pcall(string.pack, "i4", 1.5), string.unpack("i4", string.pack("i4", 3.0)), pcall(string.pack, "i4", "12"), pcall(string.pack, "d", "x"), pcall(string.pack, "i4", "x")
-- case: unpack position
return string.unpack("b", "\1\2", 2), string.unpack("b", "\1\2", -1), pcall(string.unpack, "b", "\1\2", 3), pcall(string.unpack, "b", "\1\2", 4), string.unpack("", "abc", 4), pcall(string.unpack, "b", "\1", 0), pcall(string.unpack, "b", "", -5), select("#", string.unpack("bb", "\1\2"))
-- case: packsize
return string.packsize("i4i8"), string.packsize(""), string.packsize("c10"), pcall(string.packsize, "s"), pcall(string.packsize, "z"), string.packsize("<!8 b d")
-- case: pack missing args
return pcall(string.pack, "i4"), pcall(string.pack, "i4 i4", 1), pcall(string.pack), pcall(string.unpack, "i4", "ab")
-- case: string methods via metatable index
local s = "abc"
return s:len(), s:upper(), ("x"):rep(3), s.len, pcall(function() return s:nosuch() end)
-- case: string comparison with locale bytes
return "a" < "\128", "\127" < "\128", "\255" > "\254", "abc\0" > "abc"
-- case: string.format percent s uses tostring metamethod and __name
local u = setmetatable({}, {__name = "N"})
return (string.format("%s", u):match("^N: ")), pcall(string.format, "%s")
-- case: string.len with numbers coerced
return ("x"):rep(3):len(), string.upper(1.5), string.sub(12345, 2, 3), string.byte(1), string.reverse(12)
-- case: tostring vs format of floats
return string.format("%.14g", 0.1), string.format("%s", 0.1), string.format("%5.1f", -0.04), string.format("%.3f", 2.0005), string.format("%.2f", 2.675)
