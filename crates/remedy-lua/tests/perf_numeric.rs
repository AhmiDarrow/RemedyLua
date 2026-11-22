//! The fast loop's numeric and array opcodes.
//!
//! Two kinds of test. The semantic ones run against C Lua with the fast loop on and off
//! (same results, fuel and memory: `common::remedy`). The instruction-count ones bound what
//! one operation costs in the fast loop: the test binary runs itself under callgrind
//! (`valgrind --tool=callgrind`, which counts instructions exactly and repeatably) on the
//! same loop at two iteration counts, and the difference over the extra iterations is the
//! cost of one. They need valgrind on `PATH` and fail loudly without it; they are ignored by
//! default (seconds each under valgrind), and count in release builds only (the build the
//! benchmarks use): `cargo test --release -p remedy-lua --test all perf_numeric:: --
//! --ignored`.

use std::process::Command;

use crate::common::{oracle, remedy};
use remedy_lua::{Libs, Limits, Lua};

fn same_as_c(program: &str) {
    let ours = remedy(program, 50_000_000);
    let theirs = oracle(program, 50_000_000);
    assert_eq!(ours, theirs, "differs from C Lua:\n{program}");
}

/// The variable a child run reads its workload from.
const CHILD: &str = "REMEDYLUA_PERF_NUMERIC_CHILD";

/// The workload of a child run: `setup`, then `body` `n` times in a numeric `for` (from
/// `1` or, for a float loop, `1.0`), in one unmetered state (fast loop on, a fixed seed).
fn run_workload(spec: &str) {
    let mut parts = spec.splitn(4, '\u{1}');
    let (start, n, setup, body) = (parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap(), parts.next().unwrap());
    let src = format!("{setup}\nfor _i = {start}, {n} do {body} end");
    let mut lua = Lua::new(Limits::unmetered().hash_seed(0x5eed), Libs::ALL);
    lua.set_fast_loop(true);
    lua.exec(&src, "=perf").unwrap();
}

/// Not a test of its own: the entry point of the child runs (does nothing without them).
#[test]
#[ignore = "entry point of perf_numeric's callgrind child runs"]
fn child() {
    if let Ok(spec) = std::env::var(CHILD) {
        run_workload(&spec);
    }
}

/// Instructions the whole test process executes for the workload at `n` iterations.
fn instructions(start: &str, n: u64, setup: &str, body: &str) -> u64 {
    let exe = std::env::current_exe().unwrap();
    let out = Command::new("valgrind")
        .args(["--tool=callgrind", "--callgrind-out-file=/dev/null"])
        .arg(&exe)
        .args(["perf_numeric::child", "--exact", "--ignored", "--test-threads=1", "-q"])
        .env(CHILD, format!("{start}\u{1}{n}\u{1}{setup}\u{1}{body}"))
        .output()
        .unwrap_or_else(|e| panic!("valgrind must be installed for perf_numeric's instruction counts: {e}"));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "child run failed:\n{err}");
    let refs = err.lines().find(|l| l.contains("I   refs:")).unwrap_or_else(|| panic!("no callgrind total:\n{err}"));
    refs.rsplit(':').next().unwrap().trim().replace(',', "").parse().unwrap()
}

/// Instructions one iteration of `body` (with the loop step) costs, from two runs.
fn per_iteration(start: &str, setup: &str, body: &str) -> f64 {
    // Counted in the build the benchmarks use: the tests' profile (opt-level 2, debug
    // assertions) allocates the loop's registers differently, and its counts moved by a
    // dozen instructions with changes that left release's alone.
    assert!(!cfg!(debug_assertions), "perf_numeric's instruction counts are for release builds: run with --release");
    let (lo, hi) = (20_000, 120_000);
    let a = instructions(start, lo, setup, body);
    let b = instructions(start, hi, setup, body);
    (b.saturating_sub(a)) as f64 / (hi - lo) as f64
}

