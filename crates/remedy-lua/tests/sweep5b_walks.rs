//! Sweep 5b, the walk inventory: every loop over a structure a script shapes (stacks,
//! frames, slabs, constant lists, mode strings, resume chains), paid for by fuel in
//! proportion to its length. Each test is a finding (`S5B-WALK-n`); every program runs
//! with the fast loop on and off, and the shapes run on a 64 KiB native stack.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua};

/// A state after `setup`, measuring `while true do body end`.
struct Probe {
    lua: Lua,
    f: remedy_lua::Root,
}

impl Probe {
    fn new(setup: &str, body: &str, fast: bool) -> Probe {
        let mut lua = Lua::new(
            Limits::default()
                .memory(512 << 20)
                .string_len(64 << 20)
                .fuel(1 << 40)
                .hash_seed(super::common::TEST_SEED),
            Libs::ALL,
        );
        lua.set_fast_loop(fast);
        lua.set_print(|_| {});
        lua.exec(setup, "=setup").unwrap();
        let f = lua.load(format!("while true do {body}\nend"), "=body").unwrap();
        Probe { lua, f }
    }

    /// Thread CPU nanoseconds per unit of fuel over one run of `fuel` units.
    fn ns_per_fuel(&mut self, fuel: u64) -> f64 {
        self.lua.set_limits(self.lua.limits().fuel(fuel));
        let start = remedy_lua_oracle::thread_cpu();
        let r = self.lua.call(self.f.value(), &[]);
        let t = remedy_lua_oracle::thread_cpu().saturating_sub(start) as f64;
        assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "ended with {r:?}");
        t / fuel as f64
    }

    /// The median of three runs (timing on a shared machine is noisy).
    fn median(&mut self, fuel: u64) -> f64 {
        let mut v = [self.ns_per_fuel(fuel), self.ns_per_fuel(fuel), self.ns_per_fuel(fuel)];
        v.sort_by(f64::total_cmp);
        v[1]
    }
}

/// `body` after `setup` costs at most `bound` times `control` (by default plain code) per
/// unit of fuel, with the fast loop on and off.
fn assert_ratio(setup: &str, body: &str, control: Option<(&str, &str)>, fuel: u64, bound: f64) {
    let (cs, cb) = control.unwrap_or(("x = 0", "x = x + 1"));
    for fast in [true, false] {
        let mut probe = Probe::new(setup, body, fast);
        let mut plain = Probe::new(cs, cb, fast);
        let mut ratio = f64::MAX;
        // Up to three tries: a single slow measurement on a loaded machine is not a failure.
        for _ in 0..3 {
            let base = plain.median(fuel);
            ratio = ratio.min(probe.median(fuel) / base);
            if ratio <= bound {
                break;
            }
        }
        assert!(
            ratio <= bound,
            "fast {fast}: {body}: {ratio:.1}x the control per unit of fuel (bound {bound}x)"
        );
    }
}

/// S5B-WALK-1 (critical): the collector walked thread stacks counting only the objects in
/// them. A coroutine parked with a million integer arguments, or a running vararg frame of
/// a million, was walked whole twice a cycle for a unit or two of work: 470x plain code per
/// unit of fuel with `collectgarbage('step')` in a loop, 100-200x at a pause of 0, growing
/// with the stack (so with the memory limit). Prototype constants likewise.
#[test]
fn s5b_walk_1_thread_stacks_are_counted() {
    let parked =
        "local function f(...) coroutine.yield() end co = coroutine.wrap(f) co(table.unpack({}, 1, 1000000))";
    assert_ratio(parked, "collectgarbage('step')", None, 300_000, 12.0);
    assert_ratio(&format!("collectgarbage('setpause', 0) {parked}"), "local x = {}", None, 300_000, 12.0);
    assert_ratio(
        "collectgarbage('setpause', 0)",
        "local function f(...) local t while true do t = {} end end f(table.unpack({}, 1, 1000000))",
        None,
        1_000_000,
        12.0,
    );
}

