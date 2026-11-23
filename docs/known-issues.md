# Known issues

The open issues in RemedyLua 0.1.1. Each has an ID that tests and code comments refer to.
`docs/reference.md` (Budgets) explains the limits these touch.

`tests/test_lists.rs` reads the status column below: a test marked `[known]` in
`tests/timing.list` must name an issue whose status says open.

| ID | Severity | Summary | Status |
|---|---|---|---|
| SEC7-G2 | high | What a collection cycle marks, and so what the next cycle charges in fuel, follows the order objects are reached in, which follows the hash seed. The same program under two seeds can stop at slightly different points under a fixed per-call fuel (measured: 18,313 against 18,401 calls). | open: a deterministic traversal order is designed; snapshot-at-the-beginning marking fixes it but costs more speed than the budget allows |
| SEC8-G3 | medium | At the memory limit, an allocation's full collection under a small per-call fuel stalls the program for many calls in a row (about 1,000 calls for 800,000 live keys at 512 MiB with 20,000 fuel a call). The work is charged, never skipped, and the program resumes once memory fits. | open: next step is measuring how much newborn garbage could be reclaimed earlier |
| SEC8-F4-L | low | A table rehash bigger than one call's fuel is done across calls. A host write that changes the table's layout between those calls discards the kept work, so a host that does so often enough keeps the script's write from going in. | open, documented limitation: leave such a table alone between calls, or give the calls more fuel |
| SEC8-S2 | medium | Native builds: the slab doubling that crosses glibc's mmap threshold copies the slab once, 10 to 14 ms of CPU at about 131,000 tables (18.9 MB). | open: embedder-side remedy (allocator choice or `M_MMAP_THRESHOLD`); a crate-side fix cost 1.86% speed and was declined |
| SEC8-F2 | low | Clearing the dead part of a thread's stack at the collector's atomic step charges the whole stack even when all of it is live: about a quarter of a fuel per slot on a big live stack. | open |
| SEG-W2 | info | The worst pause on wasm while a segmented array part grows has not been measured end to end. | open |
| API-R1 | info | The resumable host call (`Lua::start_call`, feature `unstable-resumable`) has no settled signature. | open: unstable, may change |

## Workarounds for hosts

- Give a state memory headroom above its scripts' working set, or larger calls, to avoid
  the stall at the memory limit (SEC8-G3). `Lua::collector_status` shows the collector's
  phase, the fuel it is already owed, and a floor on what the cycle under way still needs.
- For results that must reproduce exactly across runs under a tight per-call fuel, fix
  `Limits::hash_seed` (SEC7-G2) and keep the seed secret from scripts.
- On native targets with many small tables, benchmark the host's allocator settings
  (SEC8-S2).
