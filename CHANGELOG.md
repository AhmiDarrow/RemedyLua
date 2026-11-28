# Changelog

## [Unreleased]

### Added

- Typed host functions: `Lua::create_typed`, `create_typed_mut`, `register_typed` and
  `register_typed_mut` take `Fn(&mut Lua, A) -> Result<R, Error>`, with arguments read
  through the new `FromLua` / `FromLuaMulti` traits (tuples up to twelve, `Variadic<T>`
  for the rest) and results made through `IntoLua` / `IntoLuaMulti`. Mismatches give C
  Lua's `bad argument #n to 'f' (...)` messages. Copying strings and `Data` out of the
  script is charged fuel. See "Typed host functions" in `docs/reference.md`.
- `Error::BadArgument { position, message }`, which any host function can return to raise
  a named argument error.
- `serde` feature: `Serialize` / `Deserialize` for `Data` (sequences for tables keyed
  `1..=n`, maps otherwise; nesting capped at `MAX_SERDE_DEPTH`, 64). The default build
  still has no dependencies.
- Host-compiled chunks: `Lua::compile` / `compile_with` give a `CompiledChunk` (`Arc`,
  `Send + Sync`) that `Lua::load_compiled` instantiates in any state without compiling
  again: the same results, fuel, errors and debug info as `load` of the source, with
  its memory charged (and, inside a call, the fuel of copying it). A fresh state with a
  2,000-line prelude starts ~19x faster (`examples/precompiled.rs`). There is no way to
  make a chunk from bytes, and `load` still refuses binary chunks.

## [0.1.2] - 2026-11-25

### Fixed

- Incremental marking visits children of hash tables in a deterministic order (SEC7-G2).
  Overwrites during marking now give the same progress across hash seeds on the
  multi-cycle collector regression.
- Table constructors can reuse a free slab slot at the memory cap when the net
  allocation fits. This avoids an unnecessary emergency collection.
- Under memory pressure, fuel-limited calls collect more frequently and can reclaim
  newly constructed Lua tables within the current marking cycle (SEC8-G3).

## [0.1.1] - 2026-11-23

### Fixed

- README and API-doc links point at the GitHub repository, so they work on crates.io and docs.rs.

## [0.1.0] - 2026-11-22

### Added

- RemedyLua 0.1: the language (lexer, parser, compiler, VM, incremental GC, standard library, limits, host API).
- `Lua::collector_status`: the collector's phase, the fuel already owed to it, and a floor on what the cycle under way still needs (`CollectorStatus`, `CollectorPhase`).

### Known limitations

See [`docs/known-issues.md`](docs/known-issues.md) for every open issue.

- A collection cycle's fuel charge can differ with the hash seed (SEC7-G2): the same program run under two seeds can stop at slightly different points under a fixed per-call fuel.
- At the memory limit, a program that allocates fast can stall for many calls while the allocation's full collection runs under a small per-call fuel (see Budgets in `docs/reference.md`).
- A table rehash made across calls starts again when the host changes that table's layout between the calls, so a host that does so often enough keeps the script's write from going in (see Budgets in `docs/reference.md`).
