//! Sweep 6, error paths of the machine (`vm.rs`): regression tests for what it found. Each
//! test is named `s6_<id>_<what>` after its finding, and runs with the fast loop on and off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common::{oracle, remedy, small_stack};
use super::sweep5b_open::{baseline, ns_per_unit};

/// `body` in a loop, best of three, against plain code per unit of fuel (on a 64 KiB stack).
fn ratio(setup: &'static str, body: &'static str, fuel: u64, fast: bool) -> f64 {
    small_stack(move || {
        let base = baseline(fast);
        let r = (0..3).map(|_| ns_per_unit(setup, body, fuel, fast) / base).fold(f64::MAX, f64::min);
        eprintln!("RATIO {body} fast {fast}: {r:.2}");
        r
    })
}

/// S6-1 (critical): an error's position (`where_`) kept one interval per `__close` running
/// while an error unwinds (`Cont::ErrClosing`) and scanned them all for every frame, while
/// `charge_error` paid `(frames + natives) / 16`: an erroring `__close` that declares
/// another one recurses to the depth limit, each level's error scanning every level, at
/// 3.45 s per million units of fuel (about 175x plain code; 71x here on a 64 KiB stack;
/// cubic in the depth limit). The position is now walked from the top, only as far as the
/// level asked for: 0.7x (fast loop; 0.4x on the full path).
#[test]
fn s6_1_errors_under_err_closing_chains_are_paid() {
    let setup = "MT = {} MT.__close = function(a) local x <close> = setmetatable({}, MT) error('e') end \
                 F = function() local x <close> = setmetatable({}, MT) end";
    for fast in [true, false] {
        let r = ratio(setup, "pcall(F)", 300_000, fast);
        assert!(r < 10.0, "fast {fast}: a chain of erroring __close at {r:.1}x plain code per unit");
    }
    // The same error and positions as C Lua.
    let src = "local n = 0 local mt = {} mt.__close = function(a, e) n = n + 1 if n < 50 then \
               local x <close> = setmetatable({}, mt) end error('e' .. n, n % 3) end \
               local ok, e = pcall(function() local x <close> = setmetatable({}, mt) end) return ok, e, n";
    let want = oracle(src, 1 << 30);
    assert!(want.is_ok(), "{want:?}");
    assert_eq!(remedy(src, 1 << 30), want);
}

/// S6-2 (critical): a run of nested protected calls (`pcall(pcall, ..., f)`, one
/// `Cont::Nest` frame) was materialised level by level: an error's position took one
/// record per level, while `charge_error` counted the frame once, and each native called
/// at level `k` of the run pushed `k` entries for its levels (quadratic in the run, 6 s
/// to start 100,000 levels, where C Lua takes 0.04 s). With 100,000 levels, errors with a
/// deep level ran at 9 s per million units of fuel (156x plain code here). A run is one
/// step of the walk now, and one native entry with its count: 0.5x (0.3x on the full
/// path), and the run starts in 0.04 s. `debug.traceback` (which lists every level) was
/// charged for the run's frame once, 3,181 units for 100,000 levels: each level is now.
#[test]
fn s6_2_nested_protected_call_levels_are_paid() {
    let setup = "ARGS = {} for i = 1, 100000 do ARGS[i] = pcall end \
                 ARGS[#ARGS + 1] = function() while true do pcall(error, 'x', 1 << 40) end end";
    for fast in [true, false] {
        let r = ratio(setup, "pcall(table.unpack(ARGS))", 2_000_000, fast);
        assert!(r < 10.0, "fast {fast}: errors under 100,000 nested pcalls at {r:.1}x plain code per unit");
    }
    // Levels through a run of nested protected calls, as C Lua counts them.
    let src = "local out = {} for lv = 1, 7 do \
               local r = table.pack(pcall(pcall, pcall, pcall, function() error('x', lv) end)) \
               out[#out + 1] = tostring(r[#r]) end return table.concat(out, ',')";
    let want = oracle(src, 1 << 30);
    assert!(want.is_ok(), "{want:?}");
    assert_eq!(remedy(src, 1 << 30), want);
    // `debug.traceback` lists every level of the run (as C Lua does): paid for each, where
    // its charge counted the run's frame once.
    let traceback_fuel = |calls: usize, fast: bool| {
        let mut lua =
            remedy_lua::Lua::new(remedy_lua::Limits::default().fuel(1 << 40), remedy_lua::Libs::ALL);
        lua.open_libs(remedy_lua::Libs::DEBUG_INFO);
        lua.set_fast_loop(fast);
        let src = format!(
            "local args = {{}} for i = 1, 100000 do args[i] = pcall end \
             args[#args + 1] = function() for i = 1, {calls} do local s = debug.traceback() end end \
             assert(pcall(table.unpack(args)))"
        );
        lua.exec(&src, "=t").unwrap();
        (1u64 << 40) - lua.fuel_left()
    };
    for fast in [true, false] {
        let per = (traceback_fuel(10, fast) - traceback_fuel(0, fast)) / 10;
        assert!(per >= 40_000, "fast {fast}: a traceback through 100,000 nested pcalls charged {per}");
    }
}

