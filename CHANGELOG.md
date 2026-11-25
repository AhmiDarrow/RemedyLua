# Changelog

## Unreleased

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
