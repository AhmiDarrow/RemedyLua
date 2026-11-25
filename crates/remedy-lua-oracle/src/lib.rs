//! The reference C Lua 5.4, for RemedyLua's tests only: run a chunk, get its first result
//! as a string or its error message.

#![allow(unsafe_code, missing_docs)]

/// Runs `src` (chunk name `=t`) in a fresh state with all standard libraries, stopping it
/// after about `max_steps` VM instructions. `Ok(first result converted with tostring)` or
/// `Err(error message)`.
///
/// The state may hold at most [`MEMORY_CAP`] bytes (C Lua has no limit of its own). If it
/// ever reaches the cap, the result is `Err(`[`MEMORY_LIMIT`]`)` whatever the program did
/// after (a `pcall` may have caught C Lua's "not enough memory"): like the step limit, the
/// oracle then has no verdict on the program.
pub fn run(src: &str, max_steps: u64) -> Result<String, String> {
    imp::run(src, max_steps, MEMORY_CAP)
}

/// [`run`] with a memory ceiling of `cap` bytes instead of [`MEMORY_CAP`].
pub fn run_capped(src: &str, max_steps: u64, cap: i64) -> Result<String, String> {
    imp::run(src, max_steps, cap)
}

/// [`run`], also stopped ("step limit") once it has run for `wall`: C Lua can spend long
/// stretches between instructions, out of the step limit's reach (a vararg tail call
/// copying a growing argument list, long-runs LR-30).
pub fn run_within(src: &str, max_steps: u64, wall: std::time::Duration) -> Result<String, String> {
    imp::DEADLINE.with(|d| d.set(Some(std::time::Instant::now() + wall)));
    let r = imp::run(src, max_steps, MEMORY_CAP);
    imp::DEADLINE.with(|d| d.set(None));
    r
}

/// The memory ceiling of [`run`]'s state: 2 GiB, four times the 512 MiB the differential
/// fuzzers give RemedyLua, so RemedyLua's own limit is nearly always reached first.
pub const MEMORY_CAP: i64 = 2 << 30;

/// `run`'s error when the state reached [`MEMORY_CAP`].
pub const MEMORY_LIMIT: &str = "oracle memory limit";

/// CPU time the calling thread has run so far, in nanoseconds (on Windows, its cycle count
/// scaled by the measured cycle rate). Only differences mean anything. For timing tests:
/// unlike wall time, it does not grow while other processes have the CPU.
pub fn thread_cpu() -> u64 {
    // SAFETY: a C function without arguments that only reads this thread's own counter.
    unsafe { imp::oracle_thread_cpu() }
}

/// The process's resident memory now and at its peak, in bytes, on Windows (zeros
/// elsewhere, where tests read /proc).
pub fn process_memory() -> (u64, u64) {
    let (mut now, mut peak) = (0, 0);
    // SAFETY: a C function that only writes the two counters it is given.
    unsafe { imp::oracle_process_memory(&mut now, &mut peak) };
    (now, peak)
}

/// A state kept across runs (for benchmarks): load once, call many times.
pub struct State(imp::State);

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    /// A fresh state with the standard libraries (panics if C Lua cannot allocate one).
    pub fn new() -> State {
        State(imp::State::new().expect("C Lua could not make a state"))
    }
    /// Runs `src` in this state.
    pub fn exec(&mut self, src: &str) -> Result<String, String> {
        self.0.exec(src)
    }
}

mod imp {
    use std::ffi::{c_char, c_int, c_void, CString};

    #[repr(C)]
    pub struct LuaState {
        _p: [u8; 0],
    }
    type L = *mut LuaState;
    type Hook = unsafe extern "C" fn(L, *mut c_void);
    unsafe extern "C" {
        fn oracle_init();
        fn luaL_newstate() -> L;
        fn luaL_openlibs(l: L);
        fn oracle_cap(l: L, cap: i64) -> c_int;
        fn oracle_cap_hit(l: L) -> c_int;
        fn oracle_close(l: L);
        fn luaL_loadbufferx(l: L, b: *const c_char, sz: usize, name: *const c_char, mode: *const c_char) -> c_int;
        fn lua_pcallk(l: L, na: c_int, nr: c_int, ef: c_int, ctx: isize, k: *const c_void) -> c_int;
        fn luaL_tolstring(l: L, idx: c_int, len: *mut usize) -> *const c_char;
        fn lua_settop(l: L, idx: c_int);
        fn lua_gettop(l: L) -> c_int;
        fn lua_sethook(l: L, f: Option<Hook>, mask: c_int, count: c_int);
        fn lua_error(l: L) -> c_int;
        fn lua_pushstring(l: L, s: *const c_char) -> *const c_char;
        pub(super) fn oracle_thread_cpu() -> u64;
        pub(super) fn oracle_process_memory(now: *mut u64, peak: *mut u64);
    }

