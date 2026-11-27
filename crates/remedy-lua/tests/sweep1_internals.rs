//! Sweep 1, internals: the collector (heap.rs), the stackless machine (vm.rs) and the
//! compiler. Each `#[ignore = "sweep1: ..."]` test is a failing repro of a finding; the
//! rest are regression guards for paths that were checked and found correct. Programs are
//! compared with the C Lua 5.4 oracle where it is built.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, Value};

/// RemedyLua's outcome must equal C Lua's (when the oracle is built); returns RemedyLua's.
fn same(prog: &str) -> Result<String, String> {
    let r = common::remedy(prog, 50_000_000);
    let o = common::oracle(prog, 500_000_000);
    {
        assert_eq!(r, o, "RemedyLua vs C Lua differ for:\n{prog}");
    }
    r
}

/// RemedyLua's outcome must be `want` (and C Lua's too, when built).
fn expect(prog: &str, want: &str) {
    let o = common::oracle(prog, 500_000_000);
    {
        assert_eq!(o, Ok(want.to_owned()), "the oracle disagrees with the test's expectation:\n{prog}");
    }
    assert_eq!(common::remedy(prog, 50_000_000), Ok(want.to_owned()), "program:\n{prog}");
}

// =====================================================================================
// To-be-closed variables
// =====================================================================================

/// INT-1 (major): `return f()` with open results inside a `<close>` scope. The `__close`
/// call is placed at the frame's register top (`call_top`), which ignores `self.top`, so
/// the __close function and its registers overwrite the pending result list.
/// Expected `30 1 2 3 ... 30` (C Lua puts __close above L->top); actual: results 3.. are
/// replaced by the __close call's own values. Fault: vm.rs:373 `call_top` (ignores
/// `self.top`) as used by vm.rs:1081 `do_return` -> `close_level`.
#[test]
fn tbc_close_clobbers_open_return_values() {
    expect(
        r#"
local function many()
  local t = {} for i = 1, 30 do t[i] = i end
  return table.unpack(t)
end
local function f()
  local x <close> = setmetatable({}, {__close = function()
    local a, b, c, d, e, g, h, i, j, k = "A", "B", "C", "D", "E", "F", "G", "H", "I", "J"
  end})
  return many()
end
return table.concat({select('#', f()), f()}, ' ')
"#,
        "\"30 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30\"",
    );
}

/// INT-1b (major): same with `return ...` (VarArg MULTI above the register top).
#[test]
fn tbc_close_clobbers_vararg_return() {
    expect(
        r#"
local function f(...)
  local x <close> = setmetatable({}, {__close = function()
    local a, b, c, d, e, g, h, i, j, k = "A", "B", "C", "D", "E", "F", "G", "H", "I", "J"
  end})
  return ...
end
return table.concat({f(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12)}, ' ')
"#,
        "\"1 2 3 4 5 6 7 8 9 10 11 12\"",
    );
}

