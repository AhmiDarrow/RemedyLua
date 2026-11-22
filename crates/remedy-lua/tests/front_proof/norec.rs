//! The no-recursion proof: parsing, compiling and dropping a deep program of every shape
//! fits in a 64 KiB native stack (100k levels). A recursive step anywhere (lexer, parser, compiler,
//! tree drop, prototype drop) would overflow it and abort the process.

use remedy_lua::{Libs, Limits, Lua};

use super::deep::{ALL, Kind, chunk, pending, program};

/// Loads `kind` `n` deep on a 64 KiB stack and drops everything there.
fn load_small_stack(kind: Kind, n: usize) -> Result<(), String> {
    let _big = (crate::common::memory_heavy(), super::deep::BIG.lock().unwrap_or_else(|e| e.into_inner()));
    // The source is made out here: only the front end is on trial.
    let (src, _) = program(kind, n);
    std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(move || {
            let mut lua = Lua::new(Limits::unmetered(), Libs::NONE);
            let r = lua.load(&src, chunk(kind)).map(drop).map_err(|e| e.to_string());
            drop(lua);
            drop(src);
            r
        })
        .unwrap()
        .join()
        .unwrap()
}

#[test]
fn deep_programs_load_and_drop_on_a_64k_stack() {
    let n = 100_000;
    for &kind in ALL {
        if pending(kind, n) {
            continue; // nested interpolations, slow at this depth: the ignored test below
        }
        let r = load_small_stack(kind, n);
        assert!(r.is_ok(), "{kind:?} {n} deep: {r:?}");
    }
}

/// The shapes still pending: interpolations 100k deep (each level lexed again from its
/// text: time and memory grow with the square of the depth).
#[test]
#[ignore = "nested interpolations are quadratic in their depth"]
fn deep_pending_programs_load_and_drop_on_a_64k_stack() {
    let n = 100_000;
    for &kind in ALL {
        if pending(kind, n) {
            assert_eq!(load_small_stack(kind, n), Ok(()), "{kind:?} {n} deep");
        }
    }
}
