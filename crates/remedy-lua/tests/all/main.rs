//! Every integration suite in one test binary, so a change links once.
//! Run one suite or test with a filter: `cargo test -p remedy-lua --test all sweep1_sandbox::s9`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

#[path = "../common/mod.rs"]
mod common;

#[path = "../conformance.rs"]
mod conformance;
#[path = "../differential.rs"]
mod differential;
#[path = "../front_equiv.rs"]
mod front_equiv;
#[path = "../front_proof/mod.rs"]
mod front_proof;
#[path = "../gc_slab.rs"]
mod gc_slab;
#[path = "../host.rs"]
mod host;
#[path = "../immediates.rs"]
mod immediates;
#[path = "../libs_capability.rs"]
mod libs_capability;
#[path = "../libs_coverage.rs"]
mod libs_coverage;
#[path = "../parity_lua54.rs"]
mod parity_lua54;
#[path = "../perf_arch.rs"]
mod perf_arch;
#[path = "../perf_c_gap.rs"]
mod perf_c_gap;
#[path = "../perf_calls.rs"]
mod perf_calls;
#[path = "../perf_natives.rs"]
mod perf_natives;
#[path = "../perf_numeric.rs"]
mod perf_numeric;
#[path = "../perf_quicken.rs"]
mod perf_quicken;
#[path = "../perf_tables.rs"]
mod perf_tables;
#[path = "../red_team.rs"]
mod red_team;
#[path = "../residuals.rs"]
mod residuals;
#[path = "../rlua_types.rs"]
mod rlua_types;
#[path = "../sec_metering.rs"]
mod sec_metering;
#[path = "../smoke.rs"]
mod smoke;
#[path = "../strings.rs"]
mod strings;
#[path = "../superset.rs"]
mod superset;
#[path = "../sweep1_fuzz.rs"]
mod sweep1_fuzz;
#[path = "../sweep1_internals.rs"]
mod sweep1_internals;
#[path = "../sweep1_sandbox.rs"]
mod sweep1_sandbox;
#[path = "../sweep1_soak.rs"]
mod sweep1_soak;
#[path = "../sweep1_spec.rs"]
mod sweep1_spec;
#[path = "../sweep2_equiv.rs"]
mod sweep2_equiv;
#[path = "../sweep2_sec.rs"]
mod sweep2_sec;
#[path = "../sweep3_sec.rs"]
mod sweep3_sec;
#[path = "../sweep3_spec.rs"]
mod sweep3_spec;
#[path = "../sweep3_stab.rs"]
mod sweep3_stab;
#[path = "../sweep4_api.rs"]
mod sweep4_api;
#[path = "../sweep4_fuzz.rs"]
mod sweep4_fuzz;
#[path = "../sweep4_sec.rs"]
mod sweep4_sec;
#[path = "../sweep5_api.rs"]
mod sweep5_api;
#[path = "../sweep5_cor.rs"]
mod sweep5_cor;
#[path = "../sweep5_gc.rs"]
mod sweep5_gc;
#[path = "../sweep5_sec.rs"]
mod sweep5_sec;
#[path = "../sweep5b_cost.rs"]
mod sweep5b_cost;
#[path = "../sweep5b_open.rs"]
mod sweep5b_open;
#[path = "../sweep5b_walks.rs"]
mod sweep5b_walks;
#[path = "../sweep5c_front.rs"]
mod sweep5c_front;
#[path = "../sweep5c_open2.rs"]
mod sweep5c_open2;
#[path = "../sweep6.rs"]
mod sweep6;
#[path = "../sweep6_err.rs"]
mod sweep6_err;
#[path = "../sweep6_front.rs"]
mod sweep6_front;
#[path = "../sweep6_gcpart.rs"]
mod sweep6_gcpart;
#[path = "../sweep6_heap.rs"]
mod sweep6_heap;
#[path = "../sweep6_review.rs"]
mod sweep6_review;
#[path = "../sweep6_tables.rs"]
mod sweep6_tables;
#[path = "../tables.rs"]
mod tables;
#[path = "../test_lists.rs"]
mod test_lists;
#[path = "../typed.rs"]
mod typed;
