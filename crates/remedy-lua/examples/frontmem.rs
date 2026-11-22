//! Front-end memory: peak resident memory (Linux `VmHWM`) of one load of a file against
//! the memory limit it ran under. Each run is its own process, so the peak is the load's.
//! `cargo run --release --example frontmem -p remedy-lua -- FILE LIMIT_BYTES [host|call|check]`
#![allow(clippy::unwrap_used, missing_docs, clippy::print_stdout)]

use remedy_lua::{Libs, Limits, Lua};

fn hwm() -> usize {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with("VmHWM"))
        .and_then(|l| l.split_whitespace().nth(1)?.parse::<usize>().ok())
        .map_or(0, |kb| kb << 10)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().unwrap();
    let limit: usize = args.next().and_then(|a| a.parse().ok()).unwrap_or(64 << 20);
    let mode = args.next().unwrap_or_else(|| "host".into());
    let src = std::fs::read(&file).unwrap();
    let name = if file.ends_with(".rlua") { "=t.rlua" } else { "=t" };
    let before = hwm();
    let t = std::time::Instant::now();
    let outcome = match mode.as_str() {
        "check" => format!("{:?}", remedy_lua::check(&src, name.trim_start_matches('=')).first().map(|d| d.message.clone())),
        "call" => {
            let mut lua = Lua::new(Limits::default().memory(limit).fuel(1 << 40), Libs::ALL);
            let s = lua.create_string(&src).unwrap();
            lua.set_global("src", s).unwrap();
            let r = lua.exec(format!("local f, e = load(src, '{name}') return f and 'ok' or e"), "=c");
            format!("{:?}", r.map(|v| lua.to_string_lossy(v[0])))
        }
        _ => {
            let mut lua = Lua::new(Limits::unmetered().memory(limit), Libs::NONE);
            format!("{:?}", lua.load(&src, name).map(|_| ()))
        }
    };
    let peak = hwm();
    println!(
        "{mode}: {outcome} in {:?}; peak {:.1} MB (before {:.1} MB) under {:.1} MiB: {:.2}x",
        t.elapsed(),
        peak as f64 / 1e6,
        before as f64 / 1e6,
        limit as f64 / 1048576.0,
        peak as f64 / limit as f64
    );
}
