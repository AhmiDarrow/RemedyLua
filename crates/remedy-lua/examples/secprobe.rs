//! Security probe: CPU per unit of fuel of a script against plain code.
//! `cargo run --release -p remedy-lua --example secprobe -- SETUP BODY [fuel] [mem_mib]` (`@file` for
//! either reads it from a file)
//! SETUP runs unmetered-ish (1<<34 fuel); BODY is looped `while true do I = I + 1 BODY end`
//! under `fuel`, fast loop and full path. Prints ratio, fuel used, memory, RSS gain.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs, clippy::indexing_slicing, clippy::panic)]

use std::time::Instant;

use remedy_lua::{Libs, Limits, Lua};

fn rss() -> usize {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|x| x.parse::<usize>().ok()))
        .map_or(0, |p| p * 4096)
}

fn state(mem: usize, fast: bool) -> Lua {
    let libs = Libs::ALL.union(Libs::DEBUG_INFO).union(Libs::REQUIRE);
    let mut lua =
        Lua::new(Limits::default().memory(mem).string_len(32 << 20).fuel(1 << 34).hash_seed(7), libs);
    lua.add_module(
        "hostmod",
        "local M = {n = 1} for i = 1, 20 do M[i] = function() return i end end return M",
    );
    lua.set_fast_loop(fast);
    lua.set_print(|_| {});
    lua
}

fn run(setup: &str, body: &str, fuel: u64, mem: usize, fast: bool) -> (f64, u64, String, usize, usize) {
    let r0 = rss();
    let mut lua = state(mem, fast);
    lua.exec(format!("I = 0 {setup}"), "=setup").unwrap();
    lua.set_limits(lua.limits().fuel(fuel));
    let f = lua.load(format!("while true do I = I + 1 {body} end"), "=probe").unwrap();
    let t = Instant::now();
    let r = lua.call(f.value(), &[]);
    let el = t.elapsed().as_nanos() as f64;
    let used = fuel - lua.fuel_left();
    let mu = lua.memory_used();
    let g = rss().saturating_sub(r0);
    (el / used.max(1) as f64, used, format!("{:?}", r.err()), mu, g)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    // `@path` reads the text from a file.
    let text = |s: Option<&String>| match s {
        Some(s) if s.starts_with('@') => std::fs::read_to_string(&s[1..]).unwrap(),
        Some(s) => s.clone(),
        None => String::new(),
    };
    let setup = text(a.get(1));
    let body = text(a.get(2));
    let fuel: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(2_000_000);
    let mem: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(256) << 20;
    let mut base = f64::MAX;
    for _ in 0..3 {
        base = base.min(run("", "local x = I * 2 local y = {} ", fuel, 64 << 20, true).0);
    }
    for fast in [true, false] {
        let (c, used, r, mu, g) = run(&setup, &body, fuel, mem, fast);
        println!(
            "fast {fast}: {:.1}x plain ({c:.1} ns/unit vs {base:.1}), fuel {used}, mem_used {} KiB, rss gain {} KiB, {r}",
            c / base,
            mu >> 10,
            g >> 10
        );
    }
}
