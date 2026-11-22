-- The line a return is numbered (tests/parity_lua54.rs, parity_15): where its values end.
local O1 = setmetatable({}, {__close = 42})
local r = {}
r[1] = select(2, pcall(function() local c <close> = O1 return
 "body" end))
r[2] = select(2, pcall(function() local c <close> = O1 return
 1,
 2 end))
r[3] = select(2, pcall(function() local c <close> = O1 return
 end))
r[4] = select(2, pcall(function() local c <close> = O1
 end))
r[5] = select(2, pcall(function() local c <close> = O1 return (
 tostring(1)) end))
r[6] = select(2, pcall(function() local c <close> = O1 return tostring(
 1) end))
r[7] = select(2, pcall(function() do local c <close> = O1
  end end))
r[8] = select(2, pcall(function(...) local c <close> = O1 return ...
 end))
r[9] = select(2, pcall(function() local c <close> = O1 return 1;
 end))
r[10] = select(2, pcall(function() local c <close> = O1 return "a" ..
 "b" end))
r[11] = select(2, pcall(function() local c <close> = O1 return {
 1} end))
r[12] = select(2, pcall(function() local c <close> = O1 return 1 +
 tonumber("2") end))
return table.concat(r, "\n")
