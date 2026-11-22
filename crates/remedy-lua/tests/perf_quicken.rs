//! `local function f` reads `f` as the running closure
//! (`GetSelf`, `bytecode::settle_upvals`). Each program runs against C Lua with the fast
//! loop on and off (same results, fuel and memory: `common::remedy`).

use crate::common::{oracle, remedy};

/// `local function f` reading `f` reads the running closure (`GetSelf`) only while nothing
/// can assign `f`: every way to assign it, against C Lua.
#[test]
fn self_reads_as_c() {
    for program in [
        "local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end\nreturn fib(20)",
        "local function f(n) if n == 0 then return f.x end return f(n - 1) end\nreturn pcall(f, 3)",
        "local function f() return f end\nlocal g = f\nf = 1\nreturn g()",
        "local function f() return f end\nlocal function set() f = 2 end\nlocal g = f\nset()\nreturn g()",
        "local function f() return f end\nlocal g = f\n;(function() (function() f = 3 end)() end)()\nreturn g()",
        "local function f(x) if x then f = x end return f end\nlocal g = f\nreturn g(5), g()",
        "local fs = {}\nfor i = 1, 3 do local function f() return f, i end fs[i] = f end\nreturn fs[1]() == fs[1], fs[3]() == fs[3], select(2, fs[2]())",
        "local co = coroutine.wrap(function() local function f(n) coroutine.yield(n) if n > 0 then return f(n - 1) end end f(2) end)\nreturn co(), co(), co()",
        "local function f() local function g() return f end return g() end\nreturn f() == f",
    ] {
        assert_eq!(remedy(program, 10_000_000), oracle(program, 10_000_000), "differs from C Lua:\n{program}");
    }
}
