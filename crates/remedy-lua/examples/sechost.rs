//! Security probe: resident memory against counted memory for objects the host makes.
//! `sechost KIND [n]`, KIND one of `ud`, `ud_root`, `root`, `fn`, `fn_root`, `register`,
//! `ud_tracked`. Each object is kept alive in a global table (and, for `*_root`, a root).
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs, clippy::indexing_slicing, clippy::panic)]
use remedy_lua::{Libs, Limits, Lua, Value};

fn rss() -> usize {
    std::fs::read_to_string("/proc/self/statm").ok().and_then(|s| s.split_whitespace().nth(1)?.parse::<usize>().ok()).unwrap_or(0) * 4096
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let kind = a.get(1).cloned().unwrap_or_else(|| "ud".into());
    let n: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40), Libs::ALL);
    lua.exec("KEEP = {}", "=s").unwrap();
    let keep = match lua.exec("return KEEP", "=s").unwrap()[0] { Value::Table(t) => t, _ => panic!() };
    let mut roots = Vec::new();
    lua.collect_garbage().unwrap();
    let (r0, m0) = (rss(), lua.memory_used());
    for i in 0..n {
        let v = match kind.as_str() {
            "ud" | "ud_root" => lua.create_userdata(i as u64, None).unwrap(),
            "ud_tracked" => lua.create_userdata_tracked(i as u64, None, i as u64).unwrap(),
            "fn" | "fn_root" => lua.create_function("f", move |_, _| Ok(vec![Value::Int(i as i64)])).unwrap(),
            "root" => Value::Int(i as i64),
            "register" => {
                lua.register(&format!("g{i}"), move |_, _| Ok(vec![Value::Int(i as i64)])).unwrap();
                Value::Nil
            }
            _ => panic!("kind"),
        };
        if kind.ends_with("root") {
            let t = if kind == "root" { lua.create_table().map(Value::Table).unwrap() } else { v };
            roots.push(lua.root(t));
            if kind == "root" {
                continue;
            }
        }
        if !v.is_nil() {
            lua.table_set(keep, Value::Int(i as i64 + 1), v).unwrap();
        }
        if i % 4096 == 0 {
            lua.exec("", "=s").unwrap();
        }
    }
    lua.exec("", "=s").unwrap();
    lua.collect_garbage().unwrap();
    let (r1, m1) = (rss(), lua.memory_used());
    let (dr, dm) = (r1.saturating_sub(r0) as f64 / n as f64, m1.saturating_sub(m0) as f64 / n as f64);
    println!("{kind}: {dr:.0} B resident, {dm:.0} B counted per object ({:.2}x)", dr / dm.max(1.0));
    drop(roots);
}
