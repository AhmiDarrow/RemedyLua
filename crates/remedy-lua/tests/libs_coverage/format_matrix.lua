-- string.format: every conversion with every flag set, width and precision C accepts or
-- refuses, over values of each kind. One line per call: the spec, the value, the result.
local out = {}
local function show(ok, v)
  if type(v) == "string" then v = string.format("%q", v) else v = tostring(v) end
  return (ok and "ok " or "err ") .. v
end
local convs = {"d", "i", "u", "c", "x", "X", "o", "e", "E", "f", "F", "g", "G", "a", "A", "s", "q", "p", "%", "b", "y", "ld", "hd", "z"}
local flags = {"", "-", "+", " ", "#", "0", "-0", "+ ", "#0", "-#", "+-", "00", "--", "- +#0", "-----"}
local widths = {"", "1", "5", "20", "99", "100", "0"}
local precs = {"", ".", ".0", ".3", ".20", ".99", ".100", ".-1"}
local ints = {0, -7, 255, math.maxinteger, math.mininteger, 3.0, 3.5, "12", "0x10", "x"}
local flts = {0.0, -0.0, 3.5, -2.5, 1e300, 1e-300, 1/0, -1/0, 0/0, 2^53, 7, "1.5"}
local strs = {"abc", "", "a\0b", "\n\r\"\\\200", 12, 1.5, {}, true}
local chars = {65, 0, 255, -1, 256, 3.0, 3.5}
local ptrs = {print, {}, "s", 1, nil, true}
local vals = {d = ints, i = ints, u = ints, x = ints, X = ints, o = ints, c = chars,
  e = flts, E = flts, f = flts, F = flts, g = flts, G = flts, a = flts, A = flts,
  s = strs, q = {"abc", "a\0b\n\200", 1, -0.0, 1/0, 0/0, math.mininteger, 2^63, 0.1, true, nil, {}}, p = ptrs}
for _, c in ipairs(convs) do
  local vs = vals[c] or {1}
  for _, fl in ipairs(flags) do
    for _, w in ipairs(widths) do
      for _, pr in ipairs(precs) do
        local spec = "%" .. fl .. w .. pr .. c
        for k = 1, #vs + 1 do
          local v = vs[k]
          if k <= #vs or c == "p" then
            local ok, r = pcall(string.format, spec, v)
                    -- `%p` of an object is an address: only its shape compares.
            -- (So does `tostring` of a table, under `%s`.)
            if ok and (type(v) == "table" or type(v) == "function" or (c == "p" and type(v) == "string")) then
              r = r:gsub("0x%x+", "ADDR")
            end
            out[#out + 1] = spec .. " " .. tostring(k) .. " " .. show(ok, r)
          end
        end
      end
    end
  end
end
-- Several conversions, missing and extra arguments, `%` at the end, a long spec.
for _, f in ipairs({"%d %s", "%5.2f|%-5d|%x", "%", "abc%", "%%%d", "%10.3s|", "%099d", "%0100d",
                    "%-99.99f", "%1$d", "%*d", "%.*f", "% 05.1f", "%+.3e", "%#.0f", "%#x", "%#o", "%#g", "%.0e"}) do
  local ok, r = pcall(string.format, f, 1, 2.5, 3)
  out[#out + 1] = f .. " multi " .. show(ok, r)
  ok, r = pcall(string.format, f)
  out[#out + 1] = f .. " none " .. show(ok, r)
end
return table.concat(out, "\n")
