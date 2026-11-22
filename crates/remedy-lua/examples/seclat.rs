//! Security probe: how far past a time limit a script runs.
//! `seclat SETUP BODY [limit_ms] [mem_mib]`: SETUP unlimited, then BODY under the limit
//! (repeated 5 times; the worst overrun is printed).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs, clippy::indexing_slicing, clippy::panic)]
use std::sync::Arc;
use std::time::{Duration, Instant};

use remedy_lua::{Libs, Limits, Lua};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let setup = a.get(1).cloned().unwrap_or_default();
    let body = a.get(2).cloned().unwrap_or_default();
    let ms: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);
    let mem: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(512) << 20;
    let libs = Libs::ALL.union(Libs::DEBUG_INFO).union(Libs::REQUIRE);
    let mut worst = Duration::ZERO;
    let mut last = String::new();
    for _ in 0..5 {
        let mut lua = Lua::new(Limits::default().memory(mem).string_len(64 << 20).fuel(1 << 40).hash_seed(7), libs);
        lua.set_print(|_| {});
        lua.exec(&setup, "=setup").unwrap();
        let t0 = Instant::now();
        let clock: remedy_lua::Clock = Arc::new(move || t0.elapsed());
        lua.set_time_limit(Some((clock, Duration::from_millis(ms))));
        let f = lua.load(&body, "=body").unwrap();
        let start = Instant::now();
        let r = lua.call(f.value(), &[]);
        let el = start.elapsed();
        worst = worst.max(el.saturating_sub(Duration::from_millis(ms)));
        last = format!("{:?}", r.err());
    }
    println!("worst overrun {:?} past {ms} ms: {}", worst, &last[..last.len().min(120)]);
}
