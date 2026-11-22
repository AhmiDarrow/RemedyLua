//! Latency probe for the interner: the longest stretch between looks at the clock (the
//! host's deadline) while a script makes `N` strings that stay live, and while a host `load`
//! interns `K` string constants. `interngap [N] [K]` (defaults 2,000,000 and 600,000; a
//! `K` of 0 skips the load). `BODY` replaces the loop's body (`L[i] = 's' .. i`); the
//! table `L` has its `N` slots before the clock starts. Prints the page faults and the
//! interner's sizes after the script.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs, clippy::indexing_slicing, clippy::panic)]
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use remedy_lua::{Libs, Limits, Lua};

/// A clock that records the longest gap between two of its reads.
fn gap_clock() -> (remedy_lua::Clock, Arc<Mutex<(Instant, Duration, u64)>>) {
    let st = Arc::new(Mutex::new((Instant::now(), Duration::ZERO, 0u64)));
    let s = Arc::clone(&st);
    let t0 = Instant::now();
    let clock: remedy_lua::Clock = Arc::new(move || {
        let now = Instant::now();
        let mut g = s.lock().unwrap();
        g.1 = g.1.max(now - g.0);
        if std::env::var_os("GAPLOG").is_some() && now - g.0 > Duration::from_millis(1) {
            eprintln!("gap {:?} at read {} t {:?}", now - g.0, g.2, now - t0);
        }
        g.0 = now;
        g.2 += 1;
        now - t0
    });
    (clock, st)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(2_000_000);
    let k: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(600_000);
    let mut lua = Lua::new(Limits::default().memory(4 << 30).fuel(1 << 40).hash_seed(7), Libs::ALL);
    // The table grows first (its array part's growth is not the interner's).
    lua.exec(&format!("L = {{}} for i = 1, {n} do L[i] = true end"), "=setup").unwrap();
    let (clock, st) = gap_clock();
    lua.set_time_limit(Some((clock, Duration::from_secs(600))));
    let body = std::env::var("BODY").unwrap_or_else(|_| "L[i] = 's' .. i".into());
    let f = lua.load(&format!("local L = L for i = 1, {n} do {body} end return #L"), "=vm").unwrap();
    st.lock().unwrap().0 = Instant::now();
    let t = Instant::now();
    lua.call(f.value(), &[]).unwrap();
    let all = t.elapsed();
    let g = *st.lock().unwrap();
    println!("vm: {n} strings in {all:?}, longest between deadline reads {:?}", g.1);
    println!("reads {}, mean gap {:?}", g.2, all / g.2.max(1) as u32);
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    println!("minflt {}", stat.split_whitespace().nth(9).unwrap_or("?"));
    println!(
        "{:?}",
        lua.internal_sizes().into_iter().filter(|(k, _)| k.starts_with("intern")).collect::<Vec<_>>()
    );
    if k == 0 {
        return;
    }
    // A host load: the interrupt is looked at by the compile's steps; a thread notes how late
    // a load stops after it, at points along the load.
    let src = format!("return {{{}}}", (0..k).map(|i| format!("\"c{i}\",")).collect::<String>());
    let full = {
        let mut l = Lua::new(Limits::unmetered().memory(4 << 30), Libs::NONE);
        let t = Instant::now();
        l.load(&src, "=t").unwrap();
        t.elapsed()
    };
    let mut worst = Duration::ZERO;
    for p in 1..=12u32 {
        let at = full * p / 13;
        let mut l = Lua::new(Limits::unmetered().memory(4 << 30), Libs::NONE);
        let h = l.interrupt_handle();
        let th = std::thread::spawn(move || {
            std::thread::sleep(at);
            h.interrupt();
            Instant::now()
        });
        let r = l.load(&src, "=t").map(|_| ());
        let end = Instant::now();
        let fired = th.join().unwrap();
        if r.is_err() {
            worst = worst.max(end.saturating_duration_since(fired));
        }
    }
    println!("load: {k} constants in {full:?}, worst stop after the interrupt {worst:?}");
}
