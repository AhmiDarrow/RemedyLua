//! Shared by the test suites: run a program in RemedyLua and in the reference C Lua and
//! describe the outcome the same way, so the two can be compared.
#![allow(dead_code, clippy::unwrap_used, clippy::indexing_slicing)]

use remedy_lua::{Libs, Limits, Lua, Value};

/// Turns any value into canonical text: numbers keep their subtype, tables print sorted.
pub const SERIALIZE: &str = r#"
local function ser(v, seen)
  local t = type(v)
  if t == "string" then return string.format("%q", v)
  elseif t == "number" then
    if math.type(v) == "integer" then return tostring(v) end
    if v ~= v then return "nan" end
    if v == math.huge then return "inf" end
    if v == -math.huge then return "-inf" end
    return string.format("%.17g", v) .. "f"
  elseif t == "table" then
    seen = seen or {}
    if seen[v] then return "<cycle>" end
    seen[v] = true
    local keys = {}
    for k in next, v do keys[#keys + 1] = k end
    table.sort(keys, function(a, b)
      local ta, tb = type(a), type(b)
      if ta ~= tb then return ta < tb end
      if ta == "number" or ta == "string" then return a < b end
      if ta == "boolean" then return (not a) and b end
      return false
    end)
    local parts = {}
    for _, k in ipairs(keys) do parts[#parts + 1] = "[" .. ser(k, seen) .. "]=" .. ser(v[k], seen) end
    seen[v] = nil
    return "{" .. table.concat(parts, ",") .. "}"
  elseif t == "boolean" or t == "nil" then return tostring(v)
  else return t end
end
"#;

/// Wraps a program: loads it as chunk `t`, runs it protected, serializes all its results
/// (or `error: <message>`).
pub fn wrap(program: &str) -> String {
    let mut level = 1;
    while program.contains(&format!("]{}]", "=".repeat(level))) {
        level += 1;
    }
    let eq = "=".repeat(level);
    format!(
        "{SERIALIZE}\nlocal f, err = load([{eq}[{program}]{eq}], '=t')\n\
         if not f then return 'syntax error' end\n\
         local r = table.pack(pcall(f))\n\
         if not r[1] then return 'error: ' .. ser(r[2]) end\n\
         local out = {{}}\n\
         for i = 2, r.n do out[#out + 1] = ser(r[i]) end\n\
         return table.concat(out, ' ')"
    )
}

/// The hash seed of every state whose fuel or traversal order a test compares: with it
/// fixed, runs replay exactly (`Limits::hash_seed`).
pub const TEST_SEED: u64 = 0x5eed_7e57;

/// The outcome in RemedyLua, with the fuel it used and the memory it holds after.
pub fn remedy_measured(program: &str, fuel: u64, fast: bool) -> (Result<String, String>, u64, usize) {
    remedy_measured_libs(program, fuel, fast, Libs::ALL)
}

/// The libraries C Lua's `luaL_openlibs` opens that RemedyLua has, the opt-in ones
/// included (`Libs::DEBUG_INFO`, `Libs::REQUIRE`).
pub const ALL_OPT_IN: Libs = Libs::ALL.union(Libs::DEBUG_INFO).union(Libs::REQUIRE);

/// [`remedy_measured`] with the libraries `libs`.
pub fn remedy_measured_libs(program: &str, fuel: u64, fast: bool, libs: Libs) -> (Result<String, String>, u64, usize) {
    let mut lua = Lua::new(Limits::default().fuel(fuel).memory(256 << 20).hash_seed(TEST_SEED), libs);
    lua.set_fast_loop(fast);
    let r = match lua.exec(wrap(program), "=wrapper") {
        Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
        Err(e) => Err(e.to_string()),
    };
    (r, fuel.saturating_sub(lua.fuel_left()), lua.memory_used())
}

/// [`remedy`] with the libraries `libs`.
pub fn remedy_libs(program: &str, fuel: u64, libs: Libs) -> Result<String, String> {
    let fast = remedy_measured_libs(program, fuel, true, libs);
    let slow = remedy_measured_libs(program, fuel, false, libs);
    assert_eq!(fast, slow, "fast loop and full path differ (result, fuel, memory) for:\n{program}");
    fast.0
}

/// The outcome in RemedyLua. The program runs twice, with the fast loop and without it:
/// results, fuel used and memory held must be identical (the fast loop only shortcuts).
pub fn remedy(program: &str, fuel: u64) -> Result<String, String> {
    let fast = remedy_measured(program, fuel, true);
    let slow = remedy_measured(program, fuel, false);
    assert_eq!(fast, slow, "fast loop and full path differ (result, fuel, memory) for:\n{program}");
    fast.0
}

/// The outcome in C Lua (the oracle always runs: its crate does not build without the C
/// sources).
pub fn oracle(program: &str, steps: u64) -> Result<String, String> {
    remedy_lua_oracle::run(&wrap(program), steps)
}

/// Runs plain source and returns its results as display strings.
pub fn eval(src: &str) -> Result<Vec<String>, String> {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let v = lua.exec(src, "=t").map_err(|e| e.to_string())?;
    Ok(v.iter().map(|x| show(&lua, *x)).collect())
}

pub fn show(lua: &Lua, v: Value) -> String {
    match v {
        Value::Nil => "nil".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Num(f) => format!("{f:?}"),
        Value::Str(_) => lua.to_string_lossy(v).unwrap_or_default(),
        other => other.type_name().into(),
    }
}

/// The one class of difference from C Lua the oracle comparisons accept: C Lua stops with
/// an error from one of its implementation limits (a fixed register, local, stack,
/// nesting or result-count limit) while RemedyLua, a strict superset whose only limits
/// are the host's budgets, runs the program. These are C Lua's messages for those limits.
pub const C_LIMIT_ERRORS: &[&str] = &[
    "too many results to unpack",
    "string slice too long",
    "too many local variables",
    "function or expression needs too many registers",
    "too many upvalues",
    "too many registers",
    "chunk has too many syntax levels",
    "C stack overflow",
    "control structure too long",
    "stack overflow",
    "too many arguments",
    "too many results",
    "too many captures",
];

/// Whether C Lua's outcome `theirs` is an implementation-limit error and RemedyLua's
/// `ours` is not an error: the allowed difference.
pub fn c_limit_only(ours: &str, theirs: &str) -> bool {
    let is_err = |s: &str| s.starts_with("error:") || s.starts_with("syntax error") || s.contains("error");
    C_LIMIT_ERRORS.iter().any(|m| theirs.contains(m)) && !is_err(ours)
}

/// `c_limit_only` on whole outcomes.
pub fn c_limit_only_outcome(ours: &Result<String, String>, theirs: &Result<String, String>) -> bool {
    let text = |r: &Result<String, String>| match r {
        Ok(s) | Err(s) => s.clone(),
    };
    ours.is_ok() && c_limit_only(&text(ours), &text(theirs))
}

/// The stack of the small-stack threads fuzzers and stress modes run on (`small_stack`).
pub const SMALL_STACK: usize = 64 << 10;
/// The native stack library callbacks may use on them (`remedy_lua::set_test_stack_cap`).
pub const SMALL_STACK_CAP: usize = 24 << 10;

/// Runs `f` on a thread with a 64 KiB stack, with every state made there capped to
/// `SMALL_STACK_CAP` of native stack for library callbacks. Recursion whose depth follows
/// a script's data (SEC4-9: an ephemeron chain marked recursively) then overflows here,
/// in a test, instead of in production with its larger stacks. A panic in `f` is passed on.
pub fn small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let h = std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(move || {
            remedy_lua::set_test_stack_cap(Some(SMALL_STACK_CAP));
            f()
        })
        .unwrap();
    match h.join() {
        Ok(v) => v,
        Err(p) => std::panic::resume_unwind(p),
    }
}

/// Threads `with_deadline` gave up on, kept joinable: never detached (see there).
static STRAYS: std::sync::Mutex<Vec<std::thread::JoinHandle<()>>> = std::sync::Mutex::new(Vec::new());

/// Runs `f` on its own thread with a `stack`-byte stack: its result when it finishes
/// within `timeout`, `None` when it does not (the thread runs on).
///
/// No thread is ever detached (its `JoinHandle` dropped): glibc's `pthread_detach` reads
/// the thread's descriptor just after marking it detached, and a thread exiting at that
/// moment frees that descriptor itself. With a stack bigger than glibc's stack cache
/// (40 MiB; the oracle runs on 64 MiB) the memory is unmapped and the read segfaults:
/// `fuzz_mutated` seed 900001 died so once, in `libc`'s `pthread_detach`, not
/// reproducible (FX2-2). A thread that finished is joined; one that ran over is kept in
/// `STRAYS` and joined at a later call once it has finished.
pub fn with_deadline<T: Send + 'static>(
    stack: usize,
    timeout: std::time::Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    let h = std::thread::Builder::new()
        .stack_size(stack)
        .spawn(move || {
            let _ = tx.send(f());
        })
        .unwrap();
    match rx.recv_timeout(timeout) {
        Ok(v) => {
            let _ = h.join();
            Some(v)
        }
        Err(_) => {
            let mut strays = STRAYS.lock().unwrap_or_else(|e| e.into_inner());
            let (done, running): (Vec<_>, Vec<_>) =
                strays.drain(..).partition(std::thread::JoinHandle::is_finished);
            for d in done {
                let _ = d.join();
            }
            *strays = running;
            strays.push(h);
            None
        }
    }
}

