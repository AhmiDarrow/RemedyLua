//! Immediate and constant operands: `x + 1`, `x - 1`, `x & 0xff`, `x < 10`,
//! `0 < x`, `x == "a"` select `AddI`, the K forms, `EqI`/`EqK` and `LtI`..`GeI`. Every
//! operator meets every kind of value and constant on both sides, in value and condition
//! form, against C Lua: results, error messages and the operand order metamethods see.
//! `common::remedy` also checks the fast loop and the full path agree on fuel and memory.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

const VALUES: &[&str] = &[
    "5", "-3", "0", "2.5", "-0.0", "1/0", "0/0", "math.maxinteger", "math.mininteger", "2^63", "'10'", "'0x10'",
    "'abc'", "nil", "true", "M",
];
const CONSTS: &[&str] = &[
    "1", "0", "-1", "7", "32767", "-32767", "32768", "-32768", "2.5", "-0.0", "'3'", "'a'", "1e308",
    "9007199254740993", "0x7fffffffffffffff",
];
const OPS: &[&str] = &["+", "-", "*", "/", "%", "//", "^", "&", "|", "~", "<<", ">>", "==", "~=", "<", "<=", ">", ">="];

/// A value whose every metamethod reports the event and the operand order it saw.
const META: &str = r#"
local M = setmetatable({}, (function()
  local mt = {}
  local function name(v) if rawequal(v, M) then return "M" end return tostring(v) end
  for _, e in ipairs({"add","sub","mul","div","mod","pow","idiv","band","bor","bxor","shl","shr","lt","le","eq"}) do
    mt["__" .. e] = function(a, b) return e .. "(" .. name(a) .. "," .. name(b) .. ")" end
  end
  return mt
end)())
"#;

fn program(v: &str) -> String {
    let mut p = format!("{META}local x = {v}\nlocal r = {{}}\nlocal function s(ok, e) r[#r + 1] = tostring(ok) .. ':' .. tostring(e) end\n");
    for op in OPS {
        for k in CONSTS {
            p += &format!("s(pcall(function() return x {op} {k} end))\n");
            p += &format!("s(pcall(function() return {k} {op} x end))\n");
            if matches!(*op, "==" | "~=" | "<" | "<=" | ">" | ">=") {
                p += &format!("s(pcall(function() if x {op} {k} then return 1 end return 0 end))\n");
                p += &format!("s(pcall(function() if {k} {op} x then return 1 end return 0 end))\n");
            }
        }
    }
    p += "return table.concat(r, ' ')";
    p.replace("local M = ", "local M; M = ")
}

#[test]
fn every_operator_with_every_constant_matches_c() {
    for v in VALUES {
        let p = program(v);
        let ours = common::remedy(&p, 50_000_000);
        let theirs = common::oracle(&p, 50_000_000);
        if ours != theirs {
            let (o, t) = (ours.clone().unwrap_or_else(|e| e), theirs.clone().unwrap_or_else(|e| e));
            let diff: Vec<_> = o.split(' ').zip(t.split(' ')).filter(|(a, b)| a != b).take(5).collect();
            panic!("x = {v}: C Lua differs, first differences (ours, C): {diff:?}");
        }
    }
}

/// Counting loops on immediates in the fast loop: `i < n` style exits, `x - 1` recursion.
#[test]
fn loops_on_immediates() {
    let p = r#"
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
local n, s, i = 0, 0, 0
while i < 1000 do i = i + 1; if i % 3 == 0 then s = s + i elseif 10 >= i then n = n - 2 end end
local f = 0.5
while f <= 100 do f = f * 2 + 1 end
return fib(15), s, n, i, f, 7 & i, i >> 2, i ~ 5
"#;
    let ours = common::remedy(p, 50_000_000);
    assert_eq!(ours.as_deref(), Ok("610 166833 -14 1000 191f 0 250 1005"));
    assert_eq!(ours, common::oracle(p, 50_000_000));
}
