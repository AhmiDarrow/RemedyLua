// Each slice of a 2^21 -> 2^22 growth sliced every 2 reads: wall, CPU, and the pages the
// wasm memory grew by. Usage: node slices.mjs <the .wasm run.mjs built>
import { readFileSync } from "node:fs";
const wasm = readFileSync(process.argv[2]);
for (let run = 0; run < 6; run++) {
  const x = (await WebAssembly.instantiate(wasm, {})).instance.exports;
  x.make_table(21, 2); x.copied();
  const s = [];
  for (;;) {
    const m0 = x.memory.buffer.byteLength, c0 = process.cpuUsage(), t0 = performance.now();
    const r = x.grow_table(21);
    const t = performance.now() - t0, c = process.cpuUsage(c0);
    s.push([t, (c.user + c.system) / 1000, (x.memory.buffer.byteLength - m0) >> 16]);
    if (r === 1) break;
  }
  const top = s.map((v, i) => [...v, i]).sort((a, b) => b[0] - a[0]).slice(0, 3);
  console.log(top.map(([t, cpu, pages, i]) => `#${i}: wall ${t.toFixed(2)} cpu ${cpu.toFixed(2)} grew ${pages} pages`).join(" | "),
    "| slices >2ms with growth:", s.filter((v) => v[0] > 2 && v[2] > 0).length, "without:", s.filter((v) => v[0] > 2 && v[2] === 0).length);
}
