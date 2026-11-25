# Known issues

The open issues in RemedyLua 0.1.2. Each has an ID that tests and code comments refer to.
`docs/reference.md` (Budgets) explains the limits these touch.

`tests/test_lists.rs` reads the status column below: a test marked `[known]` in
`tests/timing.list` must name an issue whose status says open.

| ID | Severity | Summary | Status |
|---|---|---|---|
| SEC8-G3 | medium | At the memory limit, an allocation's full collection under a small per-call fuel stalls the program for many calls in a row when incremental collection cannot free enough memory first. The work is charged, never skipped, and the program resumes once memory fits. | mitigated on the 800,000-key workload; worst-case progress remains open |
| SEC8-F4-L | low | A table rehash bigger than one call's fuel is done across calls. A host write that changes the table's layout between those calls discards the kept work, so a host that does so often enough keeps the script's write from going in. | open, documented limitation: leave such a table alone between calls, or give the calls more fuel |
| SEC8-S2 | medium | Native builds: the slab doubling that crosses glibc's mmap threshold copies the slab once, 10 to 14 ms of CPU at about 131,000 tables (18.9 MB). | open: embedder-side remedy (allocator choice or `M_MMAP_THRESHOLD`); a crate-side fix cost 1.86% speed and was declined |
| SEC8-F2 | low | Clearing the dead part of a thread's stack at the collector's atomic step charges the whole stack even when all of it is live: about a quarter of a fuel per slot on a big live stack. | open |
| SEG-W2 | info | The worst pause on wasm while a segmented array part grows has not been measured end to end. | open |
| API-R1 | info | The resumable host call (`Lua::start_call`, feature `unstable-resumable`) has no settled signature. | open: unstable, may change |

## SEC8-G3 retention at the memory cap

Before the pacing change, on the 800,000-key workload, the state first came within 32 MiB of the cap on call
1,891 and within 1 KiB on call 2,187. During those 296 calls the script appended
165,711 distinct new tables to its weak-value table. At the second snapshot, 560
still had a strong owner. A full collection cleared the other 165,151 and kept all
560 survivors. Seeds 1 and 7 gave the same counts. The probe stores unrooted
handles on the host and runs the full collection only after the second snapshot.

To repeat the measurement:

```sh
cargo test --release -p remedy-lua --test all -- --exact --ignored --nocapture sweep6::sec8_g3_newborn_retention
```

Compare collector changes over at least 8,000 calls as well as the initial
3,300. Record both the longest run without script progress and the number of
inserts completed. Earlier reclamation alone shortened the first stall in a
prototype but increased the longest stall over the longer run.

## Workarounds for hosts

- Give a state memory headroom above its scripts' working set, or larger calls, to avoid
  the stall at the memory limit (SEC8-G3). `Lua::collector_status` shows the collector's
  phase, the fuel it is already owed, and a floor on what the cycle under way still needs.
- On native targets with many small tables, benchmark the host's allocator settings
  (SEC8-S2).
