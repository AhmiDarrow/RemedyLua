//! Probe: `sweep1_soak::host_cycles`' loop, printing resident memory, `memory_used` and the
//! state's internal sizes (and slab capacities) every `step` iterations.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs, clippy::indexing_slicing, clippy::panic)]
use std::sync::Arc;

use remedy_lua::{Data, Libs, Limits, Lua, Value};

struct Payload(#[allow(dead_code)] Arc<()>);

fn rss() -> usize {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<usize>().ok())
        .unwrap_or(0)
        * 4096
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(200_000);
    let step: u64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    // `nocollect`: only the incremental collector runs (garbage builds up to the limit).
    let collect = a.get(3).is_none_or(|s| s != "nocollect");
    let token = Arc::new(());
    let mut lua = Lua::new(Limits::default().memory(8 << 20), Libs::STANDARD);
    let meta = lua.create_table().unwrap();
    let meta = lua.root(Value::Table(meta));
    let Value::Table(mt) = meta.value() else { unreachable!() };
    lua.exec("function check(f, u) return f(u) == u end", "=setup").unwrap();
    let mut prev: Vec<(&'static str, usize)> = Vec::new();
    for i in 0..n {
        let t = Arc::clone(&token);
        let f = lua
            .create_function("echo", move |_, a| {
                let _held = &t;
                Ok(a.to_vec())
            })
            .unwrap();
        let rf = lua.root(f);
        let ud = lua.create_userdata(Payload(Arc::clone(&token)), Some(mt)).unwrap();
        let ru = lua.root(ud);
        let r = lua.call_global("check", &[rf.value(), ru.value()]).unwrap().unwrap();
        assert_eq!(r, vec![Value::Bool(true)]);
        let t2 = Arc::clone(&token);
        lua.register(&format!("lib.f{}", i % 8), move |_, _| {
            let _held = &t2;
            Ok(vec![Value::Int(1)])
        })
        .unwrap();
        let d = lua.from_data(&Data::Table(vec![(Data::Str(format!("k{i}")), Data::Int(i as i64))])).unwrap();
        lua.set_global("last", d).unwrap();
        drop((rf, ru));
        if collect && i % 200 == 0 {
            lua.collect_garbage().unwrap();
        }
        if i % step == 0 {
            let sizes = lua.internal_sizes();
            let changed: Vec<String> = sizes
                .iter()
                .filter(|(k, v)| prev.iter().find(|(pk, _)| pk == k).is_none_or(|(_, pv)| pv != v))
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            println!(
                "{i:>8}: rss {:>6} KiB, used {:>6} KiB | {}",
                rss() >> 10,
                lua.memory_used() >> 10,
                changed.join(" ")
            );
            prev = sizes;
        }
    }
}
