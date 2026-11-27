# Testing

RemedyLua is tested against C Lua 5.4.7 itself: every program a test runs is also run by
the reference interpreter, and the results, errors and output must match (see Testing in
`docs/reference.md` for the one class of accepted difference).

## The C Lua oracle

The `remedy-lua-oracle` crate builds C Lua 5.4.7 from source for the tests and
benchmarks. It is test-only and never published. Point `LUA_SRC` at the directory that
holds `lua.h` and the other files of Lua 5.4.7's `src/`:

    LUA_SRC=/path/to/lua-5.4.7/src cargo test -p remedy-lua --test all

Without `LUA_SRC`, the test build fails with a message saying how to set it: a comparison
is never skipped. `tools/gate.sh` sets it to the `lua-src` crate's copy in the cargo
registry when it is unset (run `cargo fetch` first).

## What the suites cover

All integration tests are one binary (`tests/all/main.rs`). They include:

- **Conformance and parity** with C Lua 5.4.7: the language, every library function, error
  messages and positions.
- **Differential fuzzers** (`sweep1_fuzz`, `sweep4_fuzz`): generated and mutated programs,
  run by both interpreters and compared.
- **Sandbox and red-team tests**: escapes, resource exhaustion and limit evasion; each must
  end with an error or a correct result, never a hang, crash or escape.
- **Metering**: fuel and memory charges are exact, identical with the fast loop on and off
  (`REMEDYLUA_TEST_NO_FAST_LOOP=1`), and independent of the hash seed.
- **Collector stress and soak tests**, the host API, the `.rlua` type checker, and the
  front end's iterative parser and compiler against the recursive reference ones
  (`--features front-oracle`).
- **WebAssembly smoke test**: `node tools/wasm-smoke/smoke.mjs` builds for
  `wasm32-unknown-unknown` and runs a script under Node with no imports.

## The gate

Run `tools/gate.sh <tier>` on a quiet machine (no other build or test running, since some
steps measure time). It builds everything first and exits 0 only if every step passed.
Logs go in `target/gate/<time>/`.

| Step | Runs |
|---|---|
| D | every test but the timing ones, in parallel |
| Dn | D with `REMEDYLUA_TEST_NO_FAST_LOOP=1` |
| T | the timing tests, one at a time |
| Tn | the timing tests that follow `REMEDYLUA_TEST_NO_FAST_LOOP`, with it set |
| O | the tests `--features front-oracle` adds or changes |
| L | the unit tests (`--lib`) |
| S | the release collector stress over the corpus |
| W | the wasm smoke test |

| The change touches | Tier | Steps |
|---|---|---|
| only `*.md` files | `docs <base>` | checks the diff is docs only |
| only `tests/`, not `tests/common/` | `tests <name>...` | the changed tests, then D |
| `tests/common/` or a helper shared across test modules | `helper` | D T |
| a library leaf: `stdlib/*` except `mod.rs`, plus `fmt.rs` | `stdlib` | D T L |
| the front end: lexer, parser, compiler, ast, check, diag | `front` | D O T L W |
| anything else (heap, collector, tables, VM, `lib.rs`, Cargo files, build scripts) | `core` | D Dn T Tn O L S W |

A change that spans tiers takes the highest one.

Policy:

- A deterministic failure (D, Dn, O, L, S, W or the changed tests) fails the gate.
- A failure in T or Tn is rerun alone three times and passes only if all three pass.

### CI

`.github/workflows/ci.yml` runs on every pull request and push to `main`, with Rust 1.96.0
and the checksummed Lua 5.4.7 sources: `tools/gate.sh ci` (D Dn O L W) on Linux,
`tools/gate.sh ci D` on Windows, and `cargo doc` with warnings denied. T, Tn and S measure
time or need a quiet machine, so shared runners do not run them: run the gate for your tier
locally as well.

## Test lists

`crates/remedy-lua/tests/timing.list` classifies the tests that read a clock:

- `[timing]`: results can depend on time; run by T.
- `[env]`: the timing tests that follow `REMEDYLUA_TEST_NO_FAST_LOOP`; run again by Tn.
- `[oracle]`: the tests `--features front-oracle` adds or changes; run by O.
- `[clock-deterministic]`, `[hang-guard]`, `[time-capped]`, `[probe]`: read a clock, but
  their results do not depend on time; they run in D.
- `[known]` (`module::fn ISSUE`): a test tied to an issue still open in
  [`known-issues.md`](known-issues.md). It runs and its result is printed, but a step whose
  only failures are `[known]` tests does not fail. The entry goes in the commit that fixes
  the issue.
- `[known-windows]`: the same, counted only when the gate runs on Windows, for an issue
  that shows there and not on Linux.

The `test_lists` guard (in D) fails when a listed name is not a test, a name is in two
classes, an `[env]` name is not in `[timing]`, a `[known]` entry names an issue that is not
open, or a test reads a clock without being listed.
