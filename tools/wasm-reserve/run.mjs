// Builds the crate for wasm32-unknown-unknown (release) and times its exports under Node.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const target = process.env.CARGO_TARGET_DIR || join(here, "target");
execFileSync("cargo", ["build", "--release", "--target", "wasm32-unknown-unknown", "--manifest-path", join(here, "Cargo.toml")], {
  stdio: "inherit",
  env: { ...process.env, CARGO_TARGET_DIR: target },
});
const wasm = readFileSync(join(target, "wasm32-unknown-unknown", "release", "remedy_lua_wasm_reserve.wasm"));
const fresh = async () => (await WebAssembly.instantiate(wasm, {})).instance.exports;
const ms = (f) => { const t = performance.now(); const r = f(); return [performance.now() - t, r]; };
for (const mib of [4, 8, 16, 32, 64]) {
  const runs = [];
  for (let i = 0; i < 5; i++) {
    const x = await fresh();
    x.make_vec(mib);
    runs.push(ms(() => x.grow_vec()));
  }
  const t = runs.map((r) => r[0]).sort((a, b) => a - b);
  console.log(`reserve ${mib} -> ${2 * mib} MiB: median ${t[2].toFixed(2)} ms (min ${t[0].toFixed(2)}, max ${t[4].toFixed(2)}), moved: ${runs.map((r) => r[1]).join("")}`);
}
// The table growths: 2^bits -> 2^(bits + 1) from a fresh table (seg::FROM is 2 MiB, 2^17
// slots: 2^16 is the last one copied, 2^17 the first made segmented; 2^20 and 2^21 are
// segmented from 2^17 on), and 2^22 -> 2^23 of one segmented already.
const grow = async (bits, every, pre) => {
  const x = await fresh();
  if (x.make_table(bits, every) !== 1) throw new Error("setup");
  if (pre) { for (let b = bits; b < pre; b++) { while (x.grow_table(b) === 0); } }
  const at = pre || bits;
  x.copied();
  const slices = [];
  for (;;) {
    const [t, r] = ms(() => x.grow_table(at));
    slices.push(t);
    if (r === 1) break;
    if (r < 0 || slices.length > 100000) throw new Error(`grow: ${r}`);
  }
  const c = x.copied();
  const total = slices.reduce((a, b) => a + b, 0);
  console.log(`table 2^${at} -> 2^${at + 1}${pre ? " (segmented already)" : ""}, every ${every} reads: ${slices.length} calls, ${total.toFixed(2)} ms in all, longest ${Math.max(...slices).toFixed(2)} ms; slots copied ${c >> 4}, made segmented ${c & 15}`);
  return x;
};
for (const bits of [16, 17, 20, 21]) for (const every of [0, 2, 9]) await grow(bits, every, 0);
for (const every of [0, 2]) await grow(21, every, 22);
// Steady state: read+write loops over 2^16 slots (1 MiB) of a filled table, a segmented one
// (2^22 slots: block 0 the first 2^17, blocks after) in block 0 and past it, and a
// contiguous one (2^17 slots, grown from 2^16 by a copy).
{
  const seg = await grow(21, 0, 0);
  if (seg.fill(1 << 22) !== 1) throw new Error("fill");
  const flat = await grow(16, 0, 0);
  if (flat.fill(1 << 17) !== 1) throw new Error("fill");
  const n = 1 << 16;
  for (let r = 0; r < 2; r++) {
    for (const [name, x, lo] of [["segmented, block 0", seg, 1], ["segmented, past block 0", seg, (1 << 21) + 1], ["contiguous", flat, (1 << 16) + 1]]) {
      const [t, ok] = ms(() => x.rw(lo, lo + n - 1, 4));
      if (ok !== 1) throw new Error(`rw ${name}`);
      console.log(`read+write loop, ${name}: ${(t * 1e6 / (4 * n)).toFixed(2)} ns a slot`);
    }
  }
}
// A block's making (allocation and its nil fill), 8 in a row each size.
for (const slots of [1 << 14, 1 << 16, 1 << 18]) {
  const x = await fresh();
  const t = [];
  for (let i = 0; i < 8; i++) t.push(ms(() => x.alloc_block(slots))[0]);
  t.sort((a, b) => a - b);
  console.log(`block of ${slots} slots (${slots * 16 >> 10} KiB): median ${t[4].toFixed(3)} ms, max ${t[7].toFixed(3)} ms`);
}
