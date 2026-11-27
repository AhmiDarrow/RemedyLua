//! Sweep 1, area "sandbox": an attacker's view of RemedyLua's limits and host API.
//!
//! Every `#[ignore = "sweep1: …"]` test is a finding that FAILS today: the comment above
//! it says what is wrong, expected vs actual, the severity and the suspected place.
//! The tests without `ignore` are attacks that are stopped correctly (kept as regression
//! tests). `hostile_fuzz` is a random hostile-program generator; long run:
//! `SWEEP1_FUZZ_ITERS=20000 SWEEP1_FUZZ_SEED=7 cargo test --test sweep1_sandbox hostile_fuzz -- --nocapture`
//! (`SWEEP1_FUZZ_KNOWN=1` also mixes in the attacks of the known findings).
//!
//! Hangs are detected with a watchdog thread: a test that times out leaves its worker
//! spinning until the test process exits. Native (Rust-side) memory is measured by
//! running the attack in a child process (this test binary again, `child_entry`) and
//! reading that process's peak working set; a child also contains aborts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::panic::AssertUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

// ---------------------------------------------------------------- helpers

/// Runs `f` on a worker thread; `Err` when it panicked or did not finish in `secs`.
fn bounded<T: Send + 'static>(
    secs: u64,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<(T, Duration), String> {
    // (Never detached: `common::with_deadline`.)
    let r = super::common::with_deadline(16 << 20, Duration::from_secs(secs), move || {
        let start = Instant::now();
        let r = std::panic::catch_unwind(AssertUnwindSafe(f));
        r.map(|v| (v, start.elapsed()))
    });
    match r {
        Some(Ok(v)) => Ok(v),
        Some(Err(p)) => Err(format!("PANIC: {}", panic_text(&p))),
        None => Err(format!("HANG: still running after {secs} s")),
    }
}

fn panic_text(p: &Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<String>()
        .cloned()
        .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
        .unwrap_or_default()
}

/// Runs `src` in a fresh state; the outcome as text.
fn outcome(limits: Limits, libs: Libs, src: &str) -> String {
    let mut lua = Lua::new(limits, libs);
    describe(&mut lua, src)
}

fn describe(lua: &mut Lua, src: &str) -> String {
    match lua.exec(src, "=t") {
        Ok(v) => format!("ok {}", v.iter().map(|x| show(lua, *x)).collect::<Vec<_>>().join(",")),
        Err(Error::Limit(k)) => format!("limit {k:?}"),
        Err(e) => format!("error {}", e.message()),
    }
}

fn show(lua: &Lua, v: Value) -> String {
    match v {
        Value::Str(_) => lua.to_string_lossy(v).unwrap_or_default(),
        other => format!("{other:?}"),
    }
}

/// Runs `src` under `fuel` and the watchdog: must end (any way) within `secs`.
fn must_end(src: &str, limits: Limits, libs: Libs, secs: u64) -> String {
    let s = src.to_owned();
    match bounded(secs, move || outcome(limits, libs, &s)) {
        Ok((r, _)) => r,
        Err(e) => panic!("{e} (fuel {}): {}", limits.fuel, &src[..src.len().min(300)]),
    }
}

/// Time to burn `fuel` on `src` (which loops forever).
fn time_to_burn(src: &str, fuel: u64, libs: Libs) -> Duration {
    let mut lua = Lua::new(Limits::default().fuel(fuel), libs);
    let start = Instant::now();
    let r = lua.exec(src, "=t");
    assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{src}: {r:?}");
    start.elapsed()
}

fn alive(lua: &mut Lua) {
    assert_eq!(lua.exec("return 1 + 1", "=alive").unwrap(), vec![Value::Int(2)]);
}

/// The process's peak working set in bytes (Windows: via PowerShell; Linux: VmHWM).
fn peak_memory() -> u64 {
    if cfg!(windows) {
        let pid = std::process::id();
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("(Get-Process -Id {pid}).PeakWorkingSet64")])
            .output();
        out.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok()).unwrap_or(0)
    } else {
        std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM")).map(|l| l.to_owned()))
            .and_then(|l| l.split_whitespace().nth(1).and_then(|k| k.parse::<u64>().ok()))
            .map_or(0, |k| k * 1024)
    }
}

struct Child {
    /// Exit code (None: killed by a signal).
    code: Option<i32>,
    /// What the case printed as its result.
    result: String,
    /// Peak working set of the child, bytes.
    peak: u64,
    stderr: String,
}

/// Runs `case` (see `child_case`) in a child process.
fn in_child(case: &str) -> Child {
    let exe = std::env::current_exe().unwrap();
    let out = std::process::Command::new(exe)
        .args(["sweep1_sandbox::child_entry", "--exact", "--ignored", "--nocapture"])
        .env("SWEEP1_CHILD", case)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let field = |k: &str| text.lines().find_map(|l| l.strip_prefix(k)).unwrap_or_default().to_owned();
    Child {
        code: out.status.code(),
        result: field("SWEEP1_RESULT="),
        peak: field("SWEEP1_PEAK=").parse().unwrap_or(0),
        stderr: String::from_utf8_lossy(&out.stderr).chars().take(600).collect(),
    }
}

/// The attacks run in a child process (memory measured there; aborts contained there).
fn child_case(case: &str) -> String {
    match case {
        "baseline" => outcome(Limits::default().memory(1 << 20), Libs::SAFE, "return 1"),
        "sort_len" => outcome(
            Limits::default().memory(1 << 20).fuel(100_000_000),
            Libs::SAFE,
            "table.sort(setmetatable({}, {__len = function() return 20000000 end}))",
        ),
        "pack_c_abort" => outcome(
            Limits::default().memory(1 << 20),
            Libs::SAFE,
            "return string.pack('c100000000000000', '')",
        ),
        "utf8_codepoint" => outcome(
            Limits::default().memory(12 << 20),
            Libs::SAFE,
            "local s = string.rep('a', 8 << 20) return select('#', utf8.codepoint(s, 1, -1))",
        ),
        "parse_ast" => {
            let src = format!("return {{{}}}", "{},".repeat(3_000_000));
            let mut lua = Lua::new(Limits::default().memory(1 << 20), Libs::SAFE);
            match lua.load(&src, "=big") {
                Ok(_) => "loaded".into(),
                Err(e) => format!("{e:?}"),
            }
        }
        "warn_concat" | "print_concat" => {
            let f = if case == "warn_concat" { "warn" } else { "print" };
            let src = format!(
                "local s = string.rep('x', 1 << 20) local t = {{}} for i = 1, 300 do t[i] = s end
                 {f}(table.unpack(t)) return 'done'"
            );
            outcome(Limits::default().memory(4 << 20), Libs::STANDARD, &src)
        }
        // A host running scripts on a 1 MiB thread (the Windows main thread's size) with
        // default limits: the deepest native nesting the limits allow, then more native
        // recursion at the bottom.
        "small_stack" => {
            let h = std::thread::Builder::new().stack_size(1 << 20).spawn(|| {
                let deep = format!("return {}1{}", "(".repeat(150), ")".repeat(150));
                let src = format!(
                    "local deep = {deep:?}
                     local function f(n)
                       if n == 0 then
                         local ok1 = pcall(load, deep)
                         local ok2 = pcall(string.find, string.rep('a', 300), string.rep('(a?)', 32) .. string.rep('a', 300))
                         local ok3 = pcall(string.format, '%s', setmetatable({{}}, {{__tostring = function() return f(0) end}}))
                         return 'bottom'
                       end
                       return (string.gsub('a', 'a', function() return f(n - 1) end))
                     end
                     local r = {{}}
                     for i = 1, 200, 25 do r[i] = pcall(f, i) end
                     return 'survived'"
                );
                outcome(Limits::default(), Libs::ALL, &src)
            });
            h.unwrap().join().unwrap_or_else(|_| "panicked".into())
        }
        other => format!("unknown case {other}"),
    }
}

