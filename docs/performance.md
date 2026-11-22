# Performance

RemedyLua against C Lua 5.4.7, wall time, with RemedyLua's metering on (fuel and memory
counted on every step).

**Method.** Release build with layout-neutral code alignment (`-C
llvm-args=-align-all-functions=6 -C llvm-args=-align-all-nofallthru-blocks=5`), the
`examples/prof` runner, pinned to one core, best of 3 per process over 8 interleaved
rounds; C Lua timed in the same processes. Moves under about 3% are within the noise of
the machine used. Numbers are from version 0.1.0.

## The main suite (`benches/lua`)

| Benchmark | C Lua ms | RemedyLua ms | Ratio |
|---|---|---|---|
| binary_trees | 373.4 | 415.8 | 1.11x |
| closures | 73.8 | 96.3 | 1.30x |
| concat | 88.8 | 80.9 | 0.91x |
| coroutines | 51.9 | 64.2 | 1.24x |
| deltablue | 103.1 | 135.3 | 1.31x |
| fannkuch | 105.0 | 131.2 | 1.25x |
| fib | 105.1 | 155.4 | 1.48x |
| format | 131.6 | 119.2 | 0.91x |
| gc_churn | 176.1 | 210.7 | 1.20x |
| json | 129.2 | 200.7 | 1.55x |
| loops | 66.9 | 77.3 | 1.16x |
| method_calls | 126.2 | 171.1 | 1.36x |
| nbody | 138.7 | 154.5 | 1.11x |
| patterns | 72.0 | 98.1 | 1.36x |
| richards | 138.7 | 171.7 | 1.24x |
| sort | 131.7 | 144.2 | 1.09x |
| spectral_norm | 73.4 | 120.1 | 1.64x |
| string_build | 99.5 | 117.9 | 1.18x |
| string_ops | 86.1 | 114.7 | 1.33x |
| table_array | 45.5 | 53.5 | 1.18x |
| table_churn | 86.8 | 80.9 | 0.93x |
| **Geometric mean** | | | **1.216x** |

## The extra suite (`benches/lua-extra`)

Programs aimed at the paths that are slowest relative to C Lua.

| Benchmark | C Lua ms | RemedyLua ms | Ratio |
|---|---|---|---|
| byte_char | 34.9 | 53.3 | 1.53x |
| closures_loop | 45.4 | 66.9 | 1.47x |
| deep_recursion | 74.2 | 88.7 | 1.20x |
| format_log | 87.1 | 77.3 | 0.89x |
| goto_fsm | 53.6 | 67.7 | 1.26x |
| int_bitwise | 51.1 | 126.1 | 2.47x |
| metamethods | 24.4 | 69.6 | 2.85x |
| oop_index_fn | 55.2 | 129.7 | 2.35x |
| utf8_ops | 60.1 | 51.0 | 0.85x |
| varargs | 59.3 | 115.7 | 1.95x |
| **Geometric mean** | | | **1.555x** |

## Running the benchmarks

    cargo run --release -p remedy-lua --example benchsuite

The goal for the main suite is a geometric mean at or under 1.2x. Changes are held to
+0.5% mean and +2% on any one benchmark, by instruction counts with `codegen-units=1`
(see `CONTRIBUTING.md`).

Profile-guided optimization helps only on the workload it was trained on; see Building for
speed in `docs/reference.md`.
