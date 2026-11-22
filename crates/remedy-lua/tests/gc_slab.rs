//! SEC-M5: free slab slots are counted against the memory limit.
//!
//! A slab keeps every slot below its last object, and a freed slot stays resident. One
//! survivor at the end of each kind's slab kept every slab at its peak, outside the
//! account: filling to near the limit with each kind in turn, keeping only the last
//! object made, ended at 210 MiB resident under a 64 MiB limit with 0 MiB counted. Free
//! slots are counted now, from the sweep that frees them until they are reused or cut.

use super::common;
use remedy_lua::{Libs, Limits, Lua};

/// Fills to 7/8 of the limit with objects from `make`, keeps only the last one made, and
/// collects; for each kind in `KINDS` (by name).
fn fill_script(kinds: &[&str], limit_mib: usize) -> String {
    let mut s = format!(
        "local keep = {{}} local function fill(make) local t = {{}} local n = 0 \
         while collectgarbage('count') < {} do for j = 1, 1000 do n = n + 1 t[n] = make(n) end end \
         keep[#keep + 1] = t[n] t = nil collectgarbage() collectgarbage() return n end ",
        limit_mib * 1024 * 7 / 8
    );
    for k in kinds {
        let make = match *k {
            "tables" => "function() return {} end",
            "closures" => "function(i) return function() return i end end",
            "coroutines" => "function() return coroutine.create(print) end",
            "upvalues" => "function(i) local x = i return function() x = x + 1 return x end end",
            "strings" => "function(i) return tostring(i) .. ('a'):rep(40) end",
            _ => unreachable!(),
        };
        s += &format!("fill({make}) ");
    }
    s
}

const KINDS: [&str; 5] = ["tables", "closures", "coroutines", "upvalues", "strings"];
const LIMIT_MIB: usize = 32;

fn kib(line: &str, key: &str) -> usize {
    line.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)?.parse().ok()).unwrap_or(0)
}

/// Runs in a child process (alone, so its peak resident memory is the script's): prints
/// the peak resident growth, the bytes counted at the peak of the fills and at the end, and
/// the fuel used.
#[test]
fn child() {
    let Ok(spec) = std::env::var("REMEDY_SLAB_CHILD") else { return };
    let mut parts = spec.split(':');
    let kinds: Vec<&str> = parts.next().unwrap().split(',').collect();
    let fast = parts.next() == Some("fast");
    let seed: u64 = parts.next().unwrap().parse().unwrap();
    let status = || std::fs::read_to_string("/proc/self/status").unwrap();
    let mut lua = Lua::new(Limits::default().memory(LIMIT_MIB << 20).fuel(1 << 34).hash_seed(seed), Libs::ALL);
    lua.set_fast_loop(fast);
    let h0 = kib(&status(), "VmHWM:");
    lua.exec(&fill_script(&kinds, LIMIT_MIB), "=fill").unwrap();
    let end_rss = kib(&status(), "VmRSS:");
    let peak = kib(&status(), "VmHWM:");
    println!(
        "SLAB peak_kib={} end_rss_kib={} counted_kib={} fuel={}",
        peak.saturating_sub(h0),
        end_rss.saturating_sub(h0),
        lua.memory_used() >> 10,
        (1u64 << 34) - lua.fuel_left()
    );
}

/// (peak resident growth KiB, end resident growth KiB, counted at the end KiB, fuel).
fn run_child(kinds: &[&str], fast: bool, seed: u64) -> (usize, usize, usize, u64) {
    let spec = format!("{}:{}:{seed}", kinds.join(","), if fast { "fast" } else { "full" });
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "gc_slab::child", "--nocapture", "--test-threads=1"])
        .env("REMEDY_SLAB_CHILD", &spec)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{spec}: {text}{}", String::from_utf8_lossy(&out.stderr));
    let line = text.find("SLAB ").map(|at| &text[at..]).and_then(|t| t.lines().next()).unwrap_or_else(|| panic!("{spec}: {text}"));
    let field = |k: &str| -> u64 {
        line.split_whitespace().find_map(|w| w.strip_prefix(k)?.strip_prefix('=')?.parse().ok()).unwrap()
    };
    (field("peak_kib") as usize, field("end_rss_kib") as usize, field("counted_kib") as usize, field("fuel"))
}

/// The repro, per kind and all kinds in turn: the resident memory stays within 1.3x the
/// limit (it was 210 MiB under 64 for all five kinds), and the free slots each fill leaves
/// are counted (it was 0). Fuel is the same with the fast loop on and off and under
/// another hash seed, and so is the account: free slots are not a seed's or a path's.
#[test]
fn one_survivor_per_slab_keeps_resident_memory_counted() {
    let _g = common::memory_heavy();
    let limit_kib = LIMIT_MIB << 10;
    let mut cases: Vec<Vec<&str>> = KINDS.iter().map(|k| vec![*k]).collect();
    cases.push(KINDS.to_vec());
    for kinds in &cases {
        let (peak, end_rss, counted, fuel) = run_child(kinds, true, 7);
        let name = kinds.join(",");
        assert!(peak * 10 <= limit_kib * 13, "{name}: peak resident {peak} KiB past 1.3x the {limit_kib} KiB limit");
        assert!(counted <= limit_kib, "{name}: counted {counted} KiB past the limit");
        // Counted tracks resident: the slots the fill left free (all of each slab but its
        // last object) are counted. Tables, closures and coroutines leave their whole
        // record in the slot; a 45-byte string half of it (its bytes were freed).
        let share = if kinds.len() == 1 && kinds[0] == "strings" { 3 } else { 2 };
        assert!(
            counted * share >= limit_kib * 7 / 8,
            "{name}: {counted} KiB counted after the fill, {end_rss} KiB resident"
        );
        for (fast, seed) in [(false, 7), (true, 0x5eed_1234)] {
            let (_, _, c, f) = run_child(kinds, fast, seed);
            assert_eq!((c, f), (counted, fuel), "{name}: fast {fast} seed {seed:#x}: account or fuel differ");
        }
    }
}

/// The collector paces on live bytes: free slots change neither when cycles run nor the
/// fuel a program costs. The same churn after the same fill costs the same fuel whether
/// the fill's free slots stay (one survivor at the slab's end) or are cut (at its start).
#[test]
fn free_slots_do_not_change_the_pacing() {
    let churn = "local s = 0 for i = 1, 200000 do local t = {i, i + 1} s = s + #t end return s";
    let mut fuel = Vec::new();
    for keep in ["t[150000]", "t[1]"] {
        for fast in [true, false] {
            let mut lua = Lua::new(Limits::default().memory(32 << 20).fuel(1 << 34).hash_seed(3), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(
                &format!(
                    "local t = {{}} for n = 1, 150000 do t[n] = {{}} end keep = {keep} t = nil \
                     collectgarbage() collectgarbage()"
                ),
                "=fill",
            )
            .unwrap();
            let slack = lua.internal_sizes().into_iter().find(|(k, _)| *k == "slack_bytes").unwrap().1;
            if keep == "t[150000]" {
                assert!(slack > 150000 * 100, "fill left {slack} bytes of free slots");
            } else {
                assert!(slack < 4096, "slots cut: {slack} bytes free");
            }
            // Each host call starts with the whole budget: the churn's fuel is the budget
            // less what it left (not less what the fill left: that was the fill's and the
            // churn's difference, and wrapped in a dev build if the fill used more).
            lua.exec(churn, "=churn").unwrap();
            fuel.push(lua.limits().fuel - lua.fuel_left());
        }
    }
    assert!(fuel.iter().all(|&f| f == fuel[0]), "churn fuel {fuel:?}");
}