/// How many threads `with_deadline` holds that had not finished when last looked at.
pub fn strays() -> usize {
    STRAYS.lock().unwrap_or_else(|e| e.into_inner()).len()
}

/// A measurement holds this for writing while it times, so it runs alone among the tests
/// that hold it for reading: those that flood memory bandwidth or caches for seconds
/// (resident-memory children, big heaps, deep programs, long stress loops). Shut out only
/// of the first and of each other, a measurement next to the rest ran 4 to 6 times its
/// own control on a loaded run (S6GC-2e-R).
pub static MEMORY_HEAVY: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// `MEMORY_HEAVY` for a measurement (a panicked holder does not matter). Nested on one
/// thread (a measuring test calling `ns_per_unit`), only the outermost takes the lock.
pub fn measuring() -> Measuring {
    thread_local!(static DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) });
    let lock = (DEPTH.get() == 0).then(|| MEMORY_HEAVY.write().unwrap_or_else(std::sync::PoisonError::into_inner));
    DEPTH.set(DEPTH.get() + 1);
    Measuring { _lock: lock, depth: &DEPTH }
}

/// A held `measuring()`.
pub struct Measuring {
    _lock: Option<std::sync::RwLockWriteGuard<'static, ()>>,
    depth: &'static std::thread::LocalKey<std::cell::Cell<usize>>,
}