/// S5B-WALK-2 (critical, grows with the memory limit): after each sweep, `Slab::trim`
/// walked the free slots at the end of the slab one by one, and gave none back while they
/// were under a quarter of it. 2 million tables, the last live one at 75%: half a million
/// free slots walked every cycle for no work, 4.5x the same loop without the gap at a
/// pause of 0 (more under a larger limit).
#[test]
fn s5b_walk_2_free_slots_at_the_end_are_not_walked() {
    let gap = "keep = {} local t = {} for i = 1, 2000000 do t[i] = {} end keep[1], keep[2] = t[1], t[1501000] \
               t = nil collectgarbage() collectgarbage() collectgarbage('setpause', 0)";
    let body = "local x = function() return x end";
    assert_ratio(gap, body, Some(("keep = {} collectgarbage('setpause', 0)", body)), 1_000_000, 2.5);
}

/// S5B-WALK-3 (critical): every table traversed scanned its metatable's whole `__mode`
/// string, free: 200 tables under a 4 MiB `__mode` ran at 110x plain code per unit of
/// fuel, and at a pause of 0 a run of 2 million units did not finish in two minutes. C Lua
/// reads a mode only from a short string (so such a table was weak here and not there).
#[test]
fn s5b_walk_3_long_mode_strings_are_not_scanned() {
    let setup = "mt = {__mode = string.rep('x', 4 << 20)} keep = {} for i = 1, 200 do keep[i] = setmetatable({}, mt) end";
    assert_ratio(setup, "local x = {}", None, 1_000_000, 12.0);
}

/// S5B-WALK-4 (critical): `xpcall`'s handler counts C levels (S5COR-5) by walking the
/// frames of every coroutine that resumed this one, which `charge_error` does not see: in
/// the innermost of 150 coroutines each 1,900 frames deep, each error walked 285,000
/// frames, 440x plain code per unit of fuel. (Both walks, the handler's and each resume's,
/// a unit per frame: a resume's walk was still a unit per 16 frames, and the test read up
/// to 9.6x alone as plain code got faster, past the bound under load.)
#[test]
fn s5b_walk_4_resume_chain_walks_are_charged() {
    let setup = "function h(e) return e end \
                 function deep(n, k) if n > 0 then return (deep(n - 1, k)) end return k() end \
                 function nest(level) if level == 0 then while true do xpcall(error, h, 'x') end end \
                   local co = coroutine.wrap(function() return deep(1900, function() return nest(level - 1) end) end) \
                   return co() end";
    assert_ratio(setup, "nest(80)", None, 2_500_000, 12.0);
}

