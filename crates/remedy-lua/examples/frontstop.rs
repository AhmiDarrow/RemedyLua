//! Front-end interrupt latency: a host `load` of a few MB of hostile source, interrupted at
//! many points along it; how late each stops (the worst is what matters).
//! `cargo run --release --example frontstop -p remedy-lua [-- CASE [POINTS]]`
#![allow(clippy::unwrap_used, missing_docs, clippy::print_stdout)]

use std::time::{Duration, Instant};

use remedy_lua::{Libs, Limits, Lua};

fn source(case: &str) -> String {
    match case {
        "do" => format!("{}{}", "do ".repeat(850_000), "end ".repeat(850_000)),
        "dots" => format!("return a{}", ".b".repeat(3_000_000)),
        "flat" => format!("return {{{}}}", "1,".repeat(3_000_000)),
        "locals" => (0..2000).map(|j| format!("local {}\n", (0..200).map(|i| format!("v{}", j * 200 + i)).collect::<Vec<_>>().join(","))).collect(),
        "keys" => format!("return {{{}}}", (0..500_000).map(|i| format!("a{i}=1,")).collect::<String>()),
        "strs" => format!("return {{{}}}", (0..600_000).map(|i| format!("\"s{i}\",")).collect::<String>()),
        "bigstr" => format!("return \"{}\"", "a".repeat(16 << 20)),
        _ => panic!("unknown case {case}"),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let cases: Vec<String> = match args.next() {
        Some(c) if c != "all" => vec![c],
        _ => ["do", "dots", "flat", "locals", "keys", "strs"].iter().map(|s| (*s).to_owned()).collect(),
    };
    let points: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(24);
    // A third argument: interrupt only there (milliseconds into the load).
    let only: Option<u64> = args.next().and_then(|a| a.parse().ok());
    for case in cases {
        let src = source(&case);
        let full = {
            let mut lua = Lua::new(Limits::unmetered().memory(8 << 30), Libs::NONE);
            let t = Instant::now();
            let r = lua.load(&src, "=t").map(|_| ());
            let e = t.elapsed();
            assert!(r.is_ok(), "{case}: {r:?}");
            e
        };
        let mut worst = (Duration::ZERO, Duration::ZERO);
        for k in 1..=points {
            let at = only.map_or(full * k / (points + 1), Duration::from_millis);
            let mut lua = Lua::new(Limits::unmetered().memory(8 << 30), Libs::NONE);
            let h = lua.interrupt_handle();
            let t = std::thread::spawn(move || {
                std::thread::sleep(at);
                h.interrupt();
                Instant::now()
            });
            let started = Instant::now();
            let r = lua.load(&src, "=t").map(|_| ());
            let end = Instant::now();
            let fired = t.join().unwrap();
            if r.is_err() {
                let late = end.saturating_duration_since(fired);
                if std::env::var("FRONTSTOP_VERBOSE").is_ok() {
                    println!("  at {at:?}: {late:?} late, {:?} into the load", fired.saturating_duration_since(started));
                }
                if late > worst.0 {
                    worst = (late, at);
                }
            }
            lua.interrupt_handle().reset();
            let _ = lua.load("return 1", "=ok");
        }
        println!("{case:8} {:.1} MB, load {full:?}: worst {:?} late (interrupted at {:?})", src.len() as f64 / 1e6, worst.0, worst.1);
    }
}
