//! Proof that the iterative front end replaces the recursive one safely: a corpus of valid
//! and invalid programs (`corpus`), old-against-new equivalence on it and on its mutations
//! (`equiv`, feature `front-oracle`), programs nested 10k and 100k deep that compile,
//! run and agree with C Lua where C Lua manages (`deep`), clean memory errors under small
//! caps (`memcap`), and parse + compile + drop of 100k-deep programs on a 64 KiB stack
//! (`norec`).
//!
//! `cargo test -p remedy-lua --test all front_proof` (add `--features front-oracle` for the
//! equivalence checks; `REMEDYLUA_FRONT_ITERS=<n>` for a longer fuzz pass).

#[path = "../sweep1_fuzz/grammar.rs"]
#[allow(dead_code)]
mod grammar;
#[path = "../sweep1_fuzz/mutate.rs"]
#[allow(dead_code)]
mod mutate;
#[path = "../sweep1_fuzz/rng.rs"]
#[allow(dead_code)]
mod rng;

pub mod corpus;
mod deep;
#[cfg(feature = "front-oracle")]
mod equiv;
mod memcap;
mod norec;
