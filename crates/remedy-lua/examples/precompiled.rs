//! Compile a prelude once, instantiate it in many fresh sandboxes: `cargo run --release
//! --example precompiled`. Prints the cold-start cost of `load` against `load_compiled`.
#![allow(clippy::print_stdout)]

use std::time::Instant;

use remedy_lua::{Error, Libs, Limits, Lua, Value};

/// A ~2,000-line prelude: many small functions, as a host's helper library would be.
fn prelude() -> String {
    let mut s = String::from("local M = {}\n");
    for i in 0..400 {
        s.push_str(&format!(
            "function M.f{i}(x, y)\n  local t = {{ name = \"helper {i}\", x = x, y = y or {i} }}\n  if t.x > t.y then return t.x - t.y end\n  return t.name .. \":\" .. tostring(t.x + t.y)\nend\n"
        ));
    }
    s.push_str("return M\n");
    s
}

fn main() -> Result<(), Error> {
    let src = prelude();
    let runs = 200u32;

    let t = Instant::now();
    for _ in 0..runs {
        let mut lua = Lua::new(Limits::default(), Libs::SAFE);
        let f = lua.load(&src, "=prelude")?;
        lua.call(f.value(), &[])?;
    }
    let loaded = t.elapsed() / runs;

    let chunk = Lua::new(Limits::default(), Libs::SAFE).compile(&src, "=prelude")?;
    let t = Instant::now();
    let mut last = Vec::new();
    for _ in 0..runs {
        let mut lua = Lua::new(Limits::default(), Libs::SAFE);
        let f = lua.load_compiled(&chunk)?;
        let m = lua.call(f.value(), &[])?;
        let probe = lua.load("local M = ...; return M.f7(3)", "=probe")?;
        last = lua.call(probe.value(), &m)?;
    }
    let compiled = t.elapsed() / runs;

    assert_eq!(last.len(), 1);
    assert!(matches!(last.first(), Some(Value::Str(_))));
    println!("prelude: {} lines, {} bytes per instance", src.lines().count(), chunk.instance_bytes());
    println!("new state + load + run:          {loaded:?}");
    println!("new state + load_compiled + run: {compiled:?}");
    println!("speed-up: {:.1}x", loaded.as_secs_f64() / compiled.as_secs_f64());
    Ok(())
}
