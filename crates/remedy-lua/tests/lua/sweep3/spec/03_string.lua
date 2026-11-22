-- case: format integer specifiers
local r = {}
for _, f in ipairs{"%d", "%5d", "%-5d|", "%05d", "%+d", "% d", "%.3d", "%x", "%X", "%#x", "%o", "%#o", "%i", "%u", "%c", "%5.2d", "%-+8.3i|"} do
  for _, v in ipairs{0, 7, -7, 255, math.maxinteger, math.mininteger} do
    if f ~= "%c" or (v >= 0 and v < 256) then r[#r+1] = string.format(f, v) end
  end
end
return table.concat(r, " ")
-- case: format float specifiers
local r = {}
for _, f in ipairs{"%f", "%.0f", "%.3f", "%10.2f", "%-10.2f|", "%+f", "%e", "%.0e", "%E", "%g", "%.0g", "%.10g", "%#g", "%G", "%#.0f", "%08.3f", "% .2e", "%.14g", "%5.1g"} do
  for _, v in ipairs{0.0, -0.0, 1.5, -2.25, 1e10, 1e-10, 123456.789, 1/3, 1e300, 5e-324, 100} do
    r[#r+1] = string.format(f, v)
  end
end
return table.concat(r, " ")
-- case: format inf and nan
return string.format("%f %e %g %.3f %5.1f", 1/0, -1/0, 1/0, -1/0, 1/0)
-- case: format a hex float
return string.format("%a %A", 1.0, 0.5), string.format("%a", 0.0), string.format("%a", -1.5)
-- case: format s and q
return string.format("%s|%10s|%-10s|%.2s|%5.1s", "abc", "abc", "abc", "abc", "abc"), string.format("%s %s %s", 1, 1.5, true), string.format("%q", "a\nb\0c\"\\\r\t\1272"), string.format("%q", 1/0), string.format("%q", -1/0), string.format("%q", math.mininteger), string.format("%q", 1.5), string.format("%q", 255), string.format("%q", 0/0)
-- case: format errors
return E(string.format, "%d", 1.5), E(string.format, "%d", "1.5"), string.format("%d", "10"), string.format("%d", 3.0), E(string.format, "%z", 1), E(string.format, "%", 1), E(string.format, "%d"), E(string.format, "%10.123f", 1), E(string.format, "%123d", 1), E(string.format, "%q", {}), E(string.format, "%10q", "x"), E(string.format, "%c", "x"), string.format("%%"), E(string.format, "%s"), E(string.format, "%-#5.3x", 1)
-- case: format s with tostring meta and embedded zero
local t = setmetatable({}, {__tostring = function() return "T!" end})
return string.format("%s", t), string.format("%s", "a\0b"), E(string.format, "%10s", "a\0b"), string.format("%.1s", "a\0b")
-- case: format x of negative
return string.format("%x", -1), string.format("%X", math.mininteger), string.format("%o", -1), string.format("%5.3x", 10)
-- case: sub byte char
local s = "hello"
return s:sub(2), s:sub(-3), s:sub(0), s:sub(10), s:sub(3, 2), s:sub(-100, 100), s:sub(math.mininteger, math.maxinteger), s:byte(), s:byte(-1), s:byte(1, -1), s:byte(10), string.char(), string.char(72, 105), E(string.char, 256), E(string.char, -1), s:sub(2.0, 3)
-- case: rep upper lower reverse len
return ("ab"):rep(3, ","), ("ab"):rep(0), ("ab"):rep(-1), ("x"):rep(1, "SEP"), ("aBc"):upper(), ("aBc"):lower(), ("abc"):reverse(), (""):reverse(), ("a\0b"):len(), E(string.rep), E(string.sub, "x", "a")
-- case: find plain and init
local s = "a.b.c"
return s:find("."), s:find(".", 1, true), s:find("b", -3), s:find("", 10), s:find("", 6), s:find("", 5), s:find("x"), s:find(".", -100, true), s:find("c", 100), ("abc"):find("", 4), ("abc"):find("", 5)
-- case: match captures
return ("hello world"):match("(%w+) (%w+)"), ("key=val"):match("(%w+)=(%w*)"), ("abc"):match("()b()"), ("  x"):match("^%s*(.-)$"), ("[[x]]"):match("%[(%b[])%]"), ("THE (quick) fox"):find("%((%a+)%)"), ("f(a(b)c)"):match("%b()")
-- case: gsub forms
return ("hello world"):gsub("o", "0"), ("hello"):gsub("", "-"), ("abc"):gsub("%w", "%0%0"), ("abc"):gsub("(a)(b)", "%2%1"), ("abc"):gsub("b", {b = "B"}), ("abc"):gsub("%w", {a = false, b = "X"}), ("abc"):gsub("%w", function(c) if c == "b" then return nil end return c:upper() end), ("aaa"):gsub("a", "b", 2), ("abc"):gsub(".", "%%"), E(string.gsub, "abc", "a", "%2"), E(string.gsub, "abc", "a", "%"), ("x"):gsub("x", "%1")
-- case: gsub repl type errors
return E(string.gsub, "abc", "a", true), E(string.gsub, "abc", "a", function() return {} end), ("abc"):gsub("a", 5), ("abc"):gsub("a", 1.5), ("abc"):gsub("a", {a = 2})
-- case: gmatch
local r = {}
for k, v in ("a=1, b=2"):gmatch("(%w+)=(%w+)") do r[#r+1] = k .. v end
for w in ("one two"):gmatch("%a+") do r[#r+1] = w end
for p in ("abc"):gmatch("()") do r[#r+1] = p end
for w in ("abc"):gmatch("^a") do r[#r+1] = "anch:" .. w end
for w in ("hello"):gmatch("l", 4) do r[#r+1] = "init" .. w end
return table.concat(r, ",")
-- case: patterns classes and frontier
return ("THE (quick) fox"):gsub("%f[%a]%a+", "W"), ("x1 y2"):gsub("%d", "#"), ("a.b"):gsub("%.", "!"), ("\0a\0"):gsub("%z", "Z"), ("áb"):gsub("[\128-\255]", "?"), ("a-b"):gsub("[a-]", "X"), ("a]b"):gsub("[]]", "X"), ("^a"):gsub("[%^]", "X"), ("abc"):find("[^%a]"), ("aaa"):match("a-b"), ("aaab"):match("a-b"), ("ab"):match("a?b"), ("b"):match("a?b")
-- case: pattern errors
return E(string.find, "a", "[a"), E(string.find, "a", "%"), E(string.find, "a", "(()"), E(string.find, "a", "%b"), E(string.find, "a", "%f"), E(string.match, "a", ")"), E(string.find, "a", "%1"), E(string.find, "a", "[%"), E(string.rep, "x", 1e10)
-- case: backrefs and position captures
return ("hello"):find("(l)%1"), ("abab"):match("(ab)%1"), ("xyx"):match("(x)(y)%1"), string.find("abc", "(b)()"), ("a"):match("()a()")
-- case: string methods on numbers
local n = 123
return n .. "", ("x"):rep(2), (5):type(), E(function() return (5):len() end)
-- case: string comparison
return "a" < "b", "a" < "aa", "" < "a", "a\0b" < "a\0c", "Z" < "a", "\255" > "a", "abc" <= "abc", E(function() return "a" < 1 end)
-- case: pack basic
local s = string.pack("i4", 100)
return #s, string.unpack("i4", s), string.packsize("i4i8"), string.pack(">I2", 258):byte(1, -1), string.unpack("<h", "\255\255"), string.unpack("z", "abc\0def"), string.unpack("s1", "\3abcd"), string.pack("s2", "hi"):byte(1, -1)
-- case: pack alignment and options
return string.packsize("!8 i1 i8"), string.packsize("i1 !4 i8"), string.packsize("<i3"), string.pack("b B", -1, 255):byte(1, -1), string.pack("f", 1.5):len(), string.pack("d", 1.5):len(), string.packsize("n j J T"), string.pack("x"):byte(), E(string.packsize, "s"), E(string.packsize, "z"), E(string.pack, "i17", 1), E(string.pack, "i0", 1)
-- case: pack overflow and errors
return E(string.pack, "i1", 128), E(string.pack, "I1", 256), E(string.pack, "I1", -1), string.pack("i1", -128):byte(), E(string.pack, "c2", "abc"), string.pack("c3", "a"):byte(1, -1), E(string.pack, "z", "a\0b"), E(string.unpack, "i4", "ab"), E(string.unpack, "i4", "abcd", 10), string.unpack("i4", "abcdabcd", -4), E(string.pack, "i", 1.5), string.pack("i", "10"):len(), E(string.pack, "d", "x")
-- case: unpack big ints
return string.unpack("<i9", ("\255"):rep(9)), E(string.unpack, "<i9", ("\0"):rep(8) .. "\1"), string.unpack("<I9", ("\255"):rep(8) .. "\0"), string.unpack(">i16", ("\255"):rep(16)), E(string.unpack, ">i16", "\0" .. ("\255"):rep(15)), string.pack("<i12", -2):byte(1, -1)
-- case: pack X alignment
return string.packsize("i1 Xi4"), string.packsize("!4 i1 Xi8"), E(string.packsize, "X"), E(string.packsize, "Xz"), string.pack("i1 Xi2 i2", 1, 2):byte(1, -1), string.unpack("i1 Xi2 i2", "\1\0\2\0")
-- case: utf8 basics
return utf8.char(72, 228, 8364, 128512), utf8.charpattern, utf8.len("häll€"), utf8.len("\255"), utf8.len("abc", 4), utf8.len("abc", 5) == nil, E(utf8.len, "abc", 5), utf8.codepoint("häll", 1, -1), utf8.offset("häll", 3), utf8.offset("häll", -1), utf8.offset("abc", 0, 2), E(utf8.offset, "ä", 1, 2), utf8.offset("abc", 5), utf8.offset("abc", 4)
-- case: utf8 codes and errors
local r = {}
for p, c in utf8.codes("aä€") do r[#r+1] = p .. ":" .. c end
local ok, e = pcall(function() for p, c in utf8.codes("a\255") do end end)
local ok2, e2 = pcall(function() for p, c in utf8.codes("a\xF4\x90\x80\x80") do end end)
local r3 = {}
for p, c in utf8.codes("\xF4\x90\x80\x80", true) do r3[#r3+1] = c end
return table.concat(r, ","), e, e2, r3[1], utf8.len("\xF4\x90\x80\x80"), utf8.len("\xF4\x90\x80\x80", 1, -1, true), utf8.char(0x7FFFFFFF):len(), E(utf8.char, 0x80000000), E(utf8.codepoint, "\xED\xA0\x80"), utf8.codepoint("\xED\xA0\x80", 1, 1, true)
-- case: utf8 offset more
local s = "a\u{E4}\u{20AC}b"
local r = {}
for n = -5, 5 do r[#r+1] = tostring(utf8.offset(s, n)) end
for i = 1, #s + 1 do r[#r+1] = tostring(utf8.offset(s, 0, i)) end
return table.concat(r, ","), E(utf8.offset, s, 1, 3), E(utf8.offset, s, 1, 10)
-- case: utf8 codes with continuation byte at end
return E(function() for _ in utf8.codes("ab\x80") do end end), E(function() for _ in utf8.codes("\xC0\x80") do end end), utf8.len("\xC0\x80"), utf8.len("abc", -1), utf8.len("", 1), E(utf8.len, "", 2)
-- case: string.len and # coercion
return string.len(123), #tostring(12.0), string.upper(12), E(string.len)
-- case: string format g precision
return string.format("%.3g|%.20g|%g|%g|%g", 0.0001234, 0.1, 1e-5, 123456789, 1e15)
-- case: tostring int vs float keys concat
return 1 .. 2, 1.0 .. "", math.mininteger .. "", 1e308 * 10 .. "", -(0/0) ~= -(0/0)
-- case: string.format with many args and positional count
return string.format("%s %s", 1, 2, 3), select("#", string.format("")), string.format("%5s%%", "a")
-- case: string.byte negative ranges
return ("abcdef"):byte(-3, -1), ("abc"):byte(0), ("abc"):byte(2, 1), ("abc"):byte(-10, 2)
-- case: gsub anchored and max n
return ("aaa"):gsub("^a", "b"), ("aaa"):gsub("a", "b", 0), ("aaa"):gsub("a", "b", -1), ("aaa"):gsub("a", "b", 1.0), E(string.gsub, "aaa", "a", "b", 1.5), ("abc"):gsub("$", "!"), ("a\nb"):gsub("^", ">")
-- case: find special chars in plain
return ("a+b"):find("+", 1, true), ("a+b"):find("a+b"), ("a%b"):find("%", 1, true), ("x"):find("x", 1, false)