#[test]
#[ignore = "helper: run by the other tests in a child process"]
fn child_entry() {
    let Ok(case) = std::env::var("SWEEP1_CHILD") else { return };
    let r = child_case(&case);
    println!("SWEEP1_RESULT={r}");
    println!("SWEEP1_PEAK={}", peak_memory());
}

/// Exploration helper: `SWEEP1_PROBE=<lua source>` (libs ALL, `SWEEP1_FUEL`, `SWEEP1_MEM`).
#[test]
#[ignore = "helper: manual probing"]
fn probe() {
    let Ok(src) = std::env::var("SWEEP1_PROBE") else { return };
    let fuel = std::env::var("SWEEP1_FUEL").ok().and_then(|s| s.parse().ok()).unwrap_or(10_000_000);
    let mem = std::env::var("SWEEP1_MEM").ok().and_then(|s| s.parse().ok()).unwrap_or(64 << 20);
    let start = Instant::now();
    let r = outcome(Limits::default().fuel(fuel).memory(mem), Libs::ALL, &src);
    println!("PROBE {:?}: {}", start.elapsed(), r.chars().take(400).collect::<String>());
}

#[test]
fn small_native_stack_host_survives() {
    let c = in_child("small_stack");
    assert_eq!(c.code, Some(0), "child died on a 1 MiB stack: {} {}", c.result, c.stderr);
}

// ---------------------------------------------------------------- findings: work the fuel does not see

