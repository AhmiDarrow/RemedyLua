// Builds the smoke crate for wasm32-unknown-unknown and runs `smoke()` under Node.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const target = process.env.CARGO_TARGET_DIR || join(here, "target");
execFileSync("cargo", ["build", "--target", "wasm32-unknown-unknown", "--manifest-path", join(here, "Cargo.toml")], {
  stdio: "inherit",
  env: { ...process.env, CARGO_TARGET_DIR: target },
});
const wasm = readFileSync(join(target, "wasm32-unknown-unknown", "debug", "remedy_lua_wasm_smoke.wasm"));
const { instance } = await WebAssembly.instantiate(wasm, {});
const r = instance.exports.smoke();
console.log(r === 1 ? "wasm smoke: ok" : `wasm smoke: FAILED at step ${-r}`);
process.exit(r === 1 ? 0 : 1);
