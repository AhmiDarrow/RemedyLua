//! Front-end speed: parse, and parse + compile, of a few MB of generated Lua.
//! `cargo run --release --example frontbench -p remedy-lua [-- MB]`
#![allow(clippy::unwrap_used, missing_docs, clippy::print_stdout)]

use std::fmt::Write;
use std::time::Instant;

use remedy_lua::{Libs, Limits, Lua};

/// Mixed realistic code: functions, tables, loops, conditions, calls, strings.
fn source(bytes: usize) -> String {
    let mut s = String::new();
    let mut seed = 0x2545_f491_u64;
    let mut rnd = |n: u64| {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) % n
    };
    let mut i = 0;
    while s.len() < bytes {
        i += 1;
        // Many small functions per chunk-level block, as real code has.
        let _ = write!(
            s,
            "local function f{i}(a, b, c)\n  local t = {{x = a, y = b * {k}, \"s{i}\", [a] = c, {{1, 2, 3}}, n = {{}}}}\n  for j = 1, #t do\n    if t[j] and j % 2 == 0 then\n      t.x = t.x + j * (a - b) / {k2} .. \"\"\n    elseif j > {k} then\n      c = c or {{}}\n    else\n      t[j] = nil\n    end\n  end\n  while a < b do a = a + 1; b = b - (c and 1 or 2) end\n  local s = string.format(\"%d:%s\", a, tostring(b)) .. 'tail' .. a\n  return function(x) return x + a * b - (t.y or 0) ^ 2, s end\nend\n",
            k = rnd(1000),
            k2 = rnd(97) + 1,
        );
        let _ = write!(
            s,
            "local obj{i} = {{name = \"o{i}\", items = {{}}, count = 0}}\nfunction obj{i}:add(v) self.count = self.count + 1; self.items[self.count] = v; return self end\nfor k, v in pairs(obj{i}) do if type(v) == \"table\" and not v.done then v.done = k ~= nil end end\nrepeat local q = f{i}(1, 2, nil) until q ~= nil or {k} >= 0\n",
            k = rnd(50),
        );
    }
    s
}

fn main() {
    let mb: usize = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(4);
    let src = match std::env::var("FRONTBENCH_FILE") {
        Ok(f) => std::fs::read_to_string(f).unwrap(),
        Err(_) => source(mb << 20),
    };
    // `FRONTBENCH_DUMP=path`: writes the source there (for `examples/frontmem.rs`) and stops.
    if let Ok(path) = std::env::var("FRONTBENCH_DUMP") {
        std::fs::write(path, &src).unwrap();
        return;
    }
    let runs = 5;
    let mut parse = f64::MAX;
    let mut load = f64::MAX;
    for _ in 0..runs {
        let t = Instant::now();
        let d = remedy_lua::check(src.as_bytes(), "bench.lua");
        assert!(d.is_empty(), "{d:?}");
        parse = parse.min(t.elapsed().as_secs_f64());
        let mut lua = Lua::new(Limits::unmetered(), Libs::STANDARD);
        let t = Instant::now();
        lua.load(&src, "=bench").unwrap();
        load = load.min(t.elapsed().as_secs_f64());
    }
    let mbs = src.len() as f64 / (1 << 20) as f64;
    // With the oracle built in, the recursive front end side by side, alternating run by
    // run so both see the same machine load.
    #[cfg(feature = "front-oracle")]
    {
        let mut best = [[f64::MAX; 3]; 2];
        for _ in 0..runs * 3 {
            for (side, old) in best.iter_mut().zip([true, false]) {
                let t = remedy_lua::__front::time(src.as_bytes(), "bench.lua", old).unwrap();
                for (b, t) in side.iter_mut().zip(t) {
                    *b = b.min(t.as_secs_f64() * 1e3);
                }
            }
        }
        for (label, b) in ["old (recursive)", "new (iterative)"].iter().zip(best) {
            println!("{label}: parse {:6.1} ms  compile {:6.1} ms  drop {:5.1} ms", b[0], b[1], b[2]);
        }
    }
    println!("source: {:.2} MB, {} lines (best of {runs})", mbs, src.lines().count());
    println!("parse:           {:8.1} ms  {:6.1} MB/s", parse * 1e3, mbs / parse);
    println!("parse + compile: {:8.1} ms  {:6.1} MB/s", load * 1e3, mbs / load);
}