    /// The hook's period. Every coroutine inherits the hook (`lua_newthread`) with a fresh
    /// count, so a count as long as the whole budget never fired in a program that spreads
    /// its work over many coroutines, each shorter than it; C Lua then ran unbounded (a
    /// fuzzed chain of `coroutine.wrap` calls kept a driver's threads spinning long after
    /// their timeouts). Now each period is taken from one budget for the whole run, and a
    /// new coroutine's count starts at its first instruction (`build.rs`), so each
    /// coroutine takes at least one period.
    const PERIOD: u64 = 1000;

    std::thread_local! {
        /// Periods left in the running `exec_limited` (on this thread; every coroutine of
        /// the state runs on it).
        static LEFT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        /// `run_within`'s deadline, checked each period.
        pub(super) static DEADLINE: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
    }

    unsafe extern "C" fn hook(l: L, _: *mut c_void) {
        let left = LEFT.with(|c| {
            let v = c.get().saturating_sub(1);
            c.set(v);
            v
        });
        if left > 0 && DEADLINE.with(|d| d.get().is_none_or(|t| std::time::Instant::now() < t)) {
            return;
        }
        // SAFETY: called by Lua with a live state; raising from a count hook is allowed.
        // (Once the budget is spent, every later period raises again, in any coroutine.)
        unsafe {
            lua_pushstring(l, c"step limit".as_ptr());
            lua_error(l);
        }
    }

    pub struct State(L);

    impl State {
        pub fn new() -> Option<State> {
            static INIT: std::sync::Once = std::sync::Once::new();
            // SAFETY: a C function without arguments, run once before any state exists.
            INIT.call_once(|| unsafe { oracle_init() });
            // SAFETY: plain state creation.
            let l = unsafe { luaL_newstate() };
            if l.is_null() {
                return None;
            }
            // SAFETY: live state.
            unsafe { luaL_openlibs(l) };
            Some(State(l))
        }

        fn exec_limited(&mut self, src: &str, steps: Option<u64>) -> Result<String, String> {
            let l = self.0;
            let name = c"=t";
            let mode = c"t";
            // SAFETY: live state; buffers outlive the calls; the stack is reset after.
            unsafe {
                lua_settop(l, 0);
                if let Some(n) = steps {
                    let period = n.clamp(1, PERIOD);
                    LEFT.with(|c| c.set(n / period));
                    lua_sethook(l, Some(hook), 1 << 3, period as c_int);
                }
                let rc = luaL_loadbufferx(l, src.as_ptr().cast(), src.len(), name.as_ptr(), mode.as_ptr());
                let rc = if rc == 0 { lua_pcallk(l, 0, 1, 0, 0, std::ptr::null()) } else { rc };
                lua_sethook(l, None, 0, 0);
                let mut len = 0usize;
                let top = lua_gettop(l);
                let out = if top >= 1 {
                    let p = luaL_tolstring(l, top, &mut len);
                    String::from_utf8_lossy(std::slice::from_raw_parts(p.cast::<u8>(), len)).into_owned()
                } else {
                    String::new()
                };
                lua_settop(l, 0);
                if rc == 0 { Ok(out) } else { Err(out) }
            }
        }

        pub fn exec(&mut self, src: &str) -> Result<String, String> {
            self.exec_limited(src, None)
        }
    }

    impl Drop for State {
        fn drop(&mut self) {
            // SAFETY: closed once (frees the cap's counter too, if `run` installed one).
            unsafe { oracle_close(self.0) };
        }
    }

    pub fn run(src: &str, max_steps: u64, cap: i64) -> Result<String, String> {
        let Some(mut s) = State::new() else { return Err("no state".into()) };
        let _ = CString::new("x");
        // SAFETY: live state, made by luaL_newstate as `oracle_cap` requires.
        if unsafe { oracle_cap(s.0, cap) } == 0 {
            return Err("no state".into());
        }
        let r = s.exec_limited(src, Some(max_steps));
        // SAFETY: live state.
        if unsafe { oracle_cap_hit(s.0) } != 0 {
            return Err(super::MEMORY_LIMIT.into());
        }
        r
    }
}
