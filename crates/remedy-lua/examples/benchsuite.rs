//! The broad benchmark suite: every `benches/lua/*.lua` run in RemedyLua and in the
//! reference C Lua 5.4 (the oracle crate), outputs compared, best-of-N wall times, ratios
//! and geometric means. Results go to a markdown file, one section per label (a rerun with
//! the same label replaces its section).
//!
//! `cargo run --release --example benchsuite -p remedy-lua`
//!
//! Env: `BENCH_RUNS` (best of N, default 3), `BENCH_ONLY` (comma-separated name filters),
//! `BENCH_LABEL` (section title, default `latest`), `BENCH_DIR` (scripts, default `benches/lua`), `BENCH_OUT` (results file, default
//! `target/remedylua-bench.md`), `BENCH_NO_WRITE=1`.
//!
//! Each run gets a fresh state (made outside the timing); the timing covers compiling and
//! running the chunk on both sides. "sandbox on" is fuel and memory accounting with real,
//! generous limits. "unbounded" is the cheapest configuration the public API offers; it
//! still counts fuel and memory, because there is no switch that turns accounting off. When
//! one exists, `unbounded_limits` is the one place to use it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    missing_docs,
    clippy::print_stdout,
    clippy::indexing_slicing
)]

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use remedy_lua::{Libs, Limits, Lua, Value};

fn sandbox_limits() -> Limits {
    // Real limits, set high enough for every benchmark: accounting runs and is checked.
    Limits::default().fuel(50_000_000_000).memory(2 << 30)
}

fn unbounded_limits() -> Limits {
    Limits::unmetered()
}

fn run_remedy(src: &str, limits: Limits) -> (Duration, Result<String, String>) {
    let mut lua = Lua::new(limits, Libs::ALL);
    let t = Instant::now();
    let r = lua.exec(src, "=bench");
    let el = t.elapsed();
    let out = match r {
        Ok(v) => {
            let first = v.first().copied().unwrap_or(Value::Nil);
            match lua.call_global("tostring", &[first]) {
                Ok(Some(s)) => Ok(s.first().and_then(|s| lua.to_string_lossy(*s)).unwrap_or_default()),
                Ok(None) => Err("no tostring".into()),
                Err(e) => Err(e.message()),
            }
        }
        Err(e) => Err(e.message()),
    };
    (el, out)
}

fn run_c(src: &str) -> (Duration, Result<String, String>) {
    let mut st = remedy_lua_oracle::State::new();
    let t = Instant::now();
    let r = st.exec(src);
    (t.elapsed(), r)
}

struct Row {
    name: String,
    c: Option<Duration>,
    on: Duration,
    off: Duration,
    status: String,
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn geomean(xs: &[f64]) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    (xs.iter().map(|x| x.ln()).sum::<f64>() / xs.len() as f64).exp()
}

fn short(s: &str) -> String {
    let s = s.replace('\n', "\\n").replace('|', "\\|");
    if s.chars().count() > 60 { format!("{}…", s.chars().take(60).collect::<String>()) } else { s }
}

