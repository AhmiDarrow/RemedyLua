//! Deep programs under small memory caps: a clean memory error (never a panic, an abort
//! or another error), the state usable afterwards, and between the caps that refuse and
//! the caps that fit, nothing but those two outcomes.

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua};

use super::deep::{ALL, Kind, chunk, program};

/// Loads and runs `src` under `cap` bytes: `Ok(result)`, or `Err(None)` for the memory
/// error, `Err(Some(other error))` otherwise. Afterwards the state must still run code.
fn under(src: &str, name: &str, cap: usize) -> Result<String, Option<String>> {
    let mut lua = Lua::new(Limits::default().memory(cap).fuel(u64::MAX), Libs::ALL);
    let r = match lua.exec(src, name) {
        Ok(v) => Ok(v.first().map_or_else(|| "nil".to_owned(), |x| crate::common::show(&lua, *x))),
        Err(Error::Limit(LimitKind::Memory)) => Err(None),
        Err(e) => Err(Some(e.to_string())),
    };
    let after = lua.exec("return 40 + 2", "=after").map_err(|e| e.to_string());
    assert!(
        matches!(after.as_deref(), Ok([remedy_lua::Value::Int(42)])),
        "state unusable after a {cap}-byte run: {after:?}"
    );
    r
}

#[test]
fn small_caps_refuse_100k_deep_programs_cleanly() {
    let _big = (crate::common::memory_heavy(), super::deep::BIG.lock().unwrap_or_else(|e| e.into_inner()));
    for &kind in ALL {
        if matches!(kind, Kind::Interp) {
            continue; // the recursive lexer aborts before memory runs out: see `deep::pending`
        }
        let (src, _) = program(kind, 100_000);
        for cap in [256 << 10, 1 << 20] {
            assert_eq!(under(&src, chunk(kind), cap), Err(None), "{kind:?} 100k deep under {cap} bytes");
        }
    }
}

/// From 256 KiB up to 64 MiB, each 10k-deep program either runs right or is refused for
/// memory; once it fits, every larger cap fits too.
#[test]
fn caps_between_refuse_and_fit_give_only_those_two_outcomes() {
    for &kind in ALL {
        if !matches!(kind, Kind::Interp) {
            check_sweep(kind);
        }
    }
}

#[test]
#[ignore = "nested interpolations are quadratic in their depth"]
fn interpolations_under_caps_give_only_memory_errors() {
    check_sweep(Kind::Interp);
    let (src, _) = program(Kind::Interp, 100_000);
    assert_eq!(under(&src, chunk(Kind::Interp), 256 << 10), Err(None));
}

fn check_sweep(kind: Kind) {
    let (src, want) = program(kind, 10_000);
    let mut fitted = None;
    let mut cap = 256 << 10;
    while cap <= 64 << 20 {
        match under(&src, chunk(kind), cap) {
            Ok(got) => {
                assert_eq!(got, want, "{kind:?} under {cap} bytes");
                fitted.get_or_insert(cap);
            }
            Err(None) => assert!(fitted.is_none(), "{kind:?}: fits in {fitted:?} bytes but not in {cap}"),
            Err(Some(e)) => panic!("{kind:?} under {cap} bytes: not a memory error: {e}"),
        }
        cap += cap / 2;
    }
    assert!(fitted.is_some(), "{kind:?} 10k deep does not fit in 64 MiB");
}
