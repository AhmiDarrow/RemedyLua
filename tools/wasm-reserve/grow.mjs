// 128 blocks of 1 MiB made and kept, each timed, split by whether the wasm memory grew.
// Usage: node grow.mjs <the .wasm run.mjs built>; with --trace-gc, the V8 GCs between.
import { readFileSync } from "node:fs";
const wasm = readFileSync(process.argv[2]);
for (let run = 0; run < 3; run++) {
  const x = (await WebAssembly.instantiate(wasm, {})).instance.exports;
  const grew = [], flat = [];
  for (let i = 0; i < 128; i++) {
    const m0 = x.memory.buffer.byteLength, t0 = performance.now();
    x.alloc_block(1 << 16);
    const t = performance.now() - t0;
    (x.memory.buffer.byteLength > m0 ? grew : flat).push([t, m0 >> 20]);
  }
  const q = (a) => { const t = a.map((v) => v[0]).sort((p, r) => p - r); return t.length ? `n ${t.length} median ${t[t.length >> 1].toFixed(2)} max ${t[t.length - 1].toFixed(2)}` : "n 0"; };
  const big = grew.filter((v) => v[0] > 2).map((v) => `${v[0].toFixed(1)}ms@${v[1]}MiB`);
  console.log(`1 MiB blocks kept, 128 of them: with memory.grow ${q(grew)}; without ${q(flat)}; >2 ms: ${big.join(" ")}`);
}