fn main() {
    // `BENCH_DIR`: another directory of benchmarks (micro-benchmarks while profiling).
    let dir = std::env::var("BENCH_DIR")
        .map_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/lua"), PathBuf::from);
    let runs: usize = std::env::var("BENCH_RUNS").ok().and_then(|s| s.parse().ok()).unwrap_or(3).max(1);
    let only: Vec<String> = std::env::var("BENCH_ONLY")
        .map(|s| s.split(',').map(|x| x.trim().to_owned()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("benches/lua")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "lua"))
        .collect();
    files.sort();
    println!("best of {runs}; times in ms");
    println!(
        "{:<16} {:>10} {:>10} {:>7} {:>10} {:>7}  status",
        "benchmark", "C Lua", "sandbox", "ratio", "unbounded", "ratio"
    );
    let mut rows = Vec::new();
    for path in &files {
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        if !only.is_empty() && !only.iter().any(|o| name.contains(o.as_str())) {
            continue;
        }
        let src = std::fs::read_to_string(path).unwrap();
        let (mut best_c, mut best_on, mut best_off) = (None::<Duration>, Duration::MAX, Duration::MAX);
        let (mut c_out, mut on_out, mut off_out) = (None, None, None);
        // Interleaved, so drift in machine load hits every column alike.
        for _ in 0..runs {
            let (t, r) = run_c(&src);
            best_c = Some(best_c.map_or(t, |b| b.min(t)));
            c_out = Some(r);
            let (t, r) = run_remedy(&src, sandbox_limits());
            best_on = best_on.min(t);
            on_out = Some(r);
            let (t, r) = run_remedy(&src, unbounded_limits());
            best_off = best_off.min(t);
            off_out = Some(r);
        }
        let on_out = on_out.unwrap();
        let status = match (&c_out, &on_out, &off_out) {
            (_, Err(e), _) => format!("RemedyLua error: {}", short(e)),
            (_, _, Some(Err(e))) => format!("RemedyLua (unbounded) error: {}", short(e)),
            (Some(Err(e)), _, _) => format!("C Lua error: {}", short(e)),
            (Some(Ok(c)), Ok(o), _) if c != o => {
                format!("DIFFERENT: C `{}` vs ours `{}`", short(c), short(o))
            }
            (_, Ok(o), _) => format!("same: `{}`", short(o)),
        };
        let ratio = |t: Duration| {
            best_c.map_or("-".to_owned(), |c| format!("{:.2}", t.as_secs_f64() / c.as_secs_f64()))
        };
        println!(
            "{:<16} {:>10} {:>10.1} {:>7} {:>10.1} {:>7}  {}",
            name,
            best_c.map_or("-".into(), |c| format!("{:.1}", ms(c))),
            ms(best_on),
            ratio(best_on),
            ms(best_off),
            ratio(best_off),
            status
        );
        rows.push(Row { name, c: best_c, on: best_on, off: best_off, status });
    }

    let ok_rows: Vec<&Row> = rows.iter().filter(|r| r.c.is_some() && r.status.starts_with("same")).collect();
    let gm_on =
        geomean(&ok_rows.iter().map(|r| r.on.as_secs_f64() / r.c.unwrap().as_secs_f64()).collect::<Vec<_>>());
    let gm_off = geomean(
        &ok_rows.iter().map(|r| r.off.as_secs_f64() / r.c.unwrap().as_secs_f64()).collect::<Vec<_>>(),
    );
    println!(
        "geometric mean of ratios over {} matching benchmarks: sandbox {gm_on:.2}, unbounded {gm_off:.2}",
        ok_rows.len()
    );

    if std::env::var("BENCH_NO_WRITE").is_ok_and(|v| v == "1") {
        return;
    }
    let label = std::env::var("BENCH_LABEL").unwrap_or_else(|_| "latest".into());
    let out_path = std::env::var("BENCH_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/remedylua-bench.md"));
    let mut sec = String::new();
    let _ = writeln!(sec, "## {label}\n");
    let _ = writeln!(
        sec,
        "Best of {runs} runs, release build, ratio = RemedyLua time / C Lua time (lower is better; 1.00 is parity). \
         \"sandbox\" = fuel and memory accounting with real limits; \"unbounded\" = `Limits::unmetered()` (no budgets; \
         the interpreter still counts). Geometric means cover the benchmarks whose outputs match.\n"
    );
    let _ = writeln!(
        sec,
        "| benchmark | C Lua ms | RemedyLua ms (sandbox) | ratio | RemedyLua ms (unbounded) | ratio | output |"
    );
    let _ = writeln!(sec, "|---|---:|---:|---:|---:|---:|---|");
    for r in &rows {
        let c = r.c.map_or("-".into(), |c| format!("{:.1}", ms(c)));
        let ro = r.c.map_or("-".into(), |c| format!("{:.2}", r.on.as_secs_f64() / c.as_secs_f64()));
        let rf = r.c.map_or("-".into(), |c| format!("{:.2}", r.off.as_secs_f64() / c.as_secs_f64()));
        let _ = writeln!(
            sec,
            "| {} | {c} | {:.1} | {ro} | {:.1} | {rf} | {} |",
            r.name,
            ms(r.on),
            ms(r.off),
            r.status
        );
    }
    let _ = writeln!(
        sec,
        "| **geometric mean** | | | **{gm_on:.2}** | | **{gm_off:.2}** | {} of {} match |\n",
        ok_rows.len(),
        rows.len()
    );

    let old = std::fs::read_to_string(&out_path).unwrap_or_else(|_| {
        "# RemedyLua benchmark results\n\nOne section per measuring run (cycle). Produced by \
         `crates/remedy-lua/examples/benchsuite.rs` over `crates/remedy-lua/benches/lua/*.lua`.\n\n"
            .into()
    });
    let head = format!("## {label}\n");
    let new = match old.find(&head) {
        Some(start) => {
            let rest = &old[start + head.len()..];
            let end = rest.find("\n## ").map_or(old.len(), |e| start + head.len() + e + 1);
            format!("{}{}{}", &old[..start], sec, &old[end..])
        }
        None => format!("{old}{sec}"),
    };
    std::fs::write(&out_path, new).expect("write results");
    println!("wrote {}", out_path.display());
}
