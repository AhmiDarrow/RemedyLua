-- JSON-style encode/decode round trips: recursive encoder with sorted keys and escapes,
-- recursive-descent decoder using byte/find/sub/match.
local function encode_str(s)
  return '"' .. s:gsub('[%c"\\]', function(c)
    if c == '"' then return '\\"'
    elseif c == '\\' then return '\\\\'
    elseif c == '\n' then return '\\n'
    else return string.format('\\u%04x', c:byte()) end
  end) .. '"'
end

local function is_array(t)
  local n = #t
  if n == 0 then return next(t) == nil end
  local c = 0
  for _ in pairs(t) do c = c + 1 end
  return c == n
end

local encode
encode = function(v, out)
  local tv = type(v)
  if tv == "table" then
    if is_array(v) then
      out[#out + 1] = "["
      for i = 1, #v do
        if i > 1 then out[#out + 1] = "," end
        encode(v[i], out)
      end
      out[#out + 1] = "]"
    else
      local keys = {}
      for k in pairs(v) do keys[#keys + 1] = k end
      table.sort(keys)
      out[#out + 1] = "{"
      for i, k in ipairs(keys) do
        if i > 1 then out[#out + 1] = "," end
        out[#out + 1] = encode_str(k)
        out[#out + 1] = ":"
        encode(v[k], out)
      end
      out[#out + 1] = "}"
    end
  elseif tv == "string" then
    out[#out + 1] = encode_str(v)
  elseif tv == "number" then
    if math.type(v) == "integer" then out[#out + 1] = tostring(v) else out[#out + 1] = string.format("%.14g", v) end
  elseif tv == "boolean" then
    out[#out + 1] = tostring(v)
  else
    out[#out + 1] = "null"
  end
end

local function skip(s, i) return s:find("[^ \t\r\n]", i) or #s + 1 end
local escapes = { ['"'] = '"', ['\\'] = '\\', ['/'] = '/', b = '\b', f = '\f', n = '\n', r = '\r', t = '\t' }

local function decode_string(s, i)
  local parts, j = {}, i + 1
  while true do
    local k = s:find('["\\]', j)
    parts[#parts + 1] = s:sub(j, k - 1)
    if s:byte(k) == 34 then return table.concat(parts), k + 1 end
    local e = s:sub(k + 1, k + 1)
    if e == "u" then
      parts[#parts + 1] = utf8.char(tonumber(s:sub(k + 2, k + 5), 16))
      j = k + 6
    else
      parts[#parts + 1] = escapes[e]
      j = k + 2
    end
  end
end

local decode_value
decode_value = function(s, i)
  i = skip(s, i)
  local c = s:byte(i)
  if c == 123 then
    local obj = {}
    i = skip(s, i + 1)
    if s:byte(i) == 125 then return obj, i + 1 end
    while true do
      local k
      k, i = decode_string(s, skip(s, i))
      i = skip(s, i) + 1
      local v
      v, i = decode_value(s, i)
      obj[k] = v
      i = skip(s, i)
      local d = s:byte(i)
      i = i + 1
      if d == 125 then return obj, i end
    end
  elseif c == 91 then
    local arr = {}
    i = skip(s, i + 1)
    if s:byte(i) == 93 then return arr, i + 1 end
    while true do
      local v
      v, i = decode_value(s, i)
      arr[#arr + 1] = v
      i = skip(s, i)
      local d = s:byte(i)
      i = i + 1
      if d == 93 then return arr, i end
    end
  elseif c == 34 then
    return decode_string(s, i)
  elseif c == 116 then
    return true, i + 4
  elseif c == 102 then
    return false, i + 5
  elseif c == 110 then
    return nil, i + 4
  else
    local num = s:match("^-?%d+%.?%d*[eE]?[-+]?%d*", i)
    return tonumber(num), i + #num
  end
end

local data = {}
for i = 1, 300 do
  data[i] = {
    id = i, name = "user" .. i, email = "u" .. i .. "@example.com", score = i * 1.5, active = i % 2 == 0,
    tags = { "t" .. (i % 7), "t" .. (i % 11), "quote\"d" },
    pos = { x = i, y = -i, z = i / 4 }, note = "line1\nline2\ttab",
  }
end
local total = 0
local s
for r = 1, 15 do
  local out = {}
  encode(data, out)
  s = table.concat(out)
  local back = decode_value(s, 1)
  total = total + #s + #back + back[r].id
end
local out = {}
encode(decode_value(s, 1), out)
return total .. " " .. tostring(table.concat(out) == s)