/// The shapes above give C Lua's results, the same with the fast loop on and off (results,
/// fuel and memory), and on a 64 KiB native stack with a whole collection at every
/// allocation.
#[test]
fn s5b_walk_shapes_match_c_lua_on_a_small_stack() {
    let programs = [
        // Parked and running vararg frames, collected while they hold their values.
        "local function f(...) coroutine.yield(select('#', ...)) return select(2000, ...) end \
         local co = coroutine.wrap(f) local n = co(table.unpack({}, 1, 3000)) collectgarbage() collectgarbage('step') \
         local function g(...) collectgarbage() return select('#', ...) end \
         return n, co(), g(table.unpack({}, 1, 3000))",
        // Prototype constants survive collection.
        "local p = {} for i = 1, 300 do p[i] = i .. '.5' end local f = load('return {' .. table.concat(p, ',') .. '}') \
         p = nil collectgarbage() local t = f() return #t, t[1], t[300]",
        // A slab with a gap at its end, trimmed or not, keeps its live objects.
        "local keep = {} local t = {} for i = 1, 3000 do t[i] = {i} end keep[1], keep[2] = t[1], t[2300] t = nil \
         collectgarbage() collectgarbage() local f = {} for i = 1, 500 do f[i] = function() return i end end \
         collectgarbage() return keep[1][1], keep[2][1], f[500]()",
        // `__mode` is read from short strings only, up to a zero byte (C Lua's
        // `ttisshrstring` and `strchr`): 40 bytes is weak, 41 and 5,000 are not, nor is
        // "\0k"; "v\0k" is weak values only.
        "local function count(t) local c = 0 for _ in pairs(t) do c = c + 1 end return c end \
         local modes = {string.rep('x', 39) .. 'k', string.rep('x', 40) .. 'k', string.rep('x', 5000) .. 'k', '\\0k', 'k', 'v\\0k'} \
         local ws = {} for j, m in ipairs(modes) do ws[j] = setmetatable({}, {__mode = m}) end \
         for i = 1, 10 do for j = 1, #ws do ws[j][{}] = {} end end collectgarbage() collectgarbage() \
         local out = {} for j = 1, #ws do out[j] = count(ws[j]) end return table.concat(out, ' ')",
        // Handler errors in nested coroutines count C levels as C Lua does.
        "local function h(e) return e end local function deep(n, k) if n > 0 then return (deep(n - 1, k)) end return k() end \
         local function nest(level) if level == 0 then return xpcall(error, h, 'x') end \
           return coroutine.wrap(function() return deep(50, function() return nest(level - 1) end) end)() end \
         local n = 0 local function loop(e) n = n + 1 error(e) end \
         local ok, e = coroutine.wrap(function() return xpcall(error, loop, 'y') end)() \
         return nest(5), n",
    ];
    for p in programs {
        let theirs = super::common::oracle(p, 1 << 30);
        // Fast loop and full path agree on results, fuel and memory.
        assert_eq!(super::common::remedy(p, 1 << 30), theirs, "{p}");
        for fast in [true, false] {
            let ours = super::common::small_stack(move || {
                let mut lua = Lua::new(Limits::default().hash_seed(super::common::TEST_SEED), Libs::ALL);
                lua.set_fast_loop(fast);
                lua.set_gc_stress(true);
                match lua.exec(super::common::wrap(p), "=wrapper") {
                    Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
                    Err(e) => Err(e.to_string()),
                }
            });
            assert_eq!(ours, theirs, "fast {fast}: {p}");
        }
    }
}

/// S5B-WALK-5: a closure's upvalues are paid for (a unit per 8 copied, 2 more per 8 made),
/// identically by the fast loop and the full path. Building a 250-upvalue closure was one
/// unit for up to 255 upvalue searches and copies (4x plain code per unit when repeated).
#[test]
fn s5b_walk_5_closure_upvalues_are_charged() {
    let locs: Vec<String> = (0..190).map(|i| format!("local a{i} = {i}")).collect();
    let refs: Vec<String> = (0..190).map(|i| format!("a{i}")).collect();
    let make = |closures: usize| {
        format!(
            "local function mk() {} local f for i = 1, {closures} do f = function() return {} end end return f end \
             local f = mk() return select('#', f()), (select(190, f()))",
            locs.join(" "),
            refs.join(", ")
        )
    };
    let fuel_of = |src: &str, fast: bool| {
        let mut lua = remedy_lua::Lua::new(
            remedy_lua::Limits::default().hash_seed(super::common::TEST_SEED),
            remedy_lua::Libs::ALL,
        );
        lua.set_fast_loop(fast);
        let f = lua.load(src, "=t").unwrap();
        let out: Vec<String> = lua.call(f.value(), &[]).unwrap().iter().map(|v| format!("{v:?}")).collect();
        (out, lua.limits().fuel - lua.fuel_left())
    };
    let (one, ten) = (make(1), make(11));
    for fast in [true, false] {
        let (r1, f1) = fuel_of(&one, fast);
        let (r10, f10) = fuel_of(&ten, fast);
        assert_eq!(r1, r10);
        // Ten more closures: their loop and collector work alone are about 306 units
        // (measured without the charge); the upvalues add 190 / 8 = 23 each, about 536.
        assert!(f10 - f1 >= 450, "fast {fast}: {f1} -> {f10}");
    }
    assert_eq!(fuel_of(&ten, true), fuel_of(&ten, false));
    let ours = super::common::remedy(&ten, 100_000_000);
    assert_eq!(ours, super::common::oracle(&ten, 100_000_000));
}
