//! Times a script and reports the fuel it used (instructions plus library work), so
//! time per step can be compared across changes: `prof file.lua [runs]`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout, missing_docs)]

use remedy_lua::{Libs, Limits, Lua};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: prof file.lua [runs]");
    let runs: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(3);
    let src = std::fs::read_to_string(&path).unwrap();
    let mut best = f64::MAX;
    let mut steps = 0;
    for _ in 0..runs {
        // A fixed hash seed: the steps reported are the same on every run.
        let mut lua = Lua::new(Limits::unmetered().hash_seed(0x5eed), Libs::ALL);
        let start = lua.fuel_left();
        let t = std::time::Instant::now();
        lua.exec(&src, "=t").unwrap();
        best = best.min(t.elapsed().as_secs_f64());
        steps = start - lua.fuel_left();
    }
    // C Lua (no hook) on the same file, for the ratio (`PROF_NO_C`: skipped, when profiling).
    let mut c_best = f64::MAX;
    let c_runs = if std::env::var_os("PROF_NO_C").is_some() { 0 } else { runs };
    for _ in 0..c_runs {
        let mut st = remedy_lua_oracle::State::new();
        let t = std::time::Instant::now();
        let _ = st.exec(&src);
        c_best = c_best.min(t.elapsed().as_secs_f64());
    }
    println!(
        "{path}: {:.1} ms, {steps} steps, {:.2} ns/step; C Lua {:.1} ms, ratio {:.2}",
        best * 1e3,
        best * 1e9 / steps as f64,
        c_best * 1e3,
        best / c_best
    );
}