fn assert_at_most(what: &str, start: &str, setup: &str, body: &str, bound: f64) {
    let got = per_iteration(start, setup, body);
    println!("{what}: {got:.1} instructions per iteration (bound {bound})");
    assert!(got <= bound, "{what}: {got:.1} instructions per iteration, over the bound of {bound}");
}

/// The numeric `for` step: one bounds check for its four registers, the index and the
/// count stored as payloads alone. Before: 66 instructions a step in a release build (C
/// Lua 5.4.7: 32), 61 in the tests' profile; after: 41 in both.
#[test]
#[ignore = "runs valgrind (perf_numeric instruction counts)"]
fn forloop_step_instructions() {
    assert_at_most("integer for step", "1", "", "", 50.0);
    assert_at_most("float for step", "1.0", "", "", 50.0);
}

/// The numeric `for` loops the fast loop runs in place: integer and float, both
/// directions, steps that overflow or land exactly on the limit, the control variable
/// assigned in the body (a copy: the loop goes on from its own index), nested loops, a
/// loop whose registers sit as high as C Lua's 200 locals let them, and
/// loops broken out of with `break`, `goto` and `return`.
#[test]
fn forloop_variants_match_c() {
    same_as_c(
        "local out = {}
local function add(x) out[#out + 1] = tostring(x) end
for i = 1, 5 do add(i) end
for i = 5, 1, -1 do add(i) end
for i = 1, 10, 3 do add(i) end
for i = 10, 1, -4 do add(i) end
for i = math.maxinteger - 2, math.maxinteger do add(i) end
for i = math.mininteger + 2, math.mininteger, -1 do add(i) end
for i = 1, math.maxinteger, math.maxinteger // 2 do add(i) end
for i = 0.5, 2.5, 0.5 do add(i) end
for i = 2.5, 0.5, -0.5 do add(i) end
for i = 1, 3.5 do add(i) end
for i = 1.0, 3 do add(i) end
for i = 0.1, 0.35, 0.1 do add(i) end
for i = 1, 0 do add('never') end
for i = 1.0, 0.0 do add('never') end
for i = 1, 6 do local j = i; i = i * 100; add(j .. ':' .. i) end
for i = 1, 3 do for j = i, 3 do add(i * 10 + j) end end
for i = 1, 100 do if i > 3 then break end add(i) end
for i = 1, 5 do if i % 2 == 0 then goto skip end add(i) ::skip:: end
local function first_over(n) for i = 1, 100 do if i * i > n then return i end end end
add(first_over(50))
local s = 0
for i = 1, 100000 do s = s + i end
add(s)
local f = 0.0
for i = 1, 1000, 0.25 do f = f + i end
add(f)
return table.concat(out, ' ')",
    );
    // A loop over the highest registers 200 locals reach: 195, then the loop's four.
    let locals: Vec<String> = (0..195).map(|i| format!("l{i}")).collect();
    let program = format!(
        "local {} = {}\nlocal s = 0\nfor i = 1, 10 do s = s + i end\nfor i = 1.5, 3.5 do s = s + i end\nreturn s, l0, l194",
        locals.join(", "),
        (0..195).map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
    );
    same_as_c(&program);
}

/// Every fuel limit through a run of nested loops stops at the same instruction on both
/// paths: the fast loop's step charges as the full path's does.
#[test]
fn forloop_fuel_matches_full_path_at_every_limit() {
    let program = "local s = 0 for i = 1, 7 do for j = 1.0, 3.0, 0.5 do s = s + j end end return s";
    for fuel in (1..400).step_by(3) {
        let _ = remedy(program, fuel);
    }
}

/// Constant-key reads: the key read once; a string key at its hinted node, or an integer
/// key inside the array part, with a live value, returned before the general read. Field
/// writes: a hit at the hinted node needs no interned-key lookup first (the node's key is
/// interned). Before (release build): `z = t.a` 85, `z = t[3]` 82, `t.a = z` 106
/// instructions each; after: 78, 67, 89 (C Lua 5.4.7: 61, 39, 64). In the tests' profile,
/// before: 86, 89, 97; after: 78, 68, 89.
#[test]
#[ignore = "runs valgrind (perf_numeric instruction counts)"]
fn constant_key_instructions() {
    let base = per_iteration("1", "local z", "");
    let t = "local t = {1, 2, 3, 4, a = 1, b = 2} local z = 0";
    for (what, body, bound) in [
        ("z = t.a", "z = t.a z = t.a z = t.a z = t.a z = t.a z = t.a z = t.a z = t.a z = t.a z = t.a", 82.0),
        ("z = t[3]", "z = t[3] z = t[3] z = t[3] z = t[3] z = t[3] z = t[3] z = t[3] z = t[3] z = t[3] z = t[3]", 76.0),
        ("t.a = z", "t.a = z t.a = z t.a = z t.a = z t.a = z t.a = z t.a = z t.a = z t.a = z t.a = z", 93.0),
    ] {
        let got = (per_iteration("1", t, body) - base) / 10.0;
        println!("{what}: {got:.1} instructions (bound {bound})");
        assert!(got <= bound, "{what}: {got:.1} instructions, over the bound of {bound}");
    }
}

/// Field reads and writes with constant keys in every case the fast loop's preludes touch
/// or pass by: a hinted node that holds the key (live, then deleted: `nil`, with and
/// without `__index`), a hint that moved (the table rebuilt, another table at the same
/// instruction), integer keys inside the array part, past it, at a `nil` slot (with
/// `__index` and without), float keys that are integers, long string constants (not
/// interned until used as a key), and writes to deleted fields under `__newindex`.
#[test]
fn constant_key_access_matches_c() {
    let long = "k".repeat(60);
    same_as_c(&format!(
        "local out = {{}}
local function add(x) out[#out + 1] = tostring(x) end
local t = {{1, 2, 3, a = 'A', b = 'B', ['{long}'] = 'L'}}
for i = 1, 3 do add(t.a) add(t[2]) add(t['{long}']) end
t.a = nil
for i = 1, 2 do add(t.a) end
setmetatable(t, {{__index = function(_, k) return 'idx:' .. tostring(k) end}})
for i = 1, 2 do add(t.a) add(t[3]) add(t[4]) add(t[2.0]) end
t[3] = nil
add(t[3])
local u = setmetatable({{}}, {{__newindex = function(tt, k, v) rawset(tt, k, 'nw:' .. tostring(v)) end}})
for i = 1, 3 do u.a = i add(u.a) u.a = nil end
local shapes = {{ {{a = 1}}, {{b = 2, a = 3}}, {{a = 4, c = 5, d = 6}}, setmetatable({{}}, {{__index = {{a = 7}}}}) }}
for r = 1, 3 do for _, s in ipairs(shapes) do add(s.a) s.a = (s.a or 0) + 1 end end
local v = {{}}
for i = 1, 40 do v['f' .. i] = i end
for i = 1, 3 do v.f7 = v.f7 + 1 add(v.f7) end
v['{long}'] = 1
for i = 1, 3 do v['{long}'] = v['{long}'] + 1 end
add(v['{long}'])
local w = {{10, 20, 30}}
for i = 1, 3 do w[1] = w[1] + w[3] w[5] = i add(w[1]) add(w[5]) end
return table.concat(out, ' ')"
    ));
}

/// Fuel at every limit through constant-key reads and writes, hits and misses, is the
/// full path's.
#[test]
fn constant_key_fuel_matches_full_path_at_every_limit() {
    let program = "local t = {1, 2, a = 1} local m = setmetatable({}, {__index = t}) local s = 0
for i = 1, 6 do s = s + t.a + t[2] + (m.a or 0) + (m[1] or 0) t.a = i t.b = i t.b = nil t[2] = i end
return s";
    for fuel in (1..500).step_by(3) {
        let _ = remedy(program, fuel);
    }
}

/// `<` and `<=` between two registers holding two integers or two floats: compared in
/// the handler, after one test of both operands' constant bits. Before (release build):
/// `if x < y then` 77 and `if x <= y then` (floats) 79 instructions; after: 65 and 71 (C
/// Lua 5.4.7: 57 and 65). In the tests' profile, before: 75 and 77; after: 67 and 73.
#[test]
#[ignore = "runs valgrind (perf_numeric instruction counts)"]
fn register_compare_instructions() {
    let base = per_iteration("1", "local z", "");
    for (what, setup, body, bound) in [
        ("if x < y", "local x, y, z = 1, 2, 0", "if x < y then z = x end", 71.0),
        ("if x <= y (floats)", "local x, y, z = 1.5, 2.5, 0", "if x <= y then z = x end", 75.0),
    ] {
        let body = [body; 10].join(" ");
        let got = (per_iteration("1", setup, &body) - base) / 10.0;
        println!("{what}: {got:.1} instructions (bound {bound})");
        assert!(got <= bound, "{what}: {got:.1} instructions, over the bound of {bound}");
    }
}

/// `<`, `<=`, `>` and `>=` between registers and against constants, for every pair of
/// number subtypes (mixed integer and float compared exactly, at 2^53 and past it, NaN,
/// infinities, -0.0), strings, and values with and without `__lt`/`__le`, with the branch
/// taken and not.
#[test]
fn register_compares_match_c() {
    same_as_c(
        "local vals = {1, 2, -3, 0, 1.0, 2.5, -0.0, 0.0, 2^53, 2^53 + 1, math.tointeger(2^53) + 1,
  math.maxinteger, math.mininteger, math.maxinteger + 0.0, math.huge, -math.huge, 0/0}
local out = {}
for i = 1, #vals do
  for j = 1, #vals do
    local a, b = vals[i], vals[j]
    local r = 0
    if a < b then r = r + 1 end
    if a <= b then r = r + 2 end
    if a > b then r = r + 4 end
    if a >= b then r = r + 8 end
    if not (a < b) then r = r + 16 end
    if a < 2.5 then r = r + 32 end
    if a <= 1e300 then r = r + 64 end
    out[#out + 1] = r
  end
end
local s1, s2 = 'abc', 'abd'
out[#out + 1] = tostring(s1 < s2) .. tostring(s1 <= s1) .. tostring(s2 < s1)
local mt = {__lt = function(x, y) return x.v < y.v end, __le = function(x, y) return x.v <= y.v end}
local p, q = setmetatable({v = 1}, mt), setmetatable({v = 2}, mt)
for i = 1, 3 do out[#out + 1] = tostring(p < q) .. tostring(q <= p) .. tostring(p >= q) end
out[#out + 1] = select(2, pcall(function() local a, b = {}, 1 return a < b end))
out[#out + 1] = select(2, pcall(function() local a, b = 'x', 1 return a <= b end))
return table.concat(out, ' ')",
    );
}

/// Fuel at every limit through compares, taken and not, is the full path's.
#[test]
fn register_compare_fuel_matches_full_path_at_every_limit() {
    let program = "local n, x, f = 0, 3, 2.5 for i = 1, 8 do if i < x then n = n + 1 end if f <= i then n = n + 2 end if i > f then n = n + 4 end end return n";
    for fuel in (1..300).step_by(2) {
        let _ = remedy(program, fuel);
    }
}

/// What the fast loop runs without leaving it (no valgrind: runs in every suite). The
/// full path takes a few instructions around the chunk's set-up (3 to 12 here); a numeric
/// operation falling back to it would add one per iteration (20,000).
#[test]
fn numeric_ops_stay_in_the_fast_loop() {
    let setup = "local t = {1, 2, 3, 4, a = 1, b = 2.5} local x, y, f, g, z = 3, 4, 1.5, 2.5, 0";
    for body in [
        "",
        "z = x + y z = f * g z = x // y z = f - 1 z = x % 3",
        "z = t.a t.a = z z = t[3] t[3] = z z = t[x] t[x] = z",
        "if x < y then z = 1 end if f <= g then z = 2 end if x > 2 then z = 3 end if x == 3 then z = 4 end",
    ] {
        for start in ["1", "1.0"] {
            let mut lua = Lua::new(Limits::default().hash_seed(0x5eed), Libs::ALL);
            lua.set_fast_loop(true);
            lua.exec(format!("{setup}\nfor _i = {start}, 20000 do {body} end"), "=guard").unwrap();
            assert!(lua.full_steps() <= 16, "{body:?} from {start}: {} instructions on the full path", lua.full_steps());
        }
    }
}

/// Field reads into a register write the register word by word: returned through the
/// general read's value they went through a stack slot written as two 8-byte stores and
/// read as one 16-byte load, which cannot be forwarded (a stall on every read; nbody ran
/// at 1.5 times C Lua's time on 1.1 times its instructions). Timed, so ignored by
/// default: a loop updating two fields against the same arithmetic on registers, each
/// the best of 7 alternating runs, per unit of fuel. Before: 1.75 (release) and 2.22 (the
/// tests' profile) times the control; after: 1.26 to 1.42 (the control's own speed moves
/// with unrelated changes to the loop: one made it faster), so the bound is 1.55.
#[test]
#[ignore = "timed (perf_numeric)"]
fn field_reads_do_not_stall() {
    let run = |src: &str| {
        let mut lua = Lua::new(Limits::unmetered().hash_seed(0x5eed), Libs::ALL);
        let f = lua.load(src, "=t").unwrap();
        let (start, fuel) = (remedy_lua_oracle::thread_cpu(), lua.fuel_left());
        lua.call(f.value(), &[]).unwrap();
        remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64 / (fuel - lua.fuel_left()) as f64
    };
    let fields = "local t = {x = 1.0, y = 2.0} for i = 1, 2000000 do t.x = t.x + t.y * 0.5 t.y = t.y - t.x * 0.25 end";
    let regs = "local a, b = 1.0, 2.0 for i = 1, 2000000 do local c = a local d = b a = c + d * 0.5 b = d - c * 0.25 end";
    let (mut f, mut r) = (f64::MAX, f64::MAX);
    for _ in 0..7 {
        f = f.min(run(fields));
        r = r.min(run(regs));
    }
    println!("field updates: {:.2} times the register control", f / r);
    assert!(f / r < 1.55, "field updates {:.2} times the register control (bound 1.55)", f / r);
}

/// `obj:m()` stores the method word by word: merged from the instance, class and string
/// library branches it was reloaded with one 16-byte load over two 8-byte stores, a stall
/// per method call. Timed like `field_reads_do_not_stall`: string method calls against the
/// register control, per unit of fuel. Before: 4.20 and 4.42; after: 3.60 and 3.67.
#[test]
#[ignore = "timed (perf_numeric)"]
fn method_lookups_do_not_stall() {
    let run = |src: &str| {
        let mut lua = Lua::new(Limits::unmetered().hash_seed(0x5eed), Libs::ALL);
        let f = lua.load(src, "=t").unwrap();
        let (start, fuel) = (remedy_lua_oracle::thread_cpu(), lua.fuel_left());
        lua.call(f.value(), &[]).unwrap();
        remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64 / (fuel - lua.fuel_left()) as f64
    };
    let methods = "local s, z = 'abcdef', 0 for i = 1, 1000000 do z = s:len() end";
    let regs = "local a, b = 1.0, 2.0 for i = 1, 2000000 do local c = a local d = b a = c + d * 0.5 b = d - c * 0.25 end";
    let (mut m, mut r) = (f64::MAX, f64::MAX);
    for _ in 0..7 {
        m = m.min(run(methods));
        r = r.min(run(regs));
    }
    println!("string method calls: {:.2} times the register control", m / r);
    assert!(m / r < 3.95, "string method calls {:.2} times the register control (bound 3.95)", m / r);
}
