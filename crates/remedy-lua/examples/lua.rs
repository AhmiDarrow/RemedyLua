//! Direct check: runs a file or `-e` code in RemedyLua and in the C Lua oracle side by
//! side and prints both outcomes (the first result as `tostring` shows it, or the error).
//!
//! `cargo run -p remedy-lua --example lua -- -e "return 1 // 0"`
//! `cargo run -p remedy-lua --example lua -- path/to/file.lua`
//! Options: `--fuel N`, `--memory BYTES`, `--rlua` (RemedyLua dialect), `--ours` (skip C),
//! `--no-fast` (the full path only, as `REMEDYLUA_TEST_NO_FAST_LOOP=1` does),
//! `--all-libs` (every `Libs` flag, the opt-in capabilities included).
#![allow(clippy::unwrap_used, missing_docs, clippy::print_stdout)]

use remedy_lua::{Libs, Limits, Lua, Value};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut src = None;
    let mut limits = Limits::default();
    let mut name = "=t".to_owned();
    let mut only_ours = false;
    let mut stress = false;
    let mut no_fast = false;
    let mut libs = Libs::ALL;
    while let Some(a) = args.next() {
        match a.as_str() {
            "-e" => src = args.next(),
            "--fuel" => limits = limits.fuel(args.next().and_then(|s| s.parse().ok()).unwrap_or(10_000_000)),
            "--memory" => {
                limits = limits.memory(args.next().and_then(|s| s.parse().ok()).unwrap_or(64 << 20))
            }
            "--rlua" => name = "=t.rlua".into(),
            "--ours" => only_ours = true,
            "--stress" => stress = true,
            "--no-fast" => no_fast = true,
            "--all-libs" => libs = Libs::ALL | Libs::DEBUG_INFO | Libs::REQUIRE | Libs::ENV | Libs::IO,
            path => {
                src = Some(std::fs::read_to_string(path).unwrap_or_else(|e| format!("error('{path}: {e}')")))
            }
        }
    }
    let Some(src) = src else {
        println!(
            "usage: lua [-e code | file] [--fuel N] [--memory BYTES] [--rlua] [--ours] [--stress] [--no-fast] [--all-libs]"
        );
        return;
    };
    let start = std::time::Instant::now();
    let limits_fuel = limits.fuel;
    let mut lua = Lua::new(limits, libs);
    lua.set_gc_stress(stress);
    lua.set_fast_loop(!(no_fast || std::env::var_os("REMEDYLUA_TEST_NO_FAST_LOOP").is_some()));
    let ours = match lua.exec(&src, &name) {
        Ok(vals) => {
            let shown: Vec<String> = vals
                .iter()
                .map(|v| match *v {
                    Value::Nil | Value::Bool(_) | Value::Int(_) | Value::Num(_) | Value::Str(_) => lua
                        .call_global("tostring", &[*v])
                        .ok()
                        .flatten()
                        .and_then(|r| r.first().and_then(|s| lua.to_string_lossy(*s)))
                        .unwrap_or_default(),
                    other => other.type_name().to_owned(),
                })
                .collect();
            format!("ok: {}", shown.join("\t"))
        }
        Err(e) => format!("error: {e:?}"),
    };
    let out = lua.take_output();
    if !out.is_empty() {
        print!("{out}");
    }
    let fuel = limits_fuel.saturating_sub(lua.fuel_left());
    println!("RemedyLua ({:.1} ms, {fuel} fuel) {ours}", start.elapsed().as_secs_f64() * 1000.0);
    if !only_ours {
        let start = std::time::Instant::now();
        let theirs = match remedy_lua_oracle::run(&src, 1_000_000_000) {
            Ok(s) => format!("ok: {s}"),
            Err(e) => format!("error: {e}"),
        };
        println!("C Lua     ({:.1} ms) {theirs}", start.elapsed().as_secs_f64() * 1000.0);
    }
}