/// INT-2 (major): an error unwinding to `pcall` through a frame with two `<close>`
/// variables. `on_error` truncates the frames to the protected one BEFORE closing, so
/// the first `__close` call runs at the protected frame's register top, its registers
/// overwrite the deeper frame's still-pending tbc slot, and the next `st(slot)` reads
/// garbage: the second `__close` silently never runs (call_close returns Ok when the
/// value has no __close). Expected `"B A"`, actual `"B"`.
/// Fault: vm.rs:1312 (truncate before close_with_error) + vm.rs:373 call_top.
#[test]
fn tbc_pending_slot_clobbered_while_unwinding_to_pcall() {
    expect(
        r#"
local log = {}
local A = setmetatable({}, {__close = function() log[#log + 1] = "A" end})
local B = setmetatable({}, {__close = function()
  local r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11, r12 = 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
  log[#log + 1] = "B"
end})
local function inner()
  local p1, p2, p3, p4, p5, p6, p7, p8 = 1, 2, 3, 4, 5, 6, 7, 8
  local a <close> = A
  local q1, q2, q3, q4 = 1, 2, 3, 4
  local b <close> = B
  error("boom")
end
pcall(function() inner() end)
return table.concat(log, " ")
"#,
        "\"B A\"",
    );
}

/// INT-2b (major): the same clobbering when the error leaves a library callback
/// (`call_nested` truncates to `bdepth`, then closes at `pos`). vm.rs:1141-1142.
#[test]
fn tbc_pending_slot_clobbered_in_nested_call_error() {
    expect(
        r#"
local log = {}
local A = setmetatable({}, {__close = function() log[#log + 1] = "A" end})
local B = setmetatable({}, {__close = function()
  local r1, r2, r3, r4, r5, r6, r7, r8, r9, r10, r11, r12 = 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0
  log[#log + 1] = "B"
end})
local function inner()
  local p1, p2, p3, p4, p5, p6, p7, p8 = 1, 2, 3, 4, 5, 6, 7, 8
  local a <close> = A
  local q1, q2, q3, q4 = 1, 2, 3, 4
  local b <close> = B
  error("boom")
end
pcall(table.sort, {1, 2, 3}, function(x, y) inner() end)
return table.concat(log, " ")
"#,
        "\"B A\"",
    );
}

/// INT-2c (major): pending tbc slots above the protected frame's top are also wiped by
/// `clear_dead_stack` when the first `__close` collects (same root cause).
#[test]
fn tbc_pending_slot_wiped_by_gc_in_close() {
    expect(
        r#"
local log = {}
local A = setmetatable({}, {__close = function() log[#log + 1] = "A" end})
local B = setmetatable({}, {__close = function() collectgarbage() log[#log + 1] = "B" end})
local function inner()
  local p1, p2, p3, p4, p5, p6, p7, p8 = 1, 2, 3, 4, 5, 6, 7, 8
  local a <close> = A
  local q1, q2, q3, q4 = 1, 2, 3, 4
  local b <close> = B
  error("boom")
end
pcall(function() inner() end)
return table.concat(log, " ")
"#,
        "\"B A\"",
    );
}

/// INT-3 (major): `coroutine.close` drops an error raised by a `__close`. close_with_error
/// reports a replaced error through Ok(err), and coroutine.rs `close` ignores the Ok value.
/// Expected `false boom`, actual `true`. Fault: stdlib/coroutine.rs `close` (`Ok(_) => {}`).
#[test]
fn coroutine_close_reports_close_error() {
    expect(
        r#"
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function() error("boom", 0) end})
  coroutine.yield()
end)
coroutine.resume(co)
local ok, e = coroutine.close(co)
return tostring(ok) .. " " .. tostring(e)
"#,
        "\"false boom\"",
    );
}

/// INT-20 (major): a coroutine that dies with an error closes its pending `<close>`
/// variables at once (on_error -> close_with_error(0, ..)). Lua 5.4 (manual 3.3.8): "if a
/// coroutine ends with an error, it does not unwind its stack, so it does not close any
/// variable"; they close on `coroutine.close`. So __close runs too early (before
/// `resume` even returns), and runs when the script never calls close. Expected
/// `resumed,closed`, actual `closed,resumed`. Fault: vm.rs:1326-1333.
#[test]
fn coroutine_error_does_not_close_until_coroutine_close() {
    expect(
        r#"
local log = {}
local co = coroutine.create(function()
  local x <close> = setmetatable({}, {__close = function() log[#log + 1] = "closed" end})
  error("E", 0)
end)
coroutine.resume(co)
log[#log + 1] = "resumed"
coroutine.close(co)
return table.concat(log, ",")
"#,
        "\"resumed,closed\"",
    );
}

#[test]
fn tbc_order_and_error_replacement_match_oracle() {
    same(
        r#"
local log = {}
local function mk(n, fail)
  return setmetatable({}, {__close = function(_, e)
    log[#log + 1] = n .. ":" .. tostring(e)
    if fail then error(n .. "!", 0) end
  end})
end
local function f()
  local a <close> = mk("a")
  local b <close> = mk("b", true)
  local c <close> = mk("c")
  error("orig", 0)
end
local ok, e = pcall(f)
log[#log + 1] = tostring(ok) .. "/" .. tostring(e)
local function g()
  local a <close> = mk("x")
  do local b <close> = mk("y", true) end
  return "unreached"
end
log[#log + 1] = select(2, pcall(g))
for i = 1, 3 do local z <close> = mk("loop" .. i) if i == 2 then break end end
local function h() for i = 1, 3 do local z <close> = mk("g" .. i) if i == 2 then goto out end end ::out:: return 1 end
h()
return table.concat(log, ",")
"#,
    )
    .unwrap();
}

#[test]
fn tbc_yield_inside_close_and_across_pcall() {
    same(
        r#"
local log = {}
local co = coroutine.wrap(function()
  local x <close> = setmetatable({}, {__close = function() log[#log+1] = "closing" end})
  local ok, v = pcall(function() local y = coroutine.yield("in pcall") return y * 2 end)
  log[#log+1] = tostring(ok) .. ":" .. tostring(v)
  return "done"
end)
log[#log+1] = co()
log[#log+1] = co(21)
return table.concat(log, ",")
"#,
    )
    .unwrap();
}

// =====================================================================================
// Weak tables and the collector
// =====================================================================================

/// INT-4 (critical: a live object is freed): strings in the ARRAY part of a weak-values
/// table are never marked. `traverse` skips the array when values are weak, `clear_weak`
/// keeps strings (they "count as marked") but its `keep` pass only re-marks node strings,
/// so the string is swept while the table still holds it. The stale StrRef then reads the
/// bytes of whatever string reuses the slot (`str_bytes` ignores the generation).
/// Expected the original string, actual "" or another string. Fault: heap.rs:919-927.
#[test]
fn weak_values_array_strings_survive() {
    expect(
        r#"
local t = setmetatable({}, {__mode = "v"})
local function put() for i = 1, 4 do t[i] = string.rep("s", 3) .. i end end
put()
local function scrub() local a, b, c, d, e, f, g, h = 1, 2, 3, 4, 5, 6, 7, 8 end
scrub()
collectgarbage() collectgarbage()
local junk = {}
for i = 1, 200 do junk[i] = "junk" .. i end
return table.concat(t, ",")
"#,
        "\"sss1,sss2,sss3,sss4\"",
    );
}

/// INT-5 (critical: a live object is freed, then aliases a new one): in a weak-KEYS table
/// (ephemeron) with a string key not referenced elsewhere, `mark_ephemeron` asks
/// `is_marked(key)` (false for an unmarked string), so the value is never marked, but
/// `clear_weak` never removes string-keyed entries. The value table is swept while the
/// entry stays; reads through the stale TableRef see the empty slot, or the table that
/// reuses the slot. C Lua treats string keys as always marked. Expected `1`, actual nil
/// (or another table's field). Fault: heap.rs:791 (is_marked on strings in ephemerons).
#[test]
fn ephemeron_string_key_keeps_value() {
    expect(
        r#"
local t = setmetatable({}, {__mode = "k"})
-- Stored from a frame that is gone, so no stale register keeps the value alive.
local function put() t[string.rep("k", 2) .. "ey"] = {x = 1} end
put()
local function scrub() local a, b, c, d, e, f, g, h = 1, 2, 3, 4, 5, 6, 7, 8 end
scrub()
collectgarbage() collectgarbage()
local fresh = {}
for i = 1, 50 do fresh[i] = {x = 1000 + i} end
local out
for k, v in pairs(t) do out = v.x end
return out
"#,
        "1",
    );
}

#[test]
fn weak_tables_clear_like_c_lua() {
    same(
        r#"
local wk = setmetatable({}, {__mode = "k"})
local wv = setmetatable({}, {__mode = "v"})
local wkv = setmetatable({}, {__mode = "kv"})
local keep = {}
for i = 1, 20 do
  local k, v = {}, {}
  wk[k] = v; wv[i] = v; wkv[k] = i
  if i % 3 == 0 then keep[#keep + 1] = k; keep[#keep + 1] = v end
end
-- ephemeron chain: value refers to the next key
local e = setmetatable({}, {__mode = "k"})
local k1, k2, k3 = {}, {}, {}
e[k1] = k2; e[k2] = k3; e[k3] = "end"
local root = k1
k1, k2, k3 = nil, nil, nil
collectgarbage() collectgarbage()
local function count(t) local n = 0 for _ in pairs(t) do n = n + 1 end return n end
local chain = 0
local cur = root
while type(cur) == "table" do chain = chain + 1; cur = e[cur] end
return count(wk), count(wv), count(wkv), chain, cur
"#,
    )
    .unwrap();
}

/// INT-6 (critical: a live object is freed): a library function that holds values read RAW from a table in a Rust
/// Vec while a metamethod runs Lua code. `table.unpack` reads t[1] raw, then t[2] calls
/// __index, which drops t[1] and collects: t[1]'s table is freed although it is about to
/// be returned (C Lua keeps it on the stack). Expected "payload", actual nil (stale ref).
/// Fault: stdlib/table.rs `unpack` (values not rooted in `scratch`, as `sort` does).
#[test]
fn unpack_keeps_values_alive_across_index_metamethod() {
    expect(
        r#"
local t = setmetatable({}, {__index = function(t, k)
  rawset(t, 1, nil)
  collectgarbage() collectgarbage()
  return k
end})
t[1] = {"payload"}
local a, b = table.unpack(t, 1, 2)
local fresh = {}
for i = 1, 50 do fresh[i] = {"other" .. i} end
return tostring(a[1]) .. " " .. b
"#,
        "\"payload 2\"",
    );
}

/// Random object graphs mutated while the collector is forced to run constantly
/// (`collectgarbage("step")` between operations, plus full collections), checked against
/// C Lua. Covers table/rawset/setmetatable/upvalue/closure/coroutine stores during every
/// phase.
#[test]
fn gc_stress_random_graphs_match_oracle() {
    // On a 64 KiB stack: data-driven native recursion overflows here, not in production.
    super::common::small_stack(gc_stress_random_graphs_match_oracle_body);
}

fn gc_stress_random_graphs_match_oracle_body() {
    let iters: u64 = std::env::var("SWEEP_GC_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(40);
    let seed0: u64 = std::env::var("SWEEP_GC_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    for s in seed0..seed0 + iters {
        let prog = gc_program(s);
        let r = common::remedy(&prog, 200_000_000);
        let o = common::oracle(&prog, 2_000_000_000);
        assert_eq!(r, o, "seed {s}:\n{prog}");
    }
}

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
}

/// A program that builds and rewires a graph of tables, closures, upvalues, weak tables
/// and coroutines, stepping the collector between operations, then prints a checksum of
/// everything reachable.
fn gc_program(seed: u64) -> String {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut body = String::new();
    let n = 60 + r.below(80);
    for _ in 0..n {
        let i = r.below(16) + 1;
        let j = r.below(16) + 1;
        let op = match r.below(16) {
            0 => format!("N[{i}] = {{v = {i}, s = 'str' .. {j}}}"),
            1 => format!("if N[{i}] then N[{i}].next = N[{j}] end"),
            2 => format!("if N[{i}] then rawset(N[{i}], 'r', {{N[{j}], 'x' .. {i}}}) end"),
            3 => format!("if N[{i}] then setmetatable(N[{i}], {{__index = N[{j}], tag = 't' .. {j}}}) end"),
            4 => format!("N[{i}] = nil"),
            5 => format!(
                "do local up = N[{j}] or {{v = -{j}}}; F[{i}] = function(x) if x then up = x end return up end end"
            ),
            6 => format!("if F[{i}] then F[{i}](N[{j}] or {{v = {j} * 100}}) end"),
            7 => format!("W[N[{i}] or {i}] = N[{j}] or 'w{j}'"),
            8 => format!("V['k{i}'] = N[{j}] or ('wv' .. {j})"), // array-part strings: INT-4
            9 => format!(
                "C[{i}] = coroutine.wrap(function(a) local mine = {{a}}; while true do local nxt = {{mine, coroutine.yield(mine)}} mine = nxt end end)"
            ),
            10 => format!(
                "if C[{i}] then local ok, x = pcall(C[{i}], N[{j}] or {j}) if ok then K[{i}] = x end end"
            ),
            11 => format!(
                "do local t = {{}} for k = 1, {} do t[k] = {{k, tostring(k)}} end N[{i}] = t end",
                r.below(40) + 1
            ),
            12 => format!("for k, v in pairs(N) do if k % {} == 0 then N[k] = nil end end", r.below(5) + 2),
            13 => format!("S[{i}] = table.concat({{'a', {i}, 'b', {j}}})"),
            14 => format!("collectgarbage()"),
            _ => format!("E[N[{i}] or 0] = function() return N[{j}] end"),
        };
        body.push_str(&op);
        body.push_str(" step()\n");
    }
    format!(
        r#"
local N, F, C, K, S = {{}}, {{}}, {{}}, {{}}, {{}}
local W = setmetatable({{}}, {{__mode = "k"}})
local V = setmetatable({{}}, {{__mode = "v"}})
local E = {{}}
local function step() collectgarbage("step") collectgarbage("step") end
{body}
collectgarbage() collectgarbage()
local seen, acc = {{}}, {{}}
local function walk(v, d)
  local t = type(v)
  if t == "table" then
    if seen[v] then return end
    seen[v] = true
    acc[#acc + 1] = "T"
    local keys = {{}}
    for k in next, v do if type(k) ~= "table" then keys[#keys + 1] = k end end
    table.sort(keys, function(a, b) return tostring(a) < tostring(b) end)
    for _, k in ipairs(keys) do acc[#acc + 1] = tostring(k); walk(rawget(v, k), d + 1) end
    local mt = getmetatable(v)
    if mt then acc[#acc + 1] = "mt:" .. tostring(rawget(mt, "tag")) end
  elseif t == "function" then
    acc[#acc + 1] = "F"
  elseif t == "string" or t == "number" or t == "boolean" then
    acc[#acc + 1] = tostring(v)
  end
end
for i = 1, 16 do walk(N[i], 0) walk(K[i], 0) walk(S[i], 0) if F[i] then walk(F[i](), 0) end end
local wn, vn = 0, 0
for k, v in pairs(W) do wn = wn + 1 end
for k, v in pairs(V) do vn = vn + 1 end
for i = 1, 16 do walk(W[i], 0) if N[i] then walk(W[N[i]], 0) end walk(V['k' .. i], 0) end
return #acc .. ":" .. wn .. ":" .. vn .. ":" .. table.concat(acc, ","):sub(1, 3000)
"#
    )
}

/// Strings, tables and closures made while marking or sweeping survive when stored into
/// already-traversed (black) objects: every store path with the collector mid-cycle.
#[test]
fn barrier_store_paths_mid_cycle() {
    same(
        r#"
local old = {}
local holder = {}
local mt = {}
local up = {}
local function setup(x) up = x end
local function getup() return up end
for round = 1, 30 do
  collectgarbage("step")
  old[round] = {round}                       -- SetTable / SetList
  rawset(old, "r" .. round, {"raw" .. round}) -- rawset
  setmetatable(holder, {__index = {deep = "d" .. round}}) -- metatable store
  setup({"up" .. round})                      -- closed upvalue store
  table.insert(old, 1, {"ins" .. round})      -- library store
  local co = coroutine.create(function(a) local l = {a} coroutine.yield() return l end)
  coroutine.resume(co, {"co" .. round})
  collectgarbage("step")
  old["co" .. round] = co
end
collectgarbage() collectgarbage()
local acc = {}
for round = 1, 30 do
  local _, l = coroutine.resume(old["co" .. round])
  acc[#acc + 1] = old["r" .. round][1] .. l[1][1]
end
acc[#acc + 1] = holder.deep .. getup()[1] .. old[1][1] .. #old
return table.concat(acc, ",")
"#,
    )
    .unwrap();
}

/// Strings interned again while the collector is sweeping must not be freed: hammer the
/// same short strings during long sweeps.
#[test]
fn interner_reuse_during_sweep() {
    same(
        r#"
local keep = {}
for round = 1, 200 do
  for i = 1, 50 do local _ = "tmp" .. (i % 7) .. round end
  collectgarbage("step")
  keep[round % 10 + 1] = "tmp" .. (round % 7) .. round
end
collectgarbage()
return table.concat(keep, ",")
"#,
    )
    .unwrap();
}

/// Emergency collection at the memory cap, mid-instruction, while values are in flight
/// (concat operands, closure upvalues, table constructors, coroutine resume args).
#[test]
fn emergency_collection_at_cap_keeps_live_values() {
    for cap in [300 << 10, 400 << 10, 600 << 10, 1 << 20] {
        let mut lua = Lua::new(Limits::default().memory(cap).fuel(50_000_000), Libs::ALL);
        let r = lua.exec(
            r#"
local live = {}
local acc = 0
for i = 1, 4000 do
  local junk = {i, tostring(i) .. "junk", function() return i end}
  local s = "a" .. i .. "b" .. tostring(i * 2)
  live[i % 64 + 1] = {s, junk[3]}
  local co = coroutine.wrap(function(x) local y = coroutine.yield(x .. "!") return y .. "?" end)
  local p = co(s)
  acc = acc + #co(p) + live[i % 64 + 1][2]()
end
for i = 1, 64 do
  local e = live[i]
  if type(e[1]) ~= "string" or e[1]:sub(1, 1) ~= "a" or type(e[2]) ~= "function" then return "corrupt " .. i end
end
return acc
"#,
            "=t",
        );
        match r {
            Ok(v) => assert!(
                matches!(v[0], Value::Int(_)),
                "cap {cap}: {}",
                lua.to_string_lossy(v[0]).unwrap_or_default()
            ),
            Err(Error::Limit(LimitKind::Memory)) => {}
            Err(e) => panic!("cap {cap}: {e}"),
        }
    }
}

/// Collector testability gap: there is no hook to force a collection at every allocation
/// (emergency paths such as `call_nested`'s `ensure_stack` before the arguments are on
/// the stack can only be reached by memory-cap tuning). This test only documents what can
/// be driven: tiny caps that force emergency collections inside error handling.
#[test]
fn emergency_collection_during_error_handling() {
    for cap in (200..=320).step_by(8) {
        let mut lua = Lua::new(Limits::default().memory(cap << 10).fuel(20_000_000), Libs::ALL);
        let r = lua.exec(
            r#"
local keep = {}
local n = 0
for i = 1, 3000 do
  local ok, e = xpcall(function() local t = {} error({msg = "e" .. i, t}) end, function(m) return {m, tostring(m.msg) .. "h"} end)
  if not ok and e[2] == "e" .. i .. "h" then n = n + 1 end
  keep[i % 50] = e
end
return n
"#,
            "=t",
        );
        match r {
            Ok(v) => assert_eq!(v[0], Value::Int(3000), "cap {cap}K"),
            Err(Error::Limit(LimitKind::Memory)) => {}
            Err(e) => panic!("cap {cap}K: {e}"),
        }
    }
}

// =====================================================================================
// Host API and stale references
// =====================================================================================

/// INT-7 (major): `Lua::set_metatable` does not check that `t` is live. A host holding a
/// TableRef whose table was collected sets the metatable of whatever table reuses the
/// slot (heap slab `get_mut` ignores generations). `table_get`/`table_len`/`table_pairs`
/// likewise read the new occupant. Expected an error for the stale ref, actual: another
/// table gets the metatable. Fault: lib.rs:584-596 (no `check_live(Value::Table(t))`).
#[test]
fn host_set_metatable_on_stale_table_is_refused() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let stale = lua.create_table().unwrap();
    lua.collect_garbage().unwrap();
    lua.collect_garbage().unwrap();
    // Script-made tables now take the freed slot.
    lua.exec("victims = {} for i = 1, 100 do victims[i] = {} end", "=t").unwrap();
    let meta = lua.create_table().unwrap();
    let r = lua.set_metatable(stale, Some(meta));
    let victim_has_mt = lua
        .exec("for i = 1, 100 do if getmetatable(victims[i]) then return true end end return false", "=t")
        .unwrap()[0];
    assert_eq!(victim_has_mt, Value::Bool(false), "stale set_metatable gave another table a metatable");
    assert!(r.is_err(), "stale TableRef accepted");
}

// =====================================================================================
// Metamethods implemented by special library functions
// =====================================================================================

/// INT-8 (major): `__index = pcall` (any metamethod that is `pcall`/`xpcall`). call_meta
/// runs the native with FrameKind::Cont(Store(a)); pcall answers Flow::PCall and the
/// called Lua function's frame gets FrameKind::Protected instead, so its result never
/// reaches register `a` (the Cont is lost). Expected `true`, actual nil.
/// Fault: vm.rs:980-987 (Flow::PCall ignores `kind` = Cont).
#[test]
fn pcall_as_index_metamethod() {
    expect(
        r#"
local mt = {}
mt.__index = pcall
mt.__call = function(self, k) return "called:" .. k end
local t = setmetatable({}, mt)
local x = t.foo
local y = setmetatable({}, {__add = pcall, __call = function(_, b) return b end}) + 5
return tostring(x) .. " " .. tostring(y)
"#,
        "\"true true\"",
    );
}

/// INT-9 (major): a `coroutine.wrap` function used as a metamethod. call_native returns
/// Called::Switched (the coroutine is already running) and call_meta turns that into
/// "attempt to yield across a metamethod" raised INSIDE the resumed coroutine, killing
/// it. C Lua resumes normally. Expected `1 2`. Fault: vm.rs:2265 (Switched after a
/// resume) / Flow::Resume with a Cont kind.
#[test]
fn wrap_function_as_index_metamethod() {
    expect(
        r#"
local gen = coroutine.wrap(function() for i = 1, 3 do coroutine.yield(i) end end)
local u = setmetatable({}, {__index = gen})
local a = u.a
local b = u.b
return a .. " " .. b
"#,
        "\"1 2\"",
    );
}

/// INT-10 (major): `__index = coroutine.yield` inside a coroutine. C Lua 5.4 yields from
/// a C function called as a metamethod (the metamethod call is yieldable from Lua code);
/// RemedyLua raises "attempt to yield across a C-call boundary" (call_meta bumps `nny`
/// for natives). Fault: vm.rs:2253.
#[test]
fn yield_as_index_metamethod() {
    expect(
        r#"
local co = coroutine.wrap(function()
  local t = setmetatable({}, {__index = coroutine.yield})
  return "got " .. t.key
end)
local a, b = co()
return tostring(b) .. " " .. co(42)
"#,
        "\"key got 42\"",
    );
}

#[test]
fn metamethod_yields_and_cont_frames_match_oracle() {
    same(
        r#"
local log = {}
local mt = {}
mt.__index = function(t, k) return coroutine.yield("idx " .. k) end
mt.__add = function(a, b) return coroutine.yield("add") + 1 end
mt.__lt = function(a, b) return coroutine.yield("lt") end
mt.__le = function(a, b) return coroutine.yield("le") end
mt.__eq = function(a, b) return coroutine.yield("eq") end
mt.__concat = function(a, b) return coroutine.yield("cat") end
mt.__len = function(a) return coroutine.yield("len") end
mt.__unm = function(a) return coroutine.yield("unm") end
mt.__newindex = function(t, k, v) rawset(t, k, coroutine.yield("ni") .. v) end
mt.__call = function(self, x) return coroutine.yield("call") * x end
local co = coroutine.wrap(function()
  local a, b = setmetatable({}, mt), setmetatable({}, mt)
  local r = {}
  r[#r+1] = a.foo
  r[#r+1] = a + 1
  r[#r+1] = tostring(a < b)
  r[#r+1] = tostring(a <= b)
  r[#r+1] = tostring(a == b)
  r[#r+1] = "x" .. a .. "y"
  r[#r+1] = #a
  r[#r+1] = -a
  a.k = "v"; r[#r+1] = rawget(a, "k")
  r[#r+1] = a(3)
  if a < b then r[#r+1] = "then" else r[#r+1] = "else" end
  return table.concat(r, "|")
end)
local v = co()
local answers = {"I", 10, false, true, 1, "C", 7, -1, "N", 5, true}
local i = 0
while true do
  log[#log+1] = tostring(v)
  i = i + 1
  if i > #answers + 1 then break end
  v = co(answers[i])
  if type(v) == "string" and v:find("|") then log[#log+1] = v break end
end
return table.concat(log, ";")
"#,
    )
    .unwrap();
}

// =====================================================================================
// Limits through coroutines and nested calls
// =====================================================================================

/// INT-11 (critical: fuel does not bound the work): a `__call` chain. Each link of
/// `precall`'s tag loop shifts every argument up one slot (copy_within) and the chain may
/// be 2000 long, so one Call instruction costs ~2000^2/2 value moves and charges no fuel
/// for them. A loop of such calls runs ~10^6 times longer than its fuel suggests (a 1M
/// fuel run took over a minute in a debug build). Expected: each link costs fuel (at
/// least one unit), actual: a 1999-link call costs a handful. Fault: vm.rs:828-839.
#[test]
fn call_chain_work_is_charged_fuel() {
    let mut lua = Lua::new(Limits::default().fuel(1_000_000_000), Libs::ALL);
    lua.exec("f = function(...) return select('#', ...) end for i = 1, 1999 do f = setmetatable({}, {__call = f}) end", "=t")
        .unwrap();
    let f = lua.get_global("f").unwrap();
    let r = lua.call(f, &[]).unwrap();
    assert_eq!(r, vec![Value::Int(1999)]);
    let used = 1_000_000_000 - lua.fuel_left();
    assert!(used >= 1999, "a 1999-link __call chain cost only {used} fuel");
}

#[test]
fn limits_kill_nested_coroutines_and_state_stays_usable() {
    let mut lua = Lua::new(Limits::default().fuel(200_000), Libs::ALL);
    let r = lua.exec(
        r#"
local function deep(n)
  if n == 0 then while true do end end
  local co = coroutine.wrap(function()
    local x <close> = setmetatable({}, {__close = function() error("must not run") end})
    return pcall(table.sort, {3, 2, 1}, function(a, b) return deep(n - 1) end)
  end)
  return pcall(co)
end
return deep(6)
"#,
        "=t",
    );
    assert_eq!(r, Err(Error::Limit(LimitKind::Fuel)));
    let after = lua.exec(
        "local co = coroutine.running() local _, main = coroutine.running() return main, coroutine.isyieldable()",
        "=t",
    );
    assert_eq!(after.unwrap(), vec![Value::Bool(true), Value::Bool(false)]);
}

#[test]
fn memory_limit_cannot_be_caught_through_coroutines_or_close() {
    let mut lua = Lua::new(Limits::default().memory(2 << 20), Libs::ALL);
    let r = lua.exec(
        r#"
local survived = false
local co = coroutine.wrap(function()
  local x <close> = setmetatable({}, {__close = function() survived = true end})
  local t = {}
  for i = 1, 1e9 do t[i] = ("x"):rep(64) .. i end
end)
pcall(co)
xpcall(co, function() survived = true end)
return survived
"#,
        "=t",
    );
    assert_eq!(r, Err(Error::Limit(LimitKind::Memory)));
    assert_eq!(lua.exec("return 1", "=t").unwrap(), vec![Value::Int(1)]);
}

// =====================================================================================
// Coroutines, pcall and error unwinding through mixed frames (random, vs the oracle)
// =====================================================================================

#[test]
fn random_pcall_coroutine_tbc_programs_match_oracle() {
    let iters: u64 = std::env::var("SWEEP_CO_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(150);
    let seed0: u64 = std::env::var("SWEEP_CO_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut known = 0;
    for s in seed0..seed0 + iters {
        let prog = co_program(s);
        let r = common::remedy(&prog, 20_000_000);
        if r == Ok("syntax error".to_owned()) {
            continue; // INT-17
        }
        let o = common::oracle(&prog, 200_000_000);
        {
            if let (Ok(a), Ok(b)) = (&r, &o)
                && only_missing_closes(a, b)
            {
                known += 1; // INT-2: a pending __close skipped while unwinding
                continue;
            }
            assert_eq!(r, o, "seed {s}:\n{prog}");
        }
    }
    println!("{known} programs differed only in __close lines (INT-2, INT-20)");
}

/// Whether `got` and `want` differ only in `close ...` log lines: some missing (INT-2) or
/// run at another time (INT-20). Everything else must be equal, in order.
fn only_missing_closes(got: &str, want: &str) -> bool {
    if got == want {
        return false;
    }
    let strip = |s: &str| {
        let lines = s.split('\n').map(|l| l.trim_start_matches('"'));
        lines.filter(|l| !l.starts_with("close ")).map(str::to_owned).collect::<Vec<_>>()
    };
    strip(got) == strip(want)
}

/// A random nest of pcall/xpcall/coroutine/wrap/sort-callback/tbc/error/yield blocks; every
/// event is logged.
fn co_program(seed: u64) -> String {
    let mut r = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) | 1);
    fn block(r: &mut Rng, depth: usize, out: &mut String, id: &mut usize) {
        *id += 1;
        let me = *id;
        let leaf = depth == 0 || r.below(4) == 0;
        if leaf {
            let s = match r.below(7) {
                0 => format!("L('e{me}') error('E{me}', 0)"),
                1 => format!("L('y{me}=' .. tostring(Y('Y{me}')))"),
                2 => format!("L('v{me}')"),
                3 => format!("error({{code = {me}}})"),
                4 => format!("L('n{me}') return 'r{me}'"),
                5 => format!("local z <close> = T('t{me}') L('in{me}')"),
                _ => format!("L('c{me}=' .. tostring(coroutine.isyieldable()))"),
            };
            out.push_str(&s);
            return;
        }
        let mut inner = String::new();
        block(r, depth - 1, &mut inner, id);
        let mut inner2 = String::new();
        if r.below(2) == 0 {
            block(r, depth - 1, &mut inner2, id);
        }
        let s = match r.below(9) {
            0 => format!("L('p{me}', pcall(function() {inner} {inner2} end))"),
            1 => format!(
                "L('x{me}', xpcall(function() {inner} {inner2} end, function(m) L('h{me}') return 'H' .. tostring(type(m) == 'table' and m.code or m) end))"
            ),
            2 => format!(
                "do local co = coroutine.create(function() {inner} {inner2} end) \
                 for i = 1, 4 do L('R{me}', coroutine.resume(co, i)) end L('S{me}=' .. coroutine.status(co)) \
                 L('CL{me}', coroutine.close(co)) end"
            ),
            3 => format!(
                "do local w = coroutine.wrap(function() {inner} {inner2} end) \
                 L('W{me}', pcall(w)) L('W{me}b', pcall(function() return w() end)) end"
            ),
            4 => format!("do local z <close> = T('c{me}') {inner} {inner2} end"),
            5 => {
                format!("pcall(table.sort, {{3, 1, 2}}, function(a, b) {inner} return a < b end) L('so{me}')")
            }
            6 => format!(
                "for i = 1, 2 do local q <close> = T('f{me}' .. i) if i == 2 then break end {inner} end"
            ),
            7 => format!(
                "L('g{me}', select(2, pcall(string.gsub, 'ab', '%w', function(c) {inner} return c end)))"
            ),
            _ => format!(
                "do local ok = pcall(function() local z <close> = T('pc{me}') {inner} end) L('pc{me}', ok) {inner2} end"
            ),
        };
        out.push_str(&s);
    }
    let mut body = String::new();
    let mut id = 0;
    // Depth 3: deeper nests hit INT-17 (parser native-stack limit) in debug builds.
    block(&mut r, 3, &mut body, &mut id);
    format!(
        r#"
local log = {{}}
local function S(v) if type(v) == "table" then return "{{" .. tostring(v.code) .. "}}" end return tostring(v) end
local function L(...) local p = {{}} for i = 1, select('#', ...) do p[i] = S((select(i, ...))) end log[#log + 1] = table.concat(p, ":") end
local function Y(v) if coroutine.isyieldable() then return coroutine.yield(v) end return "noy" end
local function T(n) return setmetatable({{}}, {{__close = function(_, e) L("close " .. n, e) end}}) end
local function main() {body} end
L("top", pcall(main))
return table.concat(log, "\n")
"#
    )
}

// =====================================================================================
// Compiler: closures in loops, assignment order, goto, folding, and/or chains
// =====================================================================================

#[test]
fn closures_capture_fresh_loop_variables() {
    same(
        r#"
local fs = {}
for i = 1, 3 do fs[#fs + 1] = function() i = i + 10 return i end end
for _, v in ipairs({"a", "b"}) do fs[#fs + 1] = function() return v end end
local j = 0
while j < 3 do j = j + 1 local k = j fs[#fs + 1] = function() k = k * 2 return k end end
repeat local r = j fs[#fs + 1] = function() return r end j = j - 1 until r < 2
do
  local n = 0
  ::top::
  local m = n
  fs[#fs + 1] = function() m = m + 100 return m end
  n = n + 1
  if n < 3 then goto top end
end
local out = {}
for i, f in ipairs(fs) do out[i] = tostring(f()) .. "/" .. tostring(f()) end
return table.concat(out, " ")
"#,
    )
    .unwrap();
}

#[test]
fn multiple_assignment_evaluation_order() {
    same(
        r#"
local log = {}
local t = setmetatable({}, {__newindex = function(t, k, v) log[#log + 1] = tostring(k) .. "=" .. tostring(v) rawset(t, k, v) end})
local i = 1
i, t[i] = i + 1, 20
local a, b, c = (function() return 1, 2, 3 end)()
local x, y = 1
local p = {}
p[1], p[2], p[1] = 1, 2, 3
local q = {}
local n = 1
n, q[n], q[n + 1] = n + 1, "A", "B"
local function f() return "f1", "f2" end
local r1, r2, r3, r4 = f(), f()
local s1, s2 = (f())
return table.concat(log, ",") .. "|" .. i .. "|" .. a .. b .. c .. tostring(y) .. "|" .. p[1] .. p[2]
  .. "|" .. n .. tostring(q[1]) .. tostring(q[2]) .. tostring(q[3]) .. "|" .. r1 .. r2 .. r3 .. tostring(r4) .. tostring(s2)
"#,
    )
    .unwrap();
}

#[test]
fn constant_folding_edge_cases() {
    same(
        r#"
local r = {}
local function add(v) r[#r + 1] = string.format("%q", v) end
add(math.maxinteger + 1) add(math.mininteger - 1) add(math.mininteger // -1) add(math.mininteger % -1)
add(-0.0) add(0.0 * -1) add(1 / -0.0) add(-(0.0)) add(- -0.0) add(0/0 ~= 0/0)
add(3 // 0.0) add(-3 // 0.0) add(3 % math.huge) add(-3 % math.huge) add(3.5 % -2) add(-3.5 % 2)
add(5 // 2) add(5.0 // 2) add(-5 // 2) add(7 % -3) add(2^63) add(2^-1) add(1 << 63) add(1 << 64) add(-1 >> 1)
add(1 >> -1) add(3 & 1.0) add("10" + 1) add("0x10" * 1) add(10 / 2) add(1e308 * 10) add(-1e308 * 10)
add(math.mininteger * -1) add(9007199254740993 + 0.0) add(1 == 1.0) add("a" .. 1 .. 2.0)
add(not nil == true) add(2^53 == 2^53 + 1)
local ok, e = pcall(function() return 1 // 0 end) add(e)
ok, e = pcall(function() return 1 % 0 end) add(e)
ok, e = pcall(function() return 1.5 | 0 end) add(e)
return table.concat(r, " ")
"#,
    )
    .unwrap();
}

#[test]
fn and_or_chains_and_jump_patching() {
    let mut r = Rng(12345);
    let atoms = ["nil", "false", "true", "0", "1", "'s'", "x", "y", "z", "(x and y)", "(nil or false)"];
    let ops = [" and ", " or "];
    for _ in 0..60 {
        let mut e = String::from(atoms[r.below(atoms.len())]);
        for _ in 0..r.below(8) + 1 {
            let op = ops[r.below(2)];
            let a = atoms[r.below(atoms.len())];
            e = match r.below(3) {
                0 => format!("{e}{op}{a}"),
                1 => format!("({e}){op}{a}"),
                _ => format!("{a}{op}({e})"),
            };
            if r.below(4) == 0 {
                e = format!("not ({e})");
            }
        }
        let prog = format!(
            "local out = {{}}\nfor _, x in ipairs({{false, 1}}) do for _, y in ipairs({{false, 2}}) do local z = nil\n\
             local v = {e}\nlocal c if {e} then c = 'T' else c = 'F' end\nwhile {e} do c = c .. 'w' break end\n\
             out[#out + 1] = tostring(v) .. c end end\nreturn table.concat(out, ',')"
        );
        same(&prog).unwrap();
    }
}

#[test]
fn register_pressure_in_nested_expressions() {
    same(
        r#"
local function f(...) return select('#', ...), ... end
local t = {f(1, f(2, f(3, f(4, 5))))}
local s = ((1 + 2) * (3 + (4 * (5 + (6 * (7 + (8 * (9 + 10))))))))
local u = {[f(1)] = f(2), f(3), f(4), [("k"):rep(2)] = {f(5, f(6))}, f(7, 8, 9)}
local v = ("a"):rep(2) .. ("b"):rep(f(1)) .. (#t + #u) .. tostring(u.kk[2])
local m = setmetatable({}, {__index = function(_, k) return k * 2 end})
local w = m[m[m[1] + m[2]] + m[m[3]]] + (m[4] or 0) * (m[5] and m[6] or m[7])
return table.concat(t, ","), s, #u, v, w, select('#', table.unpack(u))
"#,
    )
    .unwrap();
}

#[test]
fn goto_continue_closes_upvalues_and_tbc() {
    same(
        r#"
local log, fs = {}, {}
for i = 1, 4 do
  local x <close> = setmetatable({}, {__close = function() log[#log + 1] = "c" .. i end})
  local cap = i * 10
  fs[#fs + 1] = function() cap = cap + 1 return cap end
  if i % 2 == 0 then goto continue end
  log[#log + 1] = "body" .. i
  ::continue::
end
do
  local k = 0
  ::again::
  local y <close> = setmetatable({}, {__close = function() log[#log + 1] = "y" .. k end})
  local c2 = k
  fs[#fs + 1] = function() return c2 end
  k = k + 1
  if k < 3 then goto again end
end
for _, f in ipairs(fs) do log[#log + 1] = f() end
return table.concat(log, ",")
"#,
    )
    .unwrap();
}

#[test]
fn varargs_and_open_results_at_frame_boundaries() {
    same(
        r#"
local function id(...) return ... end
local function cnt(...) return select('#', ...) end
local function tail(...) return id(...) end
local function nils() return nil, nil, nil end
local co = coroutine.wrap(function(...) local a = {...} while true do local b = {coroutine.yield(table.unpack(a, 1, #a + 2))} a = b end end)
local r = {}
r[#r+1] = cnt(id())
r[#r+1] = cnt(id(nil, nil))
r[#r+1] = cnt(tail(1, nil, 3, nil))
r[#r+1] = cnt(nils())
r[#r+1] = cnt(pcall(id, nil, nil))
r[#r+1] = cnt(co(1, 2))
r[#r+1] = cnt(co())
r[#r+1] = cnt(select(2, pcall(error)))
r[#r+1] = cnt(table.unpack({}, 1, 3))
r[#r+1] = cnt(setmetatable({}, {__call = function(self, ...) return ... end})(1, nil, nil))
r[#r+1] = cnt(xpcall(id, print, 1, nil))
local big = {} for i = 1, 250 do big[i] = i end
r[#r+1] = cnt(id(table.unpack(big)))
r[#r+1] = select(250, tail(table.unpack(big)))
return table.concat(r, ",")
"#,
    )
    .unwrap();
}

#[test]
fn tail_calls_through_call_metamethods() {
    same(
        r#"
local depth = 0
local obj = setmetatable({}, {__call = function(self, n) if n == 0 then return "bottom" end return self(n - 1) end})
local function loop(n) if n == 0 then return "done" end return loop(n - 1) end
local function viaobj(n) return obj(n) end
local chain = setmetatable({}, {__call = setmetatable({}, {__call = function(a, b, c) return type(a), type(b), c end})})
return obj(500), loop(1000000), viaobj(10), chain(7)
"#,
    )
    .unwrap();
}

/// INT-12 (major, edge): numeric for loop with an integer start/step and a STRING limit
/// that is an exact integer above 2^53. RemedyLua converts the limit through a float
/// (`tonumber(..).as_num()`), losing precision; C Lua converts it with l_strton to an
/// integer. Expected 2 iterations, actual 1. Fault: vm.rs:2185 (`and_then(Value::as_num)`
/// for string limits).
#[test]
fn for_loop_string_limit_precision() {
    expect(
        r#"
local c = 0
for i = 9007199254740992, "9007199254740993" do c = c + 1 end
return c
"#,
        "2",
    );
}

#[test]
fn numeric_for_edges() {
    same(
        r#"
local r = {}
local function n(a, b, c) local k = 0 for i = a, b, c do k = k + 1 if k > 100 then break end end return k end
r[#r+1] = n(math.maxinteger - 2, math.maxinteger, 1)
r[#r+1] = n(math.mininteger, math.mininteger + 2, 1)
r[#r+1] = n(math.mininteger + 2, math.mininteger, -1)
r[#r+1] = n(1, 0, 1)
r[#r+1] = n(1, 1.5, 1)
r[#r+1] = n(1, -1e300, -1)
r[#r+1] = n(1, 1e300, 1)
r[#r+1] = n(1, 0/0, 1)
r[#r+1] = n(0.1, 1, 0.3)
r[#r+1] = n(1, math.maxinteger, math.maxinteger)
r[#r+1] = n(-1, math.mininteger, math.mininteger)
r[#r+1] = n(3, "5", 1)
r[#r+1] = n(1.0, 3, 1)
r[#r+1] = select(2, pcall(n, 1, 2, 0))
r[#r+1] = select(2, pcall(n, 1.0, 2, 0))
for i = 1, 3 do local j = i i = 10 r[#r+1] = j end
return table.concat(r, ",")
"#,
    )
    .unwrap();
}

/// INT-13 (minor): the 'for' limit/step/initial error text. Lua 5.4.7 says
/// "bad 'for' limit (number expected, got table)"; RemedyLua says
/// "'for' limit must be a number". Fault: vm.rs:2186/2223.
#[test]
fn for_loop_error_messages() {
    same("local ok, e = pcall(function() for i = 1, {} do end end) return e").unwrap();
    same("local ok, e = pcall(function() for i = 1.0, 2, 'x' do end end) return e").unwrap();
    same("local ok, e = pcall(function() for i = {}, 2 do end end) return e").unwrap();
}

/// INT-14 (major): `coroutine.yield` inside a `__close` run on a normal block exit (or a
/// `return`, `break`, `goto`). Lua 5.4 lets __close yield; RemedyLua runs every __close
/// through `call_nested`, which bumps `nny`, so the yield fails with "attempt to yield
/// across a C-call boundary". Fault: vm.rs:807-813 (`call_close` -> `call_nested`) called
/// from close_level (Jmp/Close/Return).
#[test]
fn yield_inside_close_on_block_exit() {
    expect(
        r#"
local log = {}
local co = coroutine.wrap(function()
  do
    local x <close> = setmetatable({}, {__close = function() log[#log+1] = "closing"; coroutine.yield("in close"); log[#log+1] = "closed" end})
  end
  return "done"
end)
local a = co()
local b = co()
return table.concat(log, ",") .. "|" .. a .. "|" .. b
"#,
        "\"closing,closed|in close|done\"",
    );
}

/// INT-15 (major, compiler): `x = {... x ...}` where `x` is the newest local. `assign`
/// evaluates a single `local = expr` straight into the local's register (compiler.rs:757),
/// and `table()` builds the constructor in `dst` itself when it is the newest register
/// (compiler.rs:1502), so NewTable overwrites `x` before the fields read it: `x = {x}`
/// makes a table that contains itself, `a = {#a}` gives {0}, the linked-list idiom
/// `l = {l, v}` loses the list. Expected `number,3,7/1,1,2`, actual `table,0,nil/0,nil,nil`.
#[test]
fn constructor_assigned_to_newest_local_reads_old_value() {
    expect(
        r#"
local r = {}
do local x = 5 x = {x} r[#r+1] = type(x[1]) end
do local a = {1, 2, 3} a = {#a} r[#r+1] = a[1] end
do local y = {7} y = {y[1], #y} r[#r+1] = tostring(y[1]) .. "/" .. tostring(y[2]) end
do local z = {1} z = {k = z[1]} r[#r+1] = tostring(z.k) end
do local w = 2 w = {[w] = w} r[#r+1] = tostring(w[2]) end
return table.concat(r, ",")
"#,
        "\"number,3,7/1,1,2\"",
    );
}

/// Random `x = <expr reading x>` assignments to the newest local (the INT-15 shape) over
/// every expression form, against the oracle. INT-15's constructor forms are excluded
/// (they fail); everything else must match.
#[test]
fn self_referencing_assignments_match_oracle() {
    let mut r = Rng(777);
    fn num(r: &mut Rng, d: usize) -> String {
        if d == 0 {
            return ["x", "1", "2", "x", "(x)"][r.below(5)].to_owned();
        }
        let a = num(r, d - 1);
        let b = num(r, d - 1);
        match r.below(12) {
            0 => format!("{a} + {b}"),
            1 => format!("({a}) * ({b})"),
            2 => format!("-({a})"),
            3 => format!("(({a}) < ({b}) and {a} or {b})"),
            4 => format!("id({a})"),
            5 => format!("select(2, {a}, {b})"),
            6 => format!("(({a}) == x and {b} or {a})"),
            7 => format!("(nil or {a})"),
            8 => format!("#tostring({a})"),
            9 => format!("(function() return {a} end)()"),
            10 => format!("m[{a}]"),
            _ => format!("(({a}) // 1 | 0)"),
        }
    }
    let mut stmts = String::new();
    for i in 0..80 {
        let d = 1 + r.below(3);
        let e = num(&mut r, d);
        stmts.push_str(&format!("do local x = {} x = {e} r[#r + 1] = tostring(x) end\n", i % 7 + 1));
    }
    let prog = format!(
        "local r = {{}}\nlocal function id(...) return ... end\nlocal m = setmetatable({{}}, {{__index = function(_, k) return k end}})\n{stmts}return table.concat(r, ',')"
    );
    same(&prog).unwrap();
}

/// INT-16 (major): `return obj(...)` where `obj` is callable through `__call` is not a
/// proper tail call (TailCall only reuses the frame for Lua functions, vm.rs:1985-1994),
/// so tail recursion through a callable table overflows the stack at `call_depth`.
/// C Lua 5.4 tail-calls through __call. Expected "bottom", actual "stack overflow".
#[test]
fn tail_call_through_call_metamethod_is_proper() {
    expect(
        r#"
local obj = setmetatable({}, {__call = function(self, n) if n == 0 then return "bottom" end return self(n - 1) end})
return obj(50000)
"#,
        "\"bottom\"",
    );
}

/// INT-17 (minor/perf): the parser's nesting limit is driven by native stack use
/// (`stack_bytes / 2`), and each `f(function() ... end)` level costs tens of KB of native
/// stack in a debug build, so with default limits only ~5 nested function arguments
/// parse (C Lua takes ~60). The message blames "limit is 200" though depth is ~25.
/// Fault: parser.rs:126 (stack-byte check dominates; per-level frame size is large).
#[test]
fn nested_function_arguments_parse() {
    let mut p = String::from("return 1");
    for _ in 0..12 {
        p = format!("L(pcall(function() {p} end))");
    }
    let p = format!("local function L(...) return ... end {p}");
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let r = lua.load(&p, "=t");
    assert!(r.is_ok(), "{:?}", r.err());
}

/// INT-18 (major: accounting leak, long-lived states eventually hit the memory cap):
/// `make_closure` charges `FUNC_BASE + n * (8 + UPVAL_SIZE)` for every upvalue, but an
/// upvalue object is only allocated for a NEW open upvalue; inherited upvalues (`_ENV`
/// for any closure using a global) and already-open shared ones are charged 40 bytes that
/// no sweep ever releases (the function's sweep releases `n * 8`). Every closure that
/// names a global leaks 40 bytes of the memory account. Expected no growth, actual
/// +40 bytes per closure (120000 bytes per 3000-closure round).
/// Fault: vm.rs:2158 (charge) vs heap.rs:1004-1010 (release).
#[test]
fn closure_accounting_does_not_leak() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let churn = "for i = 1, 3000 do local f = function() return print end end";
    lua.exec(churn, "=t").unwrap();
    lua.collect_garbage().unwrap();
    let base = lua.memory_used();
    for _ in 0..5 {
        lua.exec(churn, "=t").unwrap();
        lua.collect_garbage().unwrap();
    }
    let after = lua.memory_used();
    assert!(after <= base + 4096, "memory used grew from {base} to {after} bytes");
}

/// Memory accounting must not drift: build and drop many objects of every kind, collect,
/// and the used bytes return to where they started (a long-lived host state would
/// otherwise creep up to its cap and fail). Fails today only through INT-18.
#[test]
fn memory_accounting_returns_to_baseline() {
    let mut lua = Lua::new(Limits::default().memory(64 << 20), Libs::ALL);
    let churn = r#"
local t = {}
for i = 1, 3000 do
  local s = tostring(i) .. "x"
  t[i] = {s, function() return s end, coroutine.create(function(a) coroutine.yield(a) end)}
  if i % 3 == 0 then coroutine.resume(t[i][3], {i}) end
  t[s] = i
end
for i = 1, 3000, 2 do t[i] = nil end
local w = setmetatable({}, {__mode = "k"})
for i = 1, 500 do w[{}] = {i} end
local f = load("local a = ... return function() return a end")
local g = f(1)
return #t
"#;
    lua.exec(churn, "=t").unwrap();
    lua.collect_garbage().unwrap();
    lua.collect_garbage().unwrap();
    let base = lua.memory_used();
    for _ in 0..15 {
        lua.exec(churn, "=t").unwrap();
        lua.collect_garbage().unwrap();
        lua.collect_garbage().unwrap();
    }
    let after = lua.memory_used();
    assert!(
        after <= base + (64 << 10),
        "memory used grew from {base} to {after} bytes over 15 identical rounds"
    );
}

/// INT-19 (minor): "cannot resume dead coroutine" from a `wrap` function called directly
/// by `pcall` gets the position of pcall's caller ("t:3: ..."); C Lua adds no position
/// there (luaL_where(L, 1) names a C function). Fault: vm.rs:1419-1420 (`LuaError::msg`
/// regardless of `from_lua`).
#[test]
fn wrap_dead_message_position() {
    same("local w = coroutine.wrap(function() end) w() return select(2, pcall(w))").unwrap();
}

/// INT-21 (minor, host API): an error object that is not a string loses its value when it
/// crosses a host function. `Lua::call` turns any error into `Error::Runtime { message }`
/// and `from_host_error` re-raises the text, so a script's `pcall` around a host function
/// that called back into Lua sees "(error object is a table value)" instead of the table.
/// Fault: lib.rs:345-369 (`to_error` / `from_host_error` keep no Value).
#[test]
fn error_objects_survive_host_function_frames() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    lua.register("callback", |lua, args| lua.call(args[0], &[])).unwrap();
    let r = lua
        .exec("local ok, e = pcall(callback, function() error({code = 7}) end) return type(e) == 'table' and e.code", "=t")
        .unwrap();
    assert_eq!(r, vec![Value::Int(7)]);
}

#[test]
fn errors_unwind_through_host_function_frames() {
    let mut lua = Lua::new(Limits::default().fuel(1_000_000), Libs::ALL);
    lua.register("callback", |lua, args| lua.call(args[0], &[])).unwrap();
    let r = lua
        .exec(
            r#"
local log = {}
local ok, e = pcall(callback, function()
  local x <close> = setmetatable({}, {__close = function(_, err) log[#log + 1] = "closed:" .. tostring(err) end})
  error("b2", 0)
end)
log[#log + 1] = tostring(ok) .. ":" .. e
local co = coroutine.wrap(function() return callback(function() return coroutine.yield(1) end) end)
log[#log + 1] = select(2, pcall(co))
log[#log + 1] = callback(function() return callback(function() return 5 end) end)
return table.concat(log, "|")
"#,
            "=t",
        )
        .unwrap();
    assert_eq!(
        lua.to_string_lossy(r[0]).unwrap(),
        "closed:b2|false:b2|attempt to yield across a C-call boundary|5"
    );
    // A limit raised under a host function is not catchable either.
    assert_eq!(
        lua.exec("pcall(callback, function() while true do end end)", "=t"),
        Err(Error::Limit(LimitKind::Fuel))
    );
    assert_eq!(lua.exec("return 1", "=t").unwrap(), vec![Value::Int(1)]);
}

/// Decision: a test-only mode that runs a whole collection at every allocation. Programs
/// that build, drop and re-read many kinds of objects give the same results under it.
#[test]
fn gc_stress_mode_keeps_results() {
    // On a 64 KiB stack: data-driven native recursion overflows here, not in production.
    super::common::small_stack(gc_stress_mode_keeps_results_body);
}

fn gc_stress_mode_keeps_results_body() {
    let src = r#"
        local out = {}
        local mt = {__index = function(t, k) return k .. "!" end, __gc = nil}
        local function mk(n) local t = setmetatable({}, mt) for i = 1, n do t[i] = {i, tostring(i)} end return t end
        for round = 1, 5 do
            local t = mk(20)
            local w = setmetatable({}, {__mode = "k"})
            w[{}] = 1; w[t] = 2
            local co = coroutine.wrap(function(...) local s = 0 for _, v in ipairs(t) do s = s + v[1] coroutine.yield(s) end return s end)
            local last
            for _ = 1, 20 do last = co() end
            out[#out + 1] = last .. ":" .. t.missing .. ":" .. table.concat({string.rep("ab", round), ("x"):upper()}, "-")
        end
        local f = function(a) return function(b) return a .. b end end
        out[#out + 1] = f("u")("v")
        return table.concat(out, ",")
    "#;
    let run = |stress: bool| {
        let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
        lua.set_gc_stress(stress);
        let v = lua.exec(src, "=t").unwrap();
        lua.to_string_lossy(v[0]).unwrap()
    };
    assert_eq!(run(true), run(false));
}

/// Decision: the collector's knobs work: `stop` stops automatic steps, `restart` resumes
/// them, and `setpause`/`setstepmul`/`incremental` keep and return their values as C
/// Lua does.
#[test]
fn gc_knobs_work() {
    let p = r#"
        local a = collectgarbage("setpause", 100)
        local b = collectgarbage("setpause", 200)
        local c = collectgarbage("setstepmul", 400)
        local d = collectgarbage("setstepmul", 100)
        collectgarbage("incremental", 150, 300)
        local e = collectgarbage("setpause", 200)
        local f = collectgarbage("setstepmul", 100)
        collectgarbage("stop")
        local running = collectgarbage("isrunning")
        local before = collectgarbage("count")
        for i = 1, 2e4 do local _ = {i} end
        local grew = collectgarbage("count") > before + 100
        collectgarbage("restart")
        collectgarbage()
        return a, b, c, d, e, f, running, grew, collectgarbage("isrunning")
        "#;
    let ours = common::remedy(p, 100_000_000);
    assert!(ours.as_ref().is_ok_and(|s| !s.starts_with("error")), "{ours:?}");
    let c = common::oracle(p, 100_000_000);
    {
        assert_eq!(ours, c);
    }
}

/// The fast loop only shortcuts the full interpreter path: on programs that use every one
/// of its cases (calls, tail calls, returns closing upvalues, `__index` chains, string
/// methods, in-place writes and appends, `==` on tables, numeric loops), and when the fuel
/// runs out at many different points inside them, results, fuel used and memory held are
/// identical with it on and off.
#[test]
fn fast_loop_matches_full_path_exactly() {
    let prog = r#"
        local Base = {} Base.__index = Base
        function Base:get() return self.v end
        local Mid = setmetatable({}, Base) Mid.__index = Mid
        local o = setmetatable({v = 3}, Mid)
        local eqmt = {__eq = function() return true end}
        local a, b = setmetatable({}, eqmt), setmetatable({}, eqmt)
        local function fact(n, acc) if n <= 1 then return acc end return fact(n - 1, acc * n) end
        local function counter() local c = 0 return function() c = c + 1 return c end end
        local t, s = {}, 0
        for i = 1, 200 do
            t[#t + 1] = i
            t[i] = t[i] * 2
            o.v = o.v + 1
            s = s + o:get() + #("x"):rep(3) + fact(5, 1)
            local f = counter() f() s = s + f()
            if a == b then s = s + 1 end
            if t == {} then s = s - 1 end
        end
        for i = 10, 1, -3 do s = s + i end
        for x = 0.5, 2.5, 0.5 do s = s + x end
        return s, #t, o.v, Mid.get == Base.get
    "#;
    let full = common::remedy_measured(prog, 100_000_000, true);
    assert_eq!(full, common::remedy_measured(prog, 100_000_000, false));
    assert!(full.0.as_ref().is_ok_and(|s| !s.starts_with("error")), "{full:?}");
    // Stopped by fuel at many points: the same stop, fuel and memory.
    let used = full.1;
    for cut in (1..40).map(|i| used * i / 40) {
        let f = common::remedy_measured(prog, cut, true);
        let s = common::remedy_measured(prog, cut, false);
        assert_eq!(f, s, "fuel {cut}");
    }
}
