# wasm-reserve

Times RemedyLua's array growth on wasm32-unknown-unknown (release) under Node. It chose the segmented array part's threshold and block size on wasm (2 MiB and 1 MiB).

- `node run.mjs` builds the crate, then measures:
  - plain `Vec` copies of 4–64 MiB;
  - table growths either side of `seg::FROM`, whole and sliced;
  - the steady state over 1 MiB working sets;
  - block makes of 256 KiB, 1 MiB and 4 MiB.
- `node slices.mjs target/wasm32-unknown-unknown/release/remedy_lua_wasm_reserve.wasm` prints the slowest slices of a 2^21 → 2^22 growth, with wall time, CPU time and the pages the memory grew by. Every slice over 2 ms is one that grew the memory.
- `node --trace-gc grow.mjs <same .wasm>` makes 128 blocks of 1 MiB, split by whether the memory grew, with V8's GCs (a Mark-Compact on nearly every `memory.grow`).

Run one at a time on a quiet machine, and give the load with the numbers. With `CARGO_TARGET_DIR` set, the .wasm is under that directory instead.
