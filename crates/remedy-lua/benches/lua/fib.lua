-- Recursive Fibonacci: Lua-to-Lua call and return cost.
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
return tostring(fib(32))
