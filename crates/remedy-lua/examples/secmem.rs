//! Security probe: peak RSS of a script against its memory limit.
//! `secmem SCRIPT [mem_mib] [fuel]`
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs, clippy::indexing_slicing, clippy::panic)]
use remedy_lua::{Libs, Limits, Lua};
fn hwm() -> usize {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with("VmHWM")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        })
        .unwrap_or(0)
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let src = a.get(1).cloned().unwrap_or_default();
    let mem: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(64) << 20;
    let fuel: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1 << 32);
    let libs = Libs::ALL.union(Libs::DEBUG_INFO).union(Libs::REQUIRE);
    let mut lua = Lua::new(Limits::default().memory(mem).fuel(fuel).hash_seed(7), libs);
    lua.set_print(|_| {});
    let h0 = hwm();
    let t = std::time::Instant::now();
    let r = lua.exec(src, "=s");
    let r = match r {
        Ok(v) => format!("ok {:?}", v.first().and_then(|x| lua.to_string_lossy(*x))),
        Err(e) => format!("err {e}"),
    };
    let now = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<usize>().ok())
        .unwrap_or(0)
        * 4096;
    println!(
        "peak rss {} MiB (from {} MiB) under a {} MiB limit, rss now {} MiB with {} MiB counted, {:?}, fuel {}, {}",
        hwm() >> 10,
        h0 >> 10,
        mem >> 20,
        now >> 20,
        lua.memory_used() >> 20,
        t.elapsed(),
        fuel - lua.fuel_left(),
        &r[..r.len().min(200)]
    );
}
