# Contributing to RemedyLua

RemedyLua is a strict superset of Lua 5.4 in safe Rust. Thank you for helping.

## The rules

- No `unsafe`, anywhere.
- Fuel is the same in the fast loop and the full path, and does not depend on the hash seed.
- Never loosen a test to make it pass. Changing a test's bound needs the maintainer's agreement.
- Every fix comes with a regression test that fails without it.
- The fewest lines that do the job.
- Speed is part of correctness: a change stays within +0.5% mean and +2% on any one of the
  21 benchmarks (instruction counts, `codegen-units=1`), unless the maintainer agrees otherwise.

## Building and testing

Set `LUA_SRC` to Lua 5.4.7's sources (`tools/gate.sh` finds the `lua-src` crate's copy in
the cargo registry when it is unset), then run the gate for your change's tier on a quiet machine
(see [`docs/testing.md`](docs/testing.md)):

    tools/gate.sh <tier>

`tools/gate.sh` lists the tiers (`docs`, `tests`, `helper`, `stdlib`, `front`, `core`) and
which files take which. Timing tests run apart from the rest and are listed in
`crates/remedy-lua/tests/timing.list`; a test that measures time must be listed there.
CI runs the steps that do not measure time (`tools/gate.sh ci`) on every pull request; it
does not replace running your tier's gate.

## Where the open work is

[`docs/known-issues.md`](docs/known-issues.md) lists every open issue with its ID, severity
and status. The collector can stall at the memory limit under a small per-call fuel (SEC8-G3).

Open an issue before a large change, so the design can be agreed first.