// S1 (critical, limit bypass): integer table keys hash without a seed
// (`value.rs:176 hash_key`: `i * K` then `h ^ h >> 29`, both invertible), and the hash part
// is linear probing (`table.rs:74 find`, `:185 raw_insert`). A script computes keys whose
// hashes all land in slot 0, so every insert and lookup walks all earlier keys: O(n) work
// for 1 fuel. Expected: 2M fuel ends in well under a second. Actual: 16k colliding keys
// and 200k lookups of the last one (~3e9 probes) run far past 10 s.
// Fix: seed integer/float key hashing per state (like strings), or charge fuel per probe.
#[test]
fn s1_integer_key_hash_flood() {
    // The inverse of the multiplier, so the script can pick `i` with `i * K == h`.
    const K: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut inv: u64 = 1;
    for _ in 0..6 {
        inv = inv.wrapping_mul(2u64.wrapping_sub(K.wrapping_mul(inv)));
    }
    assert_eq!(inv.wrapping_mul(K), 1);
    let src = format!(
        "local KI = {} local t, last = {{}}, nil
         for j = 1, 16000 do
           local g = j << 21
           local h = g ~ (g >> 29) ~ (g >> 58)
           last = h * KI
           t[last] = true
         end
         for r = 1, 200000 do local _ = t[last] end
         return 'done'",
        inv as i64
    );
    let r = must_end(&src, Limits::default().fuel(2_000_000), Libs::SAFE, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S2 (critical, limit bypass): `#t` falls back to a linear walk from 1 when the doubling
// search in the hash part overflows (`table.rs:336-341`, as in C Lua's `hash_search`), and
// `#` costs 1 fuel. With a 2^18 array part and 45 keys n+1, 2(n+1), 4(n+1)... each `#t`
// walks 262k entries. Expected: bounded time for 2M fuel. Actual: 20k `#t` > 10 s.
// Fix: charge fuel for the walk (or return the border found so far — any border is valid).
#[test]
fn s2_border_linear_walk() {
    let src = "local n = 1 << 18 local t = {}
               for m = 0, 44 do t[(n + 1) << m] = true end
               for i = 1, n do t[i] = true end
               assert(#t == n + 1)
               for r = 1, 20000 do local _ = #t end
               return 'done'";
    let r = must_end(src, Limits::default().fuel(3_000_000), Libs::SAFE, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S3 (critical, limit bypass): `next(t)` scans the array part from the start for the first
// non-nil value (`table.rs:388 scan_arr`) and `next` charges nothing (`base.rs:148`). A big
// array part emptied with `t[i] = nil` keeps its length, so each `next(t)` (an "is it
// empty?" idiom) walks 262k slots for ~2 fuel. Expected: bounded. Actual: 20k calls > 10 s.
// Fix: charge `next` for the slots it skips (and/or shrink the array part on rehash).
#[test]
fn s3_next_scans_empty_array() {
    let src = "local n = 1 << 18 local t = {}
               for i = 1, n do t[i] = true end
               for i = 1, n do t[i] = nil end
               for r = 1, 20000 do local _ = next(t) end
               return 'done'";
    let r = must_end(src, Limits::default().fuel(3_000_000), Libs::SAFE, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S4 (critical, limit bypass): messages and `tostring` copy a metatable's `__name` in full
// with no fuel (`vm.rs:640 obj_type_name`, `vm.rs:2750 tostring`); the error text is cut to
// `error_len` only after it was built (`vm.rs:1199`). An 8 MiB `__name` makes every failed
// `t + 1` copy ~16 MiB and every `tostring(t)` copy, hash and compare 8 MiB, for a few fuel.
// Expected: bounded. Actual: > 10 s for ~300k fuel.
// Fix: cut `__name` before formatting (C Lua uses `%s` of the name, but `luaL_tolstring`
// work is O(len) there too — here it must be charged), and charge `tostring` by length.
#[test]
fn s4_huge_name_in_errors_and_tostring() {
    let errors = "local t = setmetatable({}, {__name = string.rep('x', 8 << 20)})
                  for i = 1, 3000 do pcall(function() return t + 1 end) end return 'done'";
    let r = must_end(errors, Limits::default().fuel(500_000), Libs::SAFE, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
    let tostr = "local t = setmetatable({}, {__name = string.rep('x', 8 << 20)})
                 for i = 1, 3000 do local _ = tostring(t) end return 'done'";
    let r = must_end(tostr, Limits::default().fuel(500_000), Libs::SAFE, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S5 (critical, limit bypass): string `<`/`<=` compare bytes (`vm.rs:2642 less_than`) and
// the instruction costs 1 fuel whatever the lengths. Two 8 MiB strings equal but for the
// last byte cost a full 8 MiB compare per `<`; `table.sort` of such strings likewise
// (`stdlib/table.rs:230` charges 1 per comparison). Expected: bounded. Actual: > 10 s.
// Fix: charge comparisons by the length of the common prefix (e.g. len / 64).
#[test]
fn s5_string_compare_uncharged() {
    let src = "local a = string.rep('a', 8 << 20) local b = a:sub(1, -2) .. 'b'
               for i = 1, 100000 do local _ = a < b end return 'done'";
    let r = must_end(src, Limits::default().fuel(1_000_000), Libs::SAFE, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S6 (critical, limit bypass): arithmetic on a string converts it (`vm.rs:2414 tonumber` →
// `number.rs:309 str_to_number`) with no fuel: an 8 MiB numeral (`'0x' .. zeros`, or
// spaces then a digit) is parsed again on every `s + 1`. Same for `tonumber(s)`,
// `math.tointeger`, `string.format('%d', s)`. Expected: bounded. Actual: > 10 s.
// Fix: charge conversions by length, or refuse numerals longer than ~200 bytes as C Lua's
// `l_str2d` does for floats (L_MAXLENNUM) — note C Lua still scans ints in full.
#[test]
fn s6_string_coercion_uncharged() {
    let src = "local s = '0x' .. string.rep('0', 8 << 20)
               for i = 1, 20000 do local _ = s + 1 end return 'done'";
    let r = must_end(src, Limits::default().fuel(1_000_000), Libs::SAFE, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S7 (major/perf, limit inaccuracy): an `__index`/`__newindex` chain of tables is followed
// up to 2000 links (`vm.rs:2272`, MAX_TAG_LOOP) for one instruction's fuel, so a lookup can
// cost ~2000x a normal one: with default fuel (10M) a script runs ~2000x longer than the
// host budgeted. Expected: time per fuel within ~20x of a plain lookup loop. Actual: ~hundreds x.
// Fix: charge 1 fuel per link followed.
#[test]
fn s7_index_chain_amplification() {
    let chain = "local t = {} for i = 1, 1999 do t = setmetatable({}, {__index = t}) end
                 while true do local _ = t.x end";
    let plain = "local t = {} for i = 1, 1999 do t = setmetatable({}, {}) end
                 while true do local _ = t.x end";
    let fuel = 30_000;
    let slow = time_to_burn(chain, fuel, Libs::SAFE);
    let fast = time_to_burn(plain, fuel, Libs::SAFE);
    let ratio = slow.as_secs_f64() / fast.as_secs_f64().max(1e-6);
    assert!(ratio < 20.0, "an __index chain costs {ratio:.0}x per fuel ({slow:?} vs {fast:?})");
}

// S8 (critical, limit bypass): ephemeron convergence in the atomic step
// (`heap.rs:856-869`) rescans every weak-keyed table until nothing changes; each pass may
// mark only a few more entries, so a chain k1 -> k2 -> ... stored in a weak-keyed table in
// hash order costs O(n^2), and none of the `mark_ephemeron` work is counted (only
// `traverse`/`clear_weak` are). Expected: a collection's time proportional to the fuel it
// is charged. Actual: one `collectgarbage()` on a 40k-link chain runs > 10 s.
// Fix: count `mark_ephemeron` work, and/or converge with a worklist (as C Lua's
// `convergeephemerons` alternates directions; it is quadratic too but has no fuel model).
#[test]
fn s8_ephemeron_chain_quadratic() {
    let src = "local w = setmetatable({}, {__mode = 'k'})
               local first = {} local k = first
               for i = 1, 40000 do local nk = {} w[k] = nk k = nk end
               collectgarbage() collectgarbage()
               return 'done'";
    let r = must_end(src, Limits::default().fuel(2_000_000), Libs::STANDARD, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S9 (critical, panic): `string.pack('cN', s)` pads with `size - len` zeros
// (`pack.rs:197`) before checking `string_len` (`pack.rs:220`); with N ~ 1.8e19 the Vec
// extend panics ("capacity overflow"), and the panic goes through `exec` into the host.
// Expected: an error. Actual: a Rust panic.
// Fix: check `size` against `string_len - out.len()` before padding (C Lua errors too).
#[test]
fn s9_pack_huge_c_panics() {
    let r = bounded(20, || {
        outcome(Limits::default(), Libs::SAFE, "return string.pack('c18000000000000000000', '')")
    });
    assert!(r.is_ok(), "{}", r.err().unwrap_or_default());
}

// S10 (critical, abort): the same padding with N = 1e14 asks the allocator for 100 TB:
// allocation failure aborts the whole host process (not even a panic to catch).
// Expected: an error, the child exits normally. Actual: the process aborts.
#[test]
fn s10_pack_large_c_aborts() {
    let c = in_child("pack_c_abort");
    assert_eq!(c.code, Some(0), "child died: {} {}", c.result, c.stderr);
}

// S11 (critical in debug builds, major in release): `string.packsize` checks
// `total > 0x7fff_ffff - size` (`pack.rs:239`), which underflows when one option is larger
// than 2^31 ('c3000000000'): a debug build panics ("attempt to subtract with overflow");
// a release build wraps and returns 3000000000 where C Lua errors "format result too large".
// (C reads at most nine digits of a size, so 'c3000000000' is now 'c300000000' then an
// invalid option '0', as in C: the total past 2^31 comes from three 'c999999999'.)
#[test]
fn s11_packsize_overflow() {
    let r = bounded(20, || {
        outcome(Limits::default(), Libs::SAFE, "return string.packsize('c999999999c999999999c999999999')")
    });
    let r = r.map(|x| x.0).unwrap_or_else(|e| e);
    assert!(r.contains("too large"), "{r}");
}

// S12 (critical, memory cap bypass / abort): `table.sort` sizes a Rust Vec from `#t`
// (`stdlib/table.rs:330`), and `__len` may return up to 2^31 - 2. Fuel is charged first
// (n), but not memory: with a 1 MiB cap and 100M fuel the state allocates 320 MB of native
// memory; a host with >= 2^31 fuel gets a 32 GB allocation request, which aborts the
// process. Same shape: `table.unpack` (`:199`) when `stack_slots` is raised.
// Expected: peak memory near the baseline. Actual: +300 MB.
// Fix: charge `n * 16` bytes to the memory account first (or sort in place via the table).
#[test]
fn s12_sort_len_native_allocation() {
    let base = in_child("baseline");
    let c = in_child("sort_len");
    assert_eq!(c.code, Some(0), "{}", c.stderr);
    let extra = c.peak.saturating_sub(base.peak);
    assert!(extra < 64 << 20, "peak grew {} MB with a 1 MiB cap ({})", extra >> 20, c.result);
}

// S13 (major, memory amplification): `utf8.codepoint(s, 1, -1)` collects every code point
// into a Rust Vec (`utf8.rs:91`) before `ret` fails on the stack limit: 16 bytes per input
// byte, so an 8 MiB string under a 12 MiB cap makes a 128 MB native allocation.
// (`string.byte` caps its slice at 1M; `utf8.codepoint` allows up to i32::MAX.)
// Expected: peak near the cap. Actual: +128 MB.
// Fix: check the count against `stack_slots` before collecting, as `string.byte` does.
#[test]
fn s13_utf8_codepoint_amplification() {
    let base = in_child("baseline");
    let c = in_child("utf8_codepoint");
    assert_eq!(c.code, Some(0), "{}", c.stderr);
    let extra = c.peak.saturating_sub(base.peak);
    assert!(extra < 48 << 20, "peak grew {} MB with a 12 MiB cap ({})", extra >> 20, c.result);
}

// S14 (critical, memory cap bypass): only the main function's prototype is charged
// (`lib.rs:448 proto.mem_size()`, which is not recursive, `bytecode.rs:160`); nested
// functions' code, constants, line tables and the chunk name (`source`, the whole source
// text when `load` gets no name) are never counted. A script with `load` keeps compiling
// chunks whose bulk is in an inner function: memory_used stays tiny while the process
// grows without bound. Expected: Limit(Memory) under a 4 MiB cap. Actual: 40 chunks of
// ~0.5 MB inner code each load fine, memory_used reports a fraction.
// Fix: charge `mem_size` recursively (children, lines, source), release likewise.
#[test]
fn s14_nested_protos_uncharged() {
    let body = format!("local a = 0 {}", "a = a + 1 ".repeat(40_000));
    let src = format!(
        "local src = 'return function() {body} end'
         local keep = {{}}
         for i = 1, 40 do keep[i] = load(src)() end
         return #keep"
    );
    let mut lua = Lua::new(Limits::default().memory(4 << 20).fuel(50_000_000), Libs::ALL);
    let r = lua.exec(&src, "=t");
    assert_eq!(r, Err(Error::Limit(LimitKind::Memory)), "memory_used = {} bytes", lua.memory_used());
}

// S15 (major, memory cap bypass, transient): the parser builds the whole AST in native
// memory before anything is charged (`lib.rs:444`): a 9 MB source of `{},` items under a
// 1 MiB cap makes the process grow by hundreds of MB before the compile is refused.
// Reachable from scripts through `load` (source up to `string_len`) and from any host that
// loads untrusted text. Expected: growth near the source size. Actual: see the message.
// Fix: count AST/compiler allocations against the cap (or cap source length by memory).
#[test]
fn s15_parse_ast_outside_cap() {
    let base = in_child("baseline");
    let c = in_child("parse_ast");
    assert_eq!(c.code, Some(0), "{}", c.stderr);
    let extra = c.peak.saturating_sub(base.peak);
    assert!(extra < 64 << 20, "peak grew {} MB parsing 9 MB under a 1 MiB cap ({})", extra >> 20, c.result);
}

// ---------------------------------------------------------------- findings: host API

// S16 (major, careless host): a memory limit is not sticky. `Mem::refused` is set on a
// refused allocation (`heap.rs:47`) and reset per entry (`lib.rs:396`) but never read, so
// a host function that calls back into Lua and ignores (or rewraps as `Error::runtime`)
// the `Err(Limit(Memory))` lets the script go on as if nothing happened — the fuel limit,
// by contrast, stays exhausted. Expected: the outer call ends with Limit(Memory) (the docs:
// "scripts cannot catch it"). Actual: `ok survived`.
// Fix: fail every later allocation/instruction of the entry once `refused` is set (or
// make the entry return Limit(Memory) when `refused`).
#[test]
fn s16_memory_limit_not_sticky() {
    let mut lua = Lua::new(Limits::default().memory(1 << 20), Libs::SAFE);
    lua.register("careless", |lua, args| {
        let _ = lua.call(args[0], &[]);
        Ok(vec![])
    })
    .unwrap();
    let r = describe(
        &mut lua,
        "careless(function() local t = {} while true do t[#t + 1] = {} end end) return 'survived'",
    );
    assert_eq!(r, "limit Memory");
}

#[test]
fn fuel_limit_is_sticky_through_a_careless_host() {
    let mut lua = Lua::new(Limits::default().fuel(200_000), Libs::SAFE);
    lua.register("careless", |lua, args| {
        let _ = lua.call(args[0], &[]);
        Ok(vec![])
    })
    .unwrap();
    lua.register("rewrap", |lua, args| match lua.call(args[0], &[]) {
        Ok(v) => Ok(v),
        Err(e) => Err(Error::runtime(e.message())),
    })
    .unwrap();
    let r = describe(&mut lua, "careless(function() while true do end end) return 'survived'");
    assert_eq!(r, "limit Fuel");
    let r = describe(&mut lua, "pcall(rewrap, function() while true do end end) return 'survived'");
    assert_eq!(r, "limit Fuel");
    alive(&mut lua);
}

// S17 (major, host API state confusion): object slots are reused with a bumped generation,
// but only `check_live` (used by `call`, `set`, `get`, `table_set`…) compares generations.
// `table_get`, `table_len`, `table_pairs`, `set_metatable`, `userdata`, `userdata_mut`,
// `str_bytes`, `to_data` index slabs by `idx` alone (`heap.rs:211 Slab::get`), so a stale
// handle reads or writes whatever object took the slot. Expected: stale handles see
// nothing / are refused. Actual: they reach the new object (another script's table, another
// userdata's data).
// Fix: make `Slab::get`/`get_mut` check the generation (or check in every public entry).
#[test]
fn s17_stale_handle_reads_new_occupant() {
    let mut lua = Lua::new(Limits::default(), Libs::SAFE);
    let old = lua.exec("return {}", "=t").unwrap()[0].as_table().unwrap();
    lua.exec("return 1", "=t").unwrap();
    lua.collect_garbage().unwrap();
    lua.collect_garbage().unwrap();
    // Many new tables, so one of them takes the stale handle's slot.
    lua.exec("SECRET = {} for i = 1, 1000 do SECRET[i] = {password = 'hunter2'} end", "=t").unwrap();
    let k = lua.create_string("password").unwrap();
    let seen = lua.table_get(old, k);
    assert!(
        seen.is_nil(),
        "stale handle read {:?} from the table that reused its slot",
        lua.to_string_lossy(seen)
    );
    // Userdata: a stale handle downcasts the data of the userdata that took the slot.
    let ud_old = lua.create_userdata(1u32, None).unwrap();
    lua.exec("return 1", "=t").unwrap();
    lua.collect_garbage().unwrap();
    lua.collect_garbage().unwrap();
    let roots: Vec<_> = (0..100)
        .map(|_| {
            let u = lua.create_userdata(7u32, None).unwrap();
            lua.root(u)
        })
        .collect();
    assert_eq!(lua.userdata::<u32>(ud_old), None, "stale userdata handle read the new userdata's data");
    drop(roots);
}

// S18 (major, script-visible aliasing via a careless host): values a host function returns
// are not checked (`lib.rs:613-618`; arguments of `Lua::call` are). A host that keeps a
// script's value without rooting it and hands it back after a collection gives the script
// a reference to a freed slot; writes to it charge memory that is never released, and once
// the slot is reused the script holds an alias of an unrelated new table.
// Expected: the stale value is refused (Error::Host) or becomes nil. Actual: the script
// sees writes to a new table through the old reference.
#[test]
fn s18_host_returns_stale_value() {
    let mut lua = Lua::new(Limits::default(), Libs::STANDARD);
    let stash: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let s1 = Arc::clone(&stash);
    lua.register("keep", move |_, a| {
        *s1.lock().unwrap() = a.first().copied();
        Ok(vec![])
    })
    .unwrap();
    let s2 = Arc::clone(&stash);
    lua.register("giveback", move |_, _| Ok(vec![s2.lock().unwrap().unwrap_or_default()])).unwrap();
    lua.exec("keep({})", "=t").unwrap();
    lua.exec("collectgarbage() collectgarbage()", "=t").unwrap();
    // The host API itself knows the value is dead...
    let kept = stash.lock().unwrap().unwrap();
    assert!(matches!(lua.set(kept, Value::Int(1), Value::Int(1)), Err(Error::Host(_))));
    let r = describe(
        &mut lua,
        "local stale = giveback()
         local fresh = {} for i = 1, 5000 do fresh[i] = {} end
         for i = 1, 5000 do fresh[i].mark = i end
         if type(stale) == 'table' and stale.mark then return 'aliased ' .. stale.mark end
         if type(stale) == 'table' and rawlen(stale) == 5000 then return 'aliased with the table fresh' end
         return type(stale)",
    );
    // ...but hands it to the script when a host function returns it.
    assert!(!r.contains("aliased"), "{r}");
}

// S19 (critical in debug builds, major in release): `os.difftime(t1, t2)` computes
// `t1 - t2` unchecked (`os.rs:112`); `os.date` adds the host's UTC offset unchecked
// (`os.rs:129`, also `time` at `:84`/`:86`). `os.difftime(math.mininteger, 1)` panics a
// debug build ("attempt to subtract with overflow"); release wraps to a wrong value.
// Expected: -9.2233720368547758e18 as C Lua (it subtracts as floats). Actual: panic.
#[test]
fn s19_os_time_overflow() {
    let r = bounded(20, || outcome(Limits::default(), Libs::SAFE, "return os.difftime(math.mininteger, 1)"));
    assert!(r.is_ok(), "{}", r.err().unwrap_or_default());
    let r = bounded(20, || {
        let mut lua = Lua::new(Limits::default(), Libs::SAFE);
        lua.set_utc_offset(3600);
        describe(&mut lua, "return os.date('%Y', math.maxinteger)")
    });
    assert!(r.is_ok(), "{}", r.err().unwrap_or_default());
}

// S20 (critical, limit bypass with `load`): every error position formats the chunk name
// with `chunk_id` (`vm.rs:2848`), which scans the source text for its first line; with
// `load(src)` and no chunk name the name IS the source (`base.rs:316`), so a 8 MB one-line
// chunk makes each `error('x')` (and each traceback line) scan 8 MB for a few fuel.
// Expected: bounded. Actual: > 10 s. (The name is also kept in every prototype, uncharged:
// see S14.) Fix: compute the display name once at load and keep only that.
#[test]
fn s20_chunk_name_rescanned_per_error() {
    let src =
        "local code = 'local f = function() error(\"x\") end for i = 1, 40000 do pcall(f) end return 1 --'
                          .. string.rep('x', 8 << 20)
               return load(code)()";
    let r = must_end(src, Limits::default().fuel(2_000_000), Libs::ALL, 10);
    assert!(r.starts_with("ok") || r.starts_with("limit"), "{r}");
}

// S21 (critical in debug builds, major in release; found by `hostile_fuzz`): `select(n, ...)`
// with n = math.mininteger evaluates `-i > n` (`base.rs:203`), which overflows: a debug
// build panics ("attempt to negate with overflow"); release wraps, so the check passes and
// `select` returns nothing instead of erroring. Expected (C Lua): "bad argument #1 to
// 'select' (index out of range)". Fix: compare with `i < -n` (n >= 0) instead.
#[test]
fn s21_select_mininteger() {
    let r = bounded(20, || {
        outcome(Limits::default(), Libs::SAFE, "return pcall(select, math.mininteger, 1, 2, 3)")
    });
    let r = r.map(|x| x.0).unwrap_or_else(|e| e);
    assert!(r.contains("index out of range"), "{r}");
}

// S22 (critical, memory cap bypass / abort): `warn` (BASE, so in every sandbox) and `print`
// join all their arguments into one Rust Vec before anything is checked or charged
// (`base.rs:295-298`, `:266-276`; `print` charges fuel only after joining, `warn` never).
// 300 references to one 1 MiB string under a 4 MiB cap grow the process by ~950 MB
// (`warn`) / ~540 MB (`print`); with more/longer arguments the allocation fails and the
// process aborts. Expected: growth near the cap. Actual: see the message.
// Fix: stop at `limits.output` (or `string_len`) while joining, and charge as you go.
#[test]
fn s22_warn_print_join_unbounded() {
    let base = in_child("baseline");
    for case in ["warn_concat", "print_concat"] {
        let c = in_child(case);
        assert_eq!(c.code, Some(0), "{case}: {}", c.stderr);
        let extra = c.peak.saturating_sub(base.peak);
        assert!(extra < 64 << 20, "{case}: peak grew {} MB with a 4 MiB cap ({})", extra >> 20, c.result);
    }
}

// S23 (major, limit not enforced): `Limits::output` is "bytes `print` may write per call
// from the host", but `emit_output` measures the room against the internal buffer
// (`base.rs:284`), which stays empty when the host set a sink with `set_print`: with a
// sink every `print` may write `output` bytes again, so one call streams ~100 MB to the
// host's sink under a 1 MiB limit. (The buffer itself is also only reset by `take_output`,
// so without a sink a host that never takes output silently loses all later prints.)
// Expected: at most 1 MiB reaches the sink. Actual: ~100 MB.
// Fix: count bytes emitted per entry (reset in `enter`), whatever the destination.
#[test]
fn s23_output_limit_ignored_with_sink() {
    let mut lua = Lua::new(Limits::default().output(1 << 20).fuel(20_000_000), Libs::STANDARD);
    let got = Arc::new(Mutex::new(0usize));
    let g = Arc::clone(&got);
    lua.set_print(move |s| *g.lock().unwrap() += s.len());
    let r = describe(
        &mut lua,
        "local line = string.rep('x', 1000) for i = 1, 100000 do print(line) end return 'done'",
    );
    let total = *got.lock().unwrap();
    assert!(total <= 1 << 20, "{r}: the sink received {} MB under a 1 MiB output limit", total >> 20);
}

// S24 (major, limit not enforced): `Limits::error_len` ("the longest error message kept")
// only cuts messages the machine builds (`vm.rs:1199`); a value raised with `error(s)` is
// described in full (`lib.rs:378 describe_error`), so the host receives an 8 MiB
// `Error::Runtime` message (and `where_` prefixes copy it again in `raise`, `base.rs:72`).
// Expected: message.len() <= error_len (4096). Actual: 8 MiB.
// Related (minor): `error(s)` with `#s` within a few bytes of `string_len` becomes an
// uncatchable Limit(Memory), because the position prefix makes the message too long
// (`pcall(function() error(string.rep('x', (16 << 20) - 2)) end)` → limit Memory).
#[test]
fn s24_error_len_not_applied_to_raised_strings() {
    let mut lua = Lua::new(Limits::default(), Libs::SAFE);
    match lua.exec("error(string.rep('x', 8 << 20))", "=t") {
        Err(Error::Runtime { message, .. }) => {
            assert!(message.len() <= 4096, "message is {} bytes", message.len())
        }
        other => panic!("{other:?}"),
    }
}

// ---------------------------------------------------------------- attacks that are stopped (regressions)

/// A collector torture test: objects move between every kind of container (tables,
/// closure upvalues, suspended coroutines' locals, metatables, weak tables with a strong
/// reference elsewhere) while a small memory cap keeps the incremental collector busy;
/// every object reached is checked intact.
#[test]
fn gc_torture_objects_stay_intact() {
    let src = "
        local seed = ...
        math.randomseed(seed)
        local nextid = 0
        local function new()
          nextid = nextid + 1
          return {id = nextid, tag = 'obj' .. nextid, inner = {nextid}, s = string.rep('z', nextid % 50)}
        end
        local function ok(o)
          if o == nil then return end
          if type(o) ~= 'table' or o.tag ~= 'obj' .. o.id or o.inner[1] ~= o.id or #o.s ~= o.id % 50 then
            error('CORRUPT object ' .. tostring(o and o.id))
          end
        end
        local tab = {}
        local up = {}
        local getters, setters = {}, {}
        for i = 1, 16 do
          local v
          getters[i] = function() return v end
          setters[i] = function(x) v = x end
        end
        local cos = {}
        for i = 1, 16 do
          cos[i] = coroutine.wrap(function(x)
            local held = x
            while true do
              local nx = coroutine.yield(held)
              held = nx
            end
          end)
          cos[i](nil)
        end
        local metas = {}
        for i = 1, 16 do metas[i] = setmetatable({}, {__index = {}}) end
        local weak = setmetatable({}, {__mode = 'v'})
        local strong = {}
        for step = 1, 40000 do
          local op = math.random(1, 9)
          local i = math.random(1, 16)
          if op == 1 then tab[i] = new()
          elseif op == 2 then ok(tab[i]) setters[i](tab[i]) tab[i] = nil
          elseif op == 3 then local o = getters[i]() ok(o) tab[i] = o setters[i](nil)
          elseif op == 4 then local o = cos[i](tab[i] or new()) ok(o)
          elseif op == 5 then getmetatable(metas[i]).__index.x = new()
          elseif op == 6 then ok(metas[i].x) tab[i] = metas[i].x
          elseif op == 7 then local o = new() weak[i] = o strong[i] = o
          elseif op == 8 then ok(weak[i]) strong[i] = nil
          else
            local junk = {} for j = 1, 30 do junk[j] = {j, tostring(j) .. step} end
          end
          if step % 97 == 0 then for j = 1, 16 do ok(tab[j]) ok(getters[j]()) ok(metas[j].x) ok(strong[j]) end end
        end
        return 'intact'";
    let seeds: i64 = std::env::var("SWEEP1_TORTURE_SEEDS").ok().and_then(|s| s.parse().ok()).unwrap_or(4);
    for seed in 1..=seeds {
        let mut lua = Lua::new(Limits::default().memory(2 << 20).fuel(200_000_000), Libs::SAFE);
        let f = lua.load(src, "=torture").unwrap();
        let r = lua.call(f.value(), &[Value::Int(seed)]);
        let text = match &r {
            Ok(v) => v.first().map(|v| show(&lua, *v)).unwrap_or_default(),
            Err(e) => e.message(),
        };
        assert_eq!(text, "intact", "seed {seed}");
        assert!(lua.memory_used() <= 2 << 20);
    }
}

#[test]
fn limits_survive_every_unwinding_path() {
    let fuel = Limits::default().fuel(300_000);
    let cases = [
        // Handlers and closers that try to swallow or outlast the limit.
        "xpcall(function() while true do end end, function(e) while true do end end) return 'x'",
        "local ok = pcall(function() local c <close> = setmetatable({}, {__close = function() while true do end end}) while true do end end) return 'x'",
        "local co = coroutine.create(function() local c <close> = setmetatable({}, {__close = function() return end}) coroutine.yield() end)
         coroutine.resume(co) while true do coroutine.close(co) co = coroutine.create(function() while true do end end) coroutine.resume(co) end",
        "local t = setmetatable({}, {__tostring = function() while true do end end}) pcall(error, t) return pcall(tostring, t)",
        "local t = setmetatable({}, {__index = function() while true do end end}) for i = 1, 10 do pcall(function() return t.x end) end return 'x'",
        "local co = coroutine.wrap(function() while true do pcall(coroutine.yield) end end) while true do pcall(co) end",
        "string.gsub(string.rep('a', 100), 'a', function() while true do end end) return 'x'",
        "table.sort({3, 2, 1, 5, 4}, function(a, b) while true do end end) return 'x'",
        "local t = setmetatable({}, {__eq = function() while true do end end}) return pcall(function() return t == setmetatable({}, getmetatable(t)) end)",
        "local t = setmetatable({}, {__concat = function() while true do end end}) return pcall(function() return t .. 'x' end)",
        "local t = setmetatable({}, {__len = function() while true do end end}) return pcall(table.insert, t, 1)",
        "for k in pairs(setmetatable({}, {__pairs = function() while true do end end})) do end",
    ];
    for c in cases {
        assert_eq!(must_end(c, fuel, Libs::SAFE, 30), "limit Fuel", "{c}");
    }
    let mem = Limits::default().memory(1 << 20).fuel(50_000_000);
    let cases = [
        "xpcall(function() local s = 'x' while true do s = s .. s end end, function(e) return e end) return 'x'",
        "local ok = pcall(function() local c <close> = setmetatable({}, {__close = function() end}) local t = {} while true do t[#t + 1] = {} end end) return 'x'",
        "local co = coroutine.wrap(function() local t = {} while true do t[#t + 1] = {} end end) pcall(co) return 'x'",
        "local t = setmetatable({}, {__index = function(t, k) local x = {} for i = 1, 1e6 do x[i] = i end return x end}) pcall(function() return t.x end) return 'x'",
        "local ok = pcall(string.rep, 'x', 1 << 20) local t = {} for i = 1, 100 do t[i] = string.rep('y', 1 << 16) .. i end return 'x'",
    ];
    for c in cases {
        assert_eq!(must_end(c, mem, Libs::SAFE, 30), "limit Memory", "{c}");
    }
}

#[test]
fn capabilities_cannot_be_reached() {
    let probes = "local found = {}
        local function probe(name, v) if v ~= nil then found[#found + 1] = name end end
        probe('load', load) probe('loadstring', loadstring) probe('dofile', dofile) probe('require', require)
        probe('io', io) probe('debug', debug) probe('package', package) probe('os.execute', os and os.execute)
        probe('os.getenv', os and os.getenv) probe('os.exit', os and os.exit) probe('os.remove', os and os.remove)
        probe('collectgarbage', collectgarbage) probe('print', print) probe('setfenv', setfenv)
        probe('string.meta.load', getmetatable('').__index.load)
        probe('_ENV.load', _ENV.load) probe('_G.load', _G.load)
        local ok, d = pcall(string.dump, function() end) probe('dump', ok and d or nil)
        return table.concat(found, ',')";
    let mut lua = Lua::new(Limits::default(), Libs::SAFE);
    assert_eq!(describe(&mut lua, probes), "ok ");
    // Without STRING, strings have no methods and no metatable to abuse.
    let mut bare = Lua::new(Limits::default(), Libs::BASE);
    assert_eq!(
        describe(&mut bare, "return getmetatable('') == nil, (pcall(function() return ('x'):upper() end))"),
        "ok Bool(true),Bool(false)"
    );
    // `load` never takes binary chunks, in any mode, from a string or a reader.
    let mut all = Lua::new(Limits::default(), Libs::ALL);
    let r = describe(
        &mut all,
        "local n = 0 local parts = {'\\27Lua', 'T\\0'}
         local f1 = load('\\27LuaT\\0', 'x', 'bt')
         local f2 = load(function() n = n + 1 return parts[n] end, 'x', 'b')
         n = 0 local f3 = load(function() n = n + 1 return parts[n] end)
         return f1 == nil, f2 == nil, f3 == nil",
    );
    assert_eq!(r, "ok Bool(true),Bool(true),Bool(true)");
}

#[test]
fn host_reentrancy_and_gc_mid_call() {
    let mut lua = Lua::new(Limits::default().fuel(5_000_000), Libs::STANDARD);
    lua.register("host.gc", |lua, _| {
        lua.collect_garbage()?;
        Ok(vec![])
    })
    .unwrap();
    lua.register("host.call", |lua, args| lua.call(args[0], args.get(1..).unwrap_or_default())).unwrap();
    lua.register("host.mk", |lua, _| {
        let t = lua.create_table()?;
        let s = lua.create_string("made by host")?;
        lua.table_set(t, Value::Int(1), s)?;
        lua.collect_garbage()?;
        Ok(vec![Value::Table(t)])
    })
    .unwrap();
    let src = "local acc = {}
        for i = 1, 200 do
          local t = host.mk()
          host.gc()
          acc[i] = t
          local r = host.call(function(a, b) host.gc() return a .. b, {i} end, 'x', i)
          assert(r == 'x' .. i)
        end
        local co = coroutine.wrap(function() for i = 1, 50 do host.gc() coroutine.yield(host.call(function() return i end)) end end)
        local s = 0 for i = 1, 50 do s = s + co() end
        local ok, e = pcall(host.call, function() coroutine.yield() end)
        for i = 1, 200 do assert(acc[i][1] == 'made by host') end
        return s, ok";
    assert_eq!(describe(&mut lua, src), "ok Int(1275),Bool(false)");
    alive(&mut lua);
}

// ---------------------------------------------------------------- the hostile program generator

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
    fn chance(&mut self, pct: usize) -> bool {
        self.below(100) < pct
    }
}

const INTS: &[&str] = &[
    "0",
    "1",
    "-1",
    "2",
    "3",
    "7",
    "8",
    "100",
    "255",
    "256",
    "1000",
    "-1000",
    "65536",
    "1e6",
    "2^31",
    "-2^31",
    "2^53",
    "math.maxinteger",
    "math.mininteger",
    "math.maxinteger - 1",
    "math.mininteger + 1",
    "1 << 40",
    "1 << 62",
    "-(1 << 62)",
    "0.5",
    "-0.0",
    "1/0",
    "-1/0",
    "0/0",
    "'10'",
    "'0x10'",
    "N",
];

const STRS: &[&str] = &[
    "''",
    "'x'",
    "'abc'",
    "'%d'",
    "'%s'",
    "'\\0'",
    "'\\255\\254'",
    "'é'",
    "string.rep('a', 1000)",
    "S",
    "string.rep('ab', N % 4096)",
    "'(.-)'",
    "'%a+'",
    "'[^%z]'",
    "'%b()'",
    "'%f[%w]'",
    "'i16'",
    "'c10'",
    "'<i4>!8d'",
    "'z'",
    "'s1'",
    "'xXi8'",
    "'j'",
];

fn int(r: &mut Rng) -> String {
    r.pick(INTS).to_owned()
}

fn st(r: &mut Rng) -> String {
    r.pick(STRS).to_owned()
}

/// A random value expression of any type.
fn val(r: &mut Rng) -> String {
    match r.below(9) {
        0 | 1 => int(r),
        2 | 3 => st(r),
        4 => "T".into(),
        5 => "{}".into(),
        6 => "F".into(),
        7 => "M".into(),
        _ => r.pick(&["nil", "true", "false", "co", "setmetatable({}, MT)"]).into(),
    }
}

/// One hostile statement. `known` adds the attacks of the findings above.
fn stmt(r: &mut Rng, depth: usize, known: bool) -> String {
    if depth > 4 {
        return format!("local _ = {}", val(r));
    }
    let n = r.below(if known { 64 } else { 56 });
    let (a, b, c) = (int(r), int(r), int(r));
    match n {
        0 => format!("string.rep({}, {a}, {})", st(r), st(r)),
        1 => format!("string.sub({}, {a}, {b})", st(r)),
        2 => format!("string.byte({}, {a}, {b})", st(r)),
        3 => format!("string.format({}, {})", r.pick(&["'%d'", "'%5.2f'", "'%q'", "'%s'", "'%x'", "'%99.99f'", "'%-99d'", "'%a'", "'%c'", "'%i'", "'%g'", "'%10.3s'"]), val(r)),
        4 => format!("string.pack({}, {a}, {b}, {c})", r.pick(&["'i16'", "'j'", "'b'", "'<I3'", "'!4 i3 x d'", "'s1'", "'z'", "'c3'", "'Xi4 b'", "'i0'", "'i17'", "'!17'", "'n'", "'>h'"])),
        5 => format!("string.unpack({}, {}, {a})", st(r), st(r)),
        6 => format!("string.packsize({})", r.pick(&["'i16'", "'!8 d i3'", "'c10'", "'c2147483647'", "'Xi4'", "'s4'"])),
        7 => format!("string.find({}, {}, {a}, {})", st(r), st(r), r.pick(&["true", "false", "nil"])),
        8 => format!("string.gsub({}, {}, {}, {a})", st(r), st(r), r.pick(&["'%0%0'", "F", "T", "'%1'", "'x'", "{}", "function() error('g') end", "function() return coroutine.yield() end"])),
        9 => format!("for _ in string.gmatch({}, {}) do {} end", st(r), st(r), stmt(r, depth + 1, known)),
        10 => format!("string.char({a}, {b})"),
        11 => format!("utf8.char({a}, {b})"),
        12 => format!("utf8.codepoint({}, {a}, {b})", st(r)),
        13 => format!("utf8.offset({}, {a}, {b})", st(r)),
        14 => format!("utf8.len({}, {a}, {b}, {})", st(r), r.pick(&["true", "false"])),
        15 => format!("table.move(T, {a}, {b}, {c})"),
        16 => format!("table.insert(T, {a}, {})", val(r)),
        17 => format!("table.remove(T, {a})"),
        18 => format!("table.concat(T, {}, {a}, {b})", st(r)),
        19 => format!("table.unpack(T, {a}, {b})"),
        20 => format!("table.sort(T, {})", r.pick(&["nil", "function(x, y) return math.random() < 0.5 end", "function(x, y) return true end", "function(x, y) error('s') end", "function(x, y) T[#T + 1] = x return false end", "function() coroutine.yield() end"])),
        // `select(math.mininteger, ...)` is finding S21.
        21 => format!("select({}, 1, 2, 3)", if known || a != "math.mininteger" { a.as_str() } else { "-4" }),
        22 => format!("math.random({a}, {b})"),
        23 => format!("math.tointeger({a}) math.fmod({a}, {b}) math.ult({a}, {b}) math.abs({a})"),
        24 => format!("local _ = ({a}) // ({b}) local _ = ({a}) % ({b}) local _ = ({a}) << ({b}) local _ = ({a}) ^ ({b})"),
        25 => format!("for i = {a}, {b}, {c} do N = N + 1 if N > 3000 then break end end"),
        26 => format!("T[{a}] = {} T[{b}] = nil", val(r)),
        27 => format!("setmetatable(T, {{__index = {}, __newindex = {}, __len = function() return {a} end}})", r.pick(&["T", "F", "MT", "nil", "function(t, k) return k end"]), r.pick(&["T", "F", "nil", "function(t, k, v) rawset(t, k, v) end"])),
        28 => "MT.__index = MT MT.__newindex = MT MT.__call = MT".into(),
        29 => format!("local x = setmetatable({{}}, MT) local _ = x.y, x + 1, x .. 'a', #x, x < x, x == {{}}, x({a})"),
        30 => format!("MT.__tostring = function() {} return {} end tostring(setmetatable({{}}, MT))", stmt(r, depth + 1, known), val(r)),
        31 => format!("MT.__close = function() {} end do local x <close> = setmetatable({{}}, MT) {} end", stmt(r, depth + 1, known), stmt(r, depth + 1, known)),
        32 => format!("co = coroutine.create(function(...) {} coroutine.yield({}) {} end) coroutine.resume(co, {a})", stmt(r, depth + 1, known), val(r), stmt(r, depth + 1, known)),
        33 => "coroutine.resume(co) coroutine.close(co) coroutine.status(co)".into(),
        34 => format!("coroutine.wrap(function() {} end)()", stmt(r, depth + 1, known)),
        35 => format!("error({}, {a})", val(r)),
        36 => format!("xpcall(function() {} end, function(e) {} return e end)", stmt(r, depth + 1, known), stmt(r, depth + 1, known)),
        37 => "local function f(n) if n > 0 then return f(n - 1) + 1 end return 0 end f(N % 5000)".into(),
        38 => "local function f() return f() end f()".into(),
        39 => "local function f() return pcall(f) end f()".into(),
        40 => format!("collectgarbage({})", r.pick(&["'collect'", "'step'", "'count'", "'incremental'", "'stop'"])),
        41 => "local w = setmetatable({}, {__mode = 'k'}) for i = 1, 200 do w[{}] = {} end W = w".into(),
        42 => "local w = setmetatable({}, {__mode = 'v'}) for i = 1, 200 do w[i] = {} end collectgarbage('step') W = w".into(),
        43 => format!("load({}, {}, {})", r.pick(&["'return 1'", "'x = '", "'\\27Lua'", "S", "function() return nil end", "string.rep('(', 300)", "'return ...'"]), r.pick(&["nil", "'=c'", "S"]), r.pick(&["nil", "'t'", "'b'", "'bt'"])),
        44 => "S = S .. S".into(),
        45 => "while true do end".into(),
        46 => "local t = {} while true do t[#t + 1] = {} end".into(),
        47 => format!("local t = {{}} for i = 1, N % 20000 do t[i] = {} end T = t", val(r)),
        48 => format!("next(T, {})", val(r)),
        49 => format!("for k, v in pairs(T) do T[k] = nil {} end", if r.chance(30) { "T[#T + 1] = 1" } else { "" }),
        50 => format!("rawset(T, {}, {}) rawget(T, {a}) rawlen(T) rawequal(T, {b})", val(r), val(r)),
        51 => format!("tostring({}) tonumber({}, {a})", val(r), st(r)),
        52 => format!("host.call(function() {} end) host.gc()", stmt(r, depth + 1, known)),
        53 => format!("local t = setmetatable({{}}, {{__index = function(t, k) host.gc() return k end}}) local _ = t[{a}] .. ''"),
        54 => format!("local c = coroutine.wrap(function() for i = 1, 3 do host.gc() coroutine.yield(host.call(F, i)) end end) c() c() {}", stmt(r, depth + 1, known)),
        55 => "goto skip local x = 1 ::skip:: do local y <const> = 2 end".into(),
        // Known findings (only with SWEEP1_FUZZ_KNOWN=1).
        56 => "string.pack('c18000000000000000000', '')".into(),
        57 => "string.packsize('c3000000000')".into(),
        58 => "os.difftime(math.mininteger, 1)".into(),
        59 => "local s = string.rep(' ', N % (1 << 20)) .. '1' for i = 1, 100 do local _ = s + 1 end".into(),
        60 => "local t = setmetatable({}, {__name = string.rep('n', 1 << 20)}) for i = 1, 100 do pcall(function() return t + 1 end) end".into(),
        61 => "local n = 1 << 14 local t = {} for m = 0, 49 do t[(n + 1) << m] = 1 end for i = 1, n do t[i] = 1 end for i = 1, 300 do local _ = #t end".into(),
        62 => "local t = {} for i = 1, 1 << 14 do t[i] = 1 end for i = 1, 1 << 14 do t[i] = nil end for i = 1, 300 do next(t) end".into(),
        _ => "table.sort(setmetatable({}, {__len = function() return 1 << 20 end}))".into(),
    }
}

fn block(r: &mut Rng, depth: usize, known: bool) -> String {
    let mut out = String::new();
    for _ in 0..1 + r.below(4) {
        let s = if depth < 3 && r.chance(15) { block(r, depth + 1, known) } else { stmt(r, depth, known) };
        // Most statements are protected, so the program goes on after an error.
        if r.chance(80) {
            out.push_str(&format!("pcall(function() {s} end)\n"));
        } else {
            out.push_str(&s);
            out.push('\n');
        }
    }
    out
}

const PRELUDE: &str = "local N = 1000 local S = 'seed' local T = {1, 2, 3, 'x', {}}
local F = function(...) return ... end local MT = {} local M = setmetatable({}, MT)
local co = coroutine.create(function() end) local W
local CHECK, CK = {}, setmetatable({}, {__mode = 'k'}) local KEEP = {}
for i = 1, 300 do CHECK[i] = {i, 'v' .. i} CHECK['k' .. i] = i KEEP[i] = {} CK[KEEP[i]] = i end
local up = 0 local function bump() up = up + 1 return up end
";

fn program(r: &mut Rng, known: bool) -> String {
    let mut p = String::from(PRELUDE);
    for _ in 0..1 + r.below(5) {
        p.push_str(&block(r, 0, known));
    }
    // Self-check: what the program never touched must be intact (the collector kept it).
    p.push_str(
        "for i = 1, 300 do
           local e = CHECK[i]
           if type(e) ~= 'table' or e[1] ~= i or e[2] ~= 'v' .. i or CHECK['k' .. i] ~= i or CK[KEEP[i]] ~= i then
             error('CORRUPT at ' .. i)
           end
         end
         if bump() ~= 1 then error('CORRUPT upvalue') end\n",
    );
    p.push_str("return 'end'\n");
    p
}

/// Checks the invariants on one program: no panic, bounded time for the fuel, memory
/// under the cap, the limits uncatchable, the state usable afterwards.
fn check_program(src: &str, seed: u64) -> Result<(), String> {
    let fuel = 300_000;
    let cap = 4usize << 20;
    let src = src.to_owned();
    let run = bounded(30, move || {
        super::common::small_stack(move || {
            let mut lua = Lua::new(Limits::default().fuel(fuel).memory(cap).string_len(1 << 20), Libs::ALL);
            lua.register("host.call", |lua, args| lua.call(args[0], args.get(1..).unwrap_or_default()))
                .unwrap();
            lua.register("host.gc", |lua, _| {
                lua.collect_garbage()?;
                Ok(vec![])
            })
            .unwrap();
            let mut problems = Vec::new();
            for round in 0..2 {
                let r = lua.exec(&src, "=fuzz");
                if lua.memory_used() > cap {
                    problems.push(format!("round {round}: memory_used {} > cap {cap}", lua.memory_used()));
                }
                if let Err(Error::Runtime { message, .. }) = &r
                    && (message.contains("internal") || message.contains("CORRUPT"))
                {
                    problems.push(format!("round {round}: internal error: {message}"));
                }
                match lua.exec("return 1 + 1", "=alive") {
                    Ok(v) if v == vec![Value::Int(2)] => {}
                    other => problems.push(format!("round {round}: state not usable afterwards: {other:?}")),
                }
            }
            problems
        })
    });
    match run {
        Ok((problems, took)) if problems.is_empty() => {
            // 300k fuel twice is milliseconds of normal work; allow generous debug slack.
            if took > Duration::from_secs(8) {
                return Err(format!("seed {seed}: took {took:?} for 2 x {fuel} fuel"));
            }
            Ok(())
        }
        Ok((problems, _)) => Err(format!("seed {seed}: {}", problems.join("; "))),
        Err(e) => Err(format!("seed {seed}: {e}")),
    }
}

#[test]
fn hostile_fuzz() {
    let iters: u64 = std::env::var("SWEEP1_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(150);
    let seed0: u64 = std::env::var("SWEEP1_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let known = std::env::var("SWEEP1_FUZZ_KNOWN").is_ok_and(|v| v == "1");
    let only: Option<u64> = std::env::var("SWEEP1_FUZZ_ONLY").ok().and_then(|s| s.parse().ok());
    let mut failures = Vec::new();
    let seeds: Vec<u64> = match only {
        Some(s) => vec![s],
        None => (0..iters).map(|i| seed0.wrapping_mul(1_000_003).wrapping_add(i)).collect(),
    };
    for seed in seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let src = program(&mut r, known);
        if let Err(e) = check_program(&src, seed) {
            eprintln!("---- FAILURE {e}\n{src}");
            failures.push(e);
            if e_is_hang(failures.last()) || failures.len() >= 5 {
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{} failing programs: {failures:#?}", failures.len());
}

fn e_is_hang(e: Option<&String>) -> bool {
    e.is_some_and(|e| e.contains("HANG"))
}
