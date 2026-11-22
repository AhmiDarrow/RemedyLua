-- case: string.byte char len sub (6.4)
return ("hello"):byte(1, -1), string.char(72, 105), #"\0a\0", ("hello"):sub(2, -2), ("hello"):sub(-3), ("hello"):sub(0), ("hello"):sub(10), ("hello"):sub(3, 2)
-- case: upper lower rep reverse
return ("aBc"):upper(), ("aBc"):lower(), ("ab"):rep(3, "-"), ("x"):rep(0), ("abc"):reverse(), ("ab"):rep(1)
-- case: find plain and patterns (6.4.1)
local s = "hello world from lua"
return s:find("o"), s:find("o", 6), s:find("l+"), s:find("xyz"), s:find("o w", 1, true), s:find("", 30), s:find("", 21), s:find("^hello"), s:find("^world")
-- case: match captures
return ("key = value"):match("(%w+)%s*=%s*(%w+)"), ("2024-01-15"):match("(%d+)-(%d+)-(%d+)"), ("  trim  "):match("^%s*(.-)%s*$"), ("abc"):match("()b()"), ("hello"):match(".-(l+)")
-- case: gmatch
local out = {}
for w in ("one two  three"):gmatch("%a+") do out[#out + 1] = w end
for k, v in ("a=1, b=2"):gmatch("(%w+)=(%w+)") do out[#out + 1] = k .. v end
for p in ("abc"):gmatch("()") do out[#out + 1] = p end
return table.concat(out, "|")
-- case: gsub variants
return ("hello world"):gsub("o", "0"), ("hello"):gsub("", "-"), ("abc"):gsub("%w", "%0%0"), ("hello world"):gsub("(%w+)", "<%1>"), ("abc"):gsub("b", "%%"), ("hello world"):gsub("%w+", {hello = "HI"}), ("x y z"):gsub("%a", function(c) if c == "y" then return false end return c:upper() end), ("aaa"):gsub("a", "b", 2)
-- case: pattern classes and sets
return ("A1 b2_c3!"):gsub("%w", "."), ("A1 b2_c3!"):gsub("[%a_]", "#"), ("abc123"):gsub("[^%d]", ""), ("a.b-c"):gsub("[%.%-]", "/"), ("x=[1]"):match("%[(.-)%]"), ("THE (quick) fox"):find("%((%a+)%)"), ("f(a(b)c)d"):match("%b()"), ("THE (quick) fox"):gsub("%f[%a]%a+", "W")
-- case: back references
return ('say "hi" now'):match('(["\'])(.-)%1'), ("aXbXc"):find("(X).-%1")
-- case: pattern errors -- expect error
return ("abc"):find("[a")
-- case: string.format integers
return string.format("%d %5d %-5d| %05d %+d %x %X %o %#x %3.2d", 42, 42, 42, 42, 42, 255, 255, 8, 255, 7)
-- case: string.format floats
return string.format("%f %.2f %10.3f %-10.1f| %e %.3E %g %g %g %g %G", 3.14159, 2.5, 1/3, 2.25, 12345.678, 0.00012, 100000, 1e20, 0.0001, 1e-5, 1e-10)
-- case: string.format strings and q
return string.format("%s|%10s|%-10s|%.2s|%q|%q|%q", "abc", "right", "left", "truncate", 'a "q"\n\0z', 1/3, math.mininteger)
-- case: string.format misc
return string.format("%%|%c%c|%s|%s|%s", 76, 117, 1.0, 10, true), string.format("%a", 1.0), string.format("%5.1s|", "abc")
-- case: tostring of numbers
return tostring(1e15), tostring(1e16), tostring(0.1), tostring(-0.0), tostring(1/3), tostring(123456789012), tostring(2^63), tostring(1e300 * 1e10), tostring(3.0)
-- case: tonumber
return tonumber("10"), tonumber("  0x1F  "), tonumber("1e2"), tonumber("abc"), tonumber("10", 2), tonumber("ff", 16), tonumber("zz", 36), tonumber("8", 8), tonumber(""), tonumber("0x"), tonumber("1 2"), tonumber(nil)
-- case: string comparison and concat numbers
return "a" .. 1 .. 2.5, "10" < "9", "" < "a", ("abc"):len()
-- case: pack and unpack (6.4.2)
local p = string.pack("i4i8>i2", 1, -2, 258)
local a, b, c, nxt = string.unpack("i4i8>i2", p)
local s = string.pack("z s1 B", "hi", "there", 255)
local x, y, z = string.unpack("z s1 B", s)
return #p, a, b, c, nxt, x, y, z, string.packsize("i4 i8 d"), string.unpack("<d", string.pack("<d", 1.5))
-- case: utf8 library (6.5)
local s = "héllo €"
local cps = {}
for _, c in utf8.codes(s) do cps[#cps + 1] = c end
return utf8.len(s), utf8.char(72, 233, 8364), table.concat(cps, ","), utf8.codepoint(s, 2), utf8.offset(s, 3), utf8.len("\xff"), #utf8.charpattern