impl Drop for Measuring {
    fn drop(&mut self) {
        self.depth.set(self.depth.get() - 1);
    }
}

/// `MEMORY_HEAVY` for a heavy test; heavy tests run side by side.
pub fn memory_heavy() -> std::sync::RwLockReadGuard<'static, ()> {
    MEMORY_HEAVY.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Files in memory for `io` tests (`Libs::IO`): an opener with C's `fopen` modes. Clones
/// share the files. `limited(n)` fails writes past `n` bytes in all, as a full disk does.
#[derive(Clone)]
pub struct MemoryFiles {
    files: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Bytes>>>,
    limit: usize,
}

type Bytes = std::sync::Arc<std::sync::Mutex<Vec<u8>>>;

impl MemoryFiles {
    pub fn new() -> MemoryFiles {
        MemoryFiles::limited(usize::MAX)
    }
    pub fn limited(limit: usize) -> MemoryFiles {
        MemoryFiles { files: Default::default(), limit }
    }
    pub fn insert(&self, name: &str, contents: impl Into<Vec<u8>>) {
        self.files.lock().unwrap().insert(name.to_owned(), std::sync::Arc::new(std::sync::Mutex::new(contents.into())));
    }
    pub fn get(&self, name: &str) -> Option<Vec<u8>> {
        Some(self.files.lock().unwrap().get(name)?.lock().unwrap().clone())
    }
    pub fn total(&self) -> usize {
        self.files.lock().unwrap().values().map(|f| f.lock().unwrap().len()).sum()
    }
    pub fn open(&self, name: &str, mode: &str) -> Result<Box<dyn remedy_lua::Stream>, remedy_lua::IoError> {
        let not_found = || remedy_lua::IoError::new("No such file or directory", 2);
        let kind = mode.as_bytes()[0];
        let mut files = self.files.lock().unwrap();
        let data = if kind == b'r' {
            files.get(name).ok_or_else(not_found)?.clone()
        } else if name.is_empty() {
            return Err(not_found());
        } else {
            files.entry(name.to_owned()).or_default().clone()
        };
        if kind == b'w' {
            data.lock().unwrap().clear();
        }
        let plus = mode.contains('+');
        Ok(Box::new(MemoryStream { data, pos: 0, read: kind == b'r' || plus, write: kind != b'r' || plus, append: kind == b'a', files: self.clone() }))
    }
}

