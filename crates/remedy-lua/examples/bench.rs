//! Micro-benchmarks against the reference C Lua (built from the vendored sources).
//! `cargo run --release --example bench -p remedy-lua`
#![allow(clippy::unwrap_used, missing_docs, clippy::print_stdout)]

use std::time::Instant;

use remedy_lua::{Libs, Limits, Lua};

const BENCHES: &[(&str, &str)] = &[
    ("fib(27)", "local function fib(n) if n < 2 then return n end return fib(n-1) + fib(n-2) end return fib(27)"),
    (
        "loop sum 10M",
        "local s = 0 for i = 1, 10000000 do s = s + i % 7 end return s",
    ),
    (
        "table churn",
        "local n = 0 for i = 1, 200000 do local t = {i, i + 1, x = i, y = 'a'} t[#t + 1] = t.x n = n + #t end return n",
    ),
    (
        "table array fill/read",
        "local t = {} for i = 1, 2000000 do t[i] = i end local s = 0 for i = 1, #t do s = s + t[i] end return s",
    ),
    (
        "string building",
        "local parts = {} for i = 1, 200000 do parts[#parts + 1] = tostring(i) .. ',' end local s = table.concat(parts) return #s",
    ),
    (
        "string concat ..",
        "local s = '' for i = 1, 20000 do s = s .. 'x' end return #s",
    ),
    (
        "method calls",
        "local P = {} P.__index = P function P.new(x) return setmetatable({x = x}, P) end function P:get() return self.x end function P:add(n) self.x = self.x + n end local p = P.new(0) for i = 1, 2000000 do p:add(1) end return p:get()",
    ),
    (
        "closures",
        "local function mk(i) return function(x) return x + i end end local s = 0 for i = 1, 500000 do s = s + mk(i)(1) end return s",
    ),
    (
        "string ops",
        "local n = 0 for i = 1, 100000 do local s = string.format('%d:%s', i, 'abc') n = n + #s:upper() + (s:find(':') or 0) end return n",
    ),
    (
        "sort 200k",
        "local t = {} local x = 1 for i = 1, 200000 do x = (x * 16807) % 2147483647 t[i] = x end table.sort(t) return t[1]",
    ),
];

fn main() {
    println!("{:<24} {:>12} {:>12} {:>8}", "benchmark", "RemedyLua", "C Lua", "ratio");
    for (name, src) in BENCHES {
        let mut lua = Lua::new(Limits::default().fuel(u64::MAX / 2).memory(1 << 30), Libs::ALL);
        let f = lua.load(*src, "=bench").unwrap();
        let t = Instant::now();
        let ours = lua.call(f.value(), &[]).unwrap();
        let ours_t = t.elapsed();
        let ours_v = match ours.first().copied().unwrap_or(remedy_lua::Value::Nil) {
            remedy_lua::Value::Int(i) => i.to_string(),
            v => lua.to_string_lossy(v).unwrap_or_else(|| format!("{v:?}")),
        };
        let (c_t, c_v) = {
            let mut st = remedy_lua_oracle::State::new();
            let t = Instant::now();
            let v = st.exec(src).unwrap();
            (Some(t.elapsed()), v)
        };
        let ratio = c_t.map(|c| ours_t.as_secs_f64() / c.as_secs_f64());
        println!(
            "{:<24} {:>10.1}ms {:>10}ms {:>8} {}",
            name,
            ours_t.as_secs_f64() * 1000.0,
            c_t.map_or("-".into(), |c| format!("{:.1}", c.as_secs_f64() * 1000.0)),
            ratio.map_or("-".into(), |r| format!("{r:.2}x")),
            if !c_v.contains(ours_v.trim_end_matches(".0")) { format!("(results differ: {ours_v} vs {c_v})") } else { String::new() }
        );
    }
}
