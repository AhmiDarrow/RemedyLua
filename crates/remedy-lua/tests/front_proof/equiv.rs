//! Old (recursive) against new (iterative) front end on the corpus and on seeded
//! mutations of it: the same syntax tree or the same error at the same line and column,
//! then the same bytecode, constants, upvalues and locals for every prototype or the same
//! compile error. Programs the old front end refuses only for its nesting limit are
//! skipped (the new one must still not fail on them other than cleanly).

use remedy_lua::__front::{compile_new, compile_old, parse_new, parse_old};

use super::corpus::corpus;
use super::mutate::mutate;
use super::rng::Rng;

fn old_limit(e: &str) -> bool {
    e.contains("too many syntax levels") || e.contains("expression too complex")
}

/// Compares one program; `Err` describes the first difference.
fn same(name: &str, src: &str) -> Result<(), String> {
    let b = src.as_bytes();
    let old = parse_old(b, name);
    if old.as_ref().is_err_and(|e| old_limit(e)) {
        let _ = compile_new(b, name);
        return Ok(());
    }
    let new = parse_new(b, name);
    if old != new {
        return Err(format!("parse differs for {name}:\n{src}\nold: {old:?}\nnew: {new:?}"));
    }
    if old.is_err() {
        return Ok(());
    }
    let old = compile_old(b, name);
    if old.as_ref().is_err_and(|e| old_limit(e)) {
        let _ = compile_new(b, name);
        return Ok(());
    }
    let new = compile_new(b, name);
    if old != new {
        return Err(format!("compile differs for {name}:\n{src}\nold: {old:#?}\nnew: {new:#?}"));
    }
    Ok(())
}

#[test]
fn corpus_old_and_new_agree() {
    let c = corpus();
    let diffs: Vec<_> = c.iter().filter_map(|(n, s)| same(n, s).err()).collect();
    assert!(diffs.is_empty(), "{} of {} programs differ; first:\n{}", diffs.len(), c.len(), diffs[0]);
}

/// Seeded mutations (cut, duplicate, swap and replace tokens, splice lines):
/// `REMEDYLUA_FRONT_ITERS` of them (default 2000).
#[test]
fn mutated_corpus_old_and_new_agree() {
    let iters: u64 = std::env::var("REMEDYLUA_FRONT_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(2000);
    let c = corpus();
    let pool: Vec<String> = c.iter().map(|(_, s)| s.clone()).collect();
    let mut rng = Rng::new(0xf0f0_1234);
    for i in 0..iters {
        let k = rng.below(c.len());
        let m = mutate(&c[k].1, &mut rng, &pool);
        let name = if c[k].0.ends_with(".rlua") { "mut.rlua" } else { "mut.lua" };
        if let Err(d) = same(name, &m) {
            panic!("mutation {i} of {}:\n{d}", c[k].0);
        }
    }
}