struct MemoryStream {
    data: Bytes,
    pos: usize,
    read: bool,
    write: bool,
    append: bool,
    files: MemoryFiles,
}

impl remedy_lua::Stream for MemoryStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, remedy_lua::IoError> {
        if !self.read {
            return Err(remedy_lua::IoError::new("Bad file descriptor", 9));
        }
        let data = self.data.lock().unwrap();
        let n = data.len().saturating_sub(self.pos).min(buf.len());
        buf[..n].copy_from_slice(&data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
    fn write(&mut self, bytes: &[u8]) -> Result<(), remedy_lua::IoError> {
        if !self.write {
            return Err(remedy_lua::IoError::new("Bad file descriptor", 9));
        }
        let len = self.data.lock().unwrap().len();
        if self.append {
            self.pos = len;
        }
        let grow = (self.pos + bytes.len()).saturating_sub(len);
        if grow > 0 && self.files.total().saturating_add(grow) > self.files.limit {
            return Err(remedy_lua::IoError::new("No space left on device", 28));
        }
        let mut data = self.data.lock().unwrap();
        if data.len() < self.pos + bytes.len() {
            data.resize(self.pos + bytes.len(), 0);
        }
        data[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }
    fn seek(&mut self, pos: std::io::SeekFrom) -> Result<u64, remedy_lua::IoError> {
        let len = self.data.lock().unwrap().len() as i64;
        let to = match pos {
            std::io::SeekFrom::Start(n) => n as i64,
            std::io::SeekFrom::Current(d) => self.pos as i64 + d,
            std::io::SeekFrom::End(d) => len + d,
        };
        if to < 0 {
            return Err(remedy_lua::IoError::new("Invalid argument", 22));
        }
        self.pos = to as usize;
        Ok(self.pos as u64)
    }
}

/// This thread's CPU time and page faults (`/proc/thread-self/stat`; zeros where there is
/// none), for the collector's trace in diagnostic runs (SEC7-G1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ThreadStat {
    /// User and system time, in clock ticks.
    pub utime: u64,
    pub stime: u64,
    /// Minor and major page faults.
    pub minflt: u64,
    pub majflt: u64,
}

pub fn thread_stat() -> ThreadStat {
    let Ok(t) = std::fs::read_to_string("/proc/thread-self/stat") else { return ThreadStat::default() };
    // After the command's closing parenthesis: state, ppid, pgrp, session, tty, tpgid,
    // flags, minflt, cminflt, majflt, cmajflt, utime, stime.
    let f: Vec<u64> = t.rsplit(')').next().unwrap_or("").split_whitespace().skip(1).map(|x| x.parse().unwrap_or(0)).collect();
    let at = |i: usize| f.get(i - 1).copied().unwrap_or(0);
    ThreadStat { minflt: at(7), majflt: at(9), utime: at(11), stime: at(12) }
}

/// The trace's slowest record of `op` and the records around it, as text (for a failing
/// assert's message).
pub fn slowest_around(recs: &[remedy_lua::TraceRecord], op: remedy_lua::TraceOp, around: usize) -> String {
    let Some((i, _)) = recs.iter().enumerate().filter(|(_, r)| r.op == op).max_by_key(|(_, r)| r.wall()) else {
        return "no records".into();
    };
    let mut out = String::new();
    for (j, r) in recs.iter().enumerate().take(i + around + 1).skip(i.saturating_sub(around)) {
        out += &format!(
            "{} call {} poll {} step {} {:?}: {} us, {} slots, {} bytes, {} fuel\n",
            if j == i { ">>" } else { "  " },
            r.call,
            r.poll,
            r.step,
            r.op,
            r.wall().as_micros(),
            r.slots,
            r.bytes,
            r.fuel
        );
    }
    out
}
