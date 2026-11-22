-- string.pack / unpack / packsize: every option, with sizes, endianness and alignment
-- prefixes, over values in and out of range; then unpack of what was packed, at offsets,
-- and of truncated data.
local out = {}
local function hex(s) return (s:gsub(".", function(c) return string.format("%02x", c:byte()) end)) end
local function show(ok, ...)
  local t = {ok and "ok" or "err"}
  for i = 1, select("#", ...) do
    local v = select(i, ...)
    if type(v) == "string" then v = ok and hex(v) or v
    elseif math.type(v) == "float" then v = string.format("%.17g", v) .. "f" end
    t[#t + 1] = tostring(v)
  end
  return table.concat(t, " ")
end
local prefixes = {"", "<", ">", "=", "!", "!1", "!2", "!4", "!8", "!16", "!3", "!17", "!0", "<!4", ">!8", " "}
local opts = {"b", "B", "h", "H", "i", "I", "l", "L", "j", "J", "T", "f", "d", "n",
  "i1", "i2", "i3", "i4", "i7", "i8", "i9", "i16", "i0", "i17", "I1", "I3", "I8", "I9", "I16",
  "s", "s1", "s2", "s4", "s8", "s9", "s0", "z", "x", "Xi4", "Xh", "X", "Xz", "Xb", "c", "c0", "c3", "c10",
  "i4i8", "bi8", "!4 b i8", "<i2 >i2", "hxi4", "Xi8 b", "Hd", "bXd", "i", "w", "%", "i99999999999999999999"}
local ints = {0, 1, -1, 127, 128, 255, 256, -128, -129, 32767, -32769, 2^31, -2^31 - 1, math.maxinteger, math.mininteger, 1.0, 1.5, "3", "x", 1e100}
local strs = {"", "ab", "a\0b", string.rep("x", 300), 12}
local function args(o)
  local last = o:match("[%a]%d*$") or o
  local k = last:sub(1, 1)
  if k == "s" or k == "z" or k == "c" then return strs end
  if k == "f" or k == "d" or k == "n" then return {0, -0.0, 1.5, 1e300, 1/0, 0/0, 3, "2.5"} end
  if k == "x" or k == "X" then return {false} end
  return ints
end
for _, pre in ipairs(prefixes) do
  for _, o in ipairs(opts) do
    local fmt = pre .. o
    out[#out + 1] = fmt .. " size " .. show(pcall(string.packsize, fmt))
    local nvals = select(2, o:gsub("[bBhHiIlLjJTfdnszc]", ""))
    for i, v in ipairs(args(o)) do
      local vs = {}
      for k = 1, nvals do vs[k] = v end
      local ok, p = pcall(string.pack, fmt, table.unpack(vs, 1, nvals))
      out[#out + 1] = fmt .. " pack " .. i .. " " .. show(ok, p)
      if ok then
        out[#out + 1] = fmt .. " unpack " .. i .. " " .. show(pcall(string.unpack, fmt, p))
        out[#out + 1] = fmt .. " short " .. i .. " " .. show(pcall(string.unpack, fmt, p:sub(1, -2)))
        out[#out + 1] = fmt .. " at " .. i .. " " .. show(pcall(string.unpack, fmt, "\0" .. p, 2))
      end
    end
  end
end
-- Offsets and data unpack checks: positions in and out of range, negative.
for _, pos in ipairs({1, 0, -1, -8, 8, 9, 10, math.mininteger, 1.5}) do
  out[#out + 1] = "pos " .. tostring(pos) .. " " .. show(pcall(string.unpack, "i4", "12345678", pos))
end
for _, d in ipairs({"\255\255\255\255\255\255\255\255\1", "\0\0\0\0\0\0\0\128\255", "\0\0\0\0\0\0\0\0\1"}) do
  out[#out + 1] = "i9 " .. hex(d) .. " " .. show(pcall(string.unpack, "<i9", d))
  out[#out + 1] = "I9 " .. hex(d) .. " " .. show(pcall(string.unpack, "<I9", d))
end
out[#out + 1] = "z unterminated " .. show(pcall(string.unpack, "z", "abc"))
out[#out + 1] = "s1 long " .. show(pcall(string.unpack, "s1", "\5ab"))
out[#out + 1] = "s too big " .. show(pcall(string.pack, "s1", string.rep("x", 256)))
out[#out + 1] = "packsize var " .. show(pcall(string.packsize, "s")) .. " " .. show(pcall(string.packsize, "z"))
out[#out + 1] = "packsize huge " .. show(pcall(string.packsize, string.rep("c2147483647", 3)))
return table.concat(out, "\n")