/// S6-6 (low): a `__close` that is a callable table (`__call`) raising while an error
/// unwinds lost its position (`callable t:1: orig`; C Lua `t:1: callable t:1: orig`):
/// `err_closing` marked every frame above the protected call as gone, the running callee's
/// own frame too. It marks those below the `__close` call now.
#[test]
fn s6_6_a_callable_close_keeps_its_position() {
    for lv in ["'callable ' .. tostring(e)", "'two', 2", "'three', 3", "'four', 4"] {
        let src = format!(
            "local function f() local x <close> = setmetatable({{}}, {{__close = setmetatable({{}}, \
             {{__call = function(s, o, e) error({lv}) end}})}}) error('orig') end \
             local ok, e = pcall(f) return e"
        );
        let want = oracle(&src, 1 << 30);
        assert!(want.is_ok(), "{want:?}");
        assert_eq!(remedy(&src, 1 << 30), want, "{lv}");
    }
}

/// S6GC-H1 (high): every error that reached `on_error`'s end built a traceback (up to 22
/// frames, two `format!`s each), free, even when a native or `pcall` above caught it, and
/// an error value (`error({})`) was not charged at all: `pcall(string.gsub, 'a', 'a', f)`
/// with `f` raising a table, 30 frames deep, ran at 16x plain code per unit of fuel (8x
/// here); a suspended coroutine with an erroring `__close` at each of 20 levels, closed,
/// at 15-19x (8-12x here). The frames are now noted (a chunk, a line and where the
/// function starts) and the text is made only when the error reaches the host, as it was:
/// 1.7x both (1.0-1.2x on the full path).
#[test]
fn s6gc_h1_tracebacks_are_made_for_the_host_only() {
    let deep = "G = function() error({}) end \
                function D(n) if n == 0 then while true do pcall(string.gsub, 'a', 'a', G) end end return (D(n - 1)) end";
    let close = "EM = {__close = function() error({}) end} \
                 function MK() local co = coroutine.create(function() local function lvl(n) \
                 local x <close> = setmetatable({}, EM) if n > 0 then lvl(n - 1) else coroutine.yield() end end lvl(20) end) \
                 coroutine.resume(co) return co end";
    for fast in [true, false] {
        let r = ratio(deep, "D(30)", 1_000_000, fast);
        assert!(
            r < 5.0,
            "fast {fast}: errors caught by a native 30 frames deep at {r:.1}x plain code per unit"
        );
        let r = ratio(close, "coroutine.close(MK())", 1_000_000, fast);
        assert!(
            r < 5.0,
            "fast {fast}: closing a coroutine with 20 erroring __close at {r:.1}x plain code per unit"
        );
    }
    // The host sees the same traceback as before (the frames where the error was raised,
    // at most 22).
    let src = "local function a(n) if n == 0 then error('x') end a(n - 1) end\n\
               local function b() a(25) end\n\
               local ok = pcall(b)\n\
               local co = coroutine.wrap(function() error({}) end)\n\
               pcall(co)\n\
               b()";
    for fast in [true, false] {
        let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
        lua.set_fast_loop(fast);
        match lua.exec(src, "=t") {
            Err(remedy_lua::Error::Runtime { message, traceback }) => {
                assert_eq!(message, "t:1: x");
                let want = format!("stack traceback:{}", "\n\tt:1: in function <t:1>".repeat(22));
                assert_eq!(traceback, want, "fast {fast}");
            }
            r => panic!("{r:?}"),
        }
    }
}

/// S6-5 (low): inside a `__close` run by `coroutine.close(co)`, the closing thread said
/// `running` to `coroutine.status`, and `pcall(coroutine.close, closer)` "cannot close a
/// running coroutine"; C Lua says `normal` (and "cannot close a normal coroutine"):
/// `swap_in_for_close` never set it. Checked for a coroutine closing and for the main
/// thread.
#[test]
fn s6_cor_a_closing_thread_is_normal() {
    let src = "local out = {} \
               local function closing(me) \
                 local co = coroutine.create(function() \
                   local x <close> = setmetatable({}, {__close = function() \
                     out[#out + 1] = coroutine.status(me) \
                     out[#out + 1] = select(2, pcall(coroutine.close, me)) \
                     out[#out + 1] = coroutine.status(coroutine.running()) end}) \
                   coroutine.yield() end) \
                 coroutine.resume(co) local ok = coroutine.close(co) out[#out + 1] = tostring(ok) \
                 out[#out + 1] = coroutine.status(me) end \
               coroutine.wrap(function() closing(coroutine.running()) end)() \
               closing(coroutine.running()) \
               return table.concat(out, ' | ')";
    let want = oracle(src, 1 << 30);
    assert!(want.is_ok(), "{want:?}");
    assert_eq!(remedy(src, 1 << 30), want);
}
