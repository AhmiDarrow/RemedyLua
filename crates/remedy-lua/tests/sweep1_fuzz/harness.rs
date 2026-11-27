//! Runs one program in RemedyLua and in C Lua inside the same Lua-level wrapper, and
//! describes each outcome as comparable text.
//!
//! The wrapper is identical for both engines: it captures `print`, defines the helpers the
//! generated programs use (`__emit`, `__enter`, `__seg`, `__spairs`, `__tag`), makes
//! `pcall`/`xpcall`/`coroutine.resume`/`coroutine.close` re-raise the oracle's step-limit
//! error (otherwise a mutated `while true do pcall(f) end` would never stop in C Lua), and
//! caps `string.rep`/`table.move` sizes (C Lua has no memory limit and no hook inside C
//! functions). The program text is passed byte-exact as an escaped short string.
//!
//! Known non-bugs are taken out two ways, so a
//! run's findings are only real differences:
//! - made the same on both sides by the wrapper: `collectgarbage("count")` (each
//!   collector's heap size) is 0 and `collectgarbage("step")` false; `math.random` starts from a fixed seed
//!   (and `math.randomseed()` without arguments takes one), as C Lua's own seed is random;
//!   the libraries RemedyLua leaves out by design (`io`, `debug`, `package`, `require`,
//!   `dofile`, `loadfile`, `os.execute` and the like) are taken out of C Lua's globals too,
//!   so listings and calls of them compare;
//! - classified by `classify` when they still differ: `pairs`/`next` order (the program's
//!   output changes with RemedyLua's hash seed, and C Lua's output is RemedyLua's under
//!   another seed, or the same lines in another order), C Lua disagreeing with itself
//!   (clocks), and C Lua's own implementation limits
//!   (a limit error a `pcall`, `load` or resume saw before the first difference: the
//!   wrapper marks it in the output).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use remedy_lua::{Dialect, Error, Libs, LimitKind, Limits, Lua};

pub const ORACLE_STEPS: u64 = 4_000_000;
/// Fuel is counted differently from C Lua's hook steps; give RemedyLua far more, so a
/// fuel stop where C Lua finished means something.
pub const REMEDY_FUEL: u64 = 200_000_000;
pub const REMEDY_MEM: usize = 512 << 20;
const TIMEOUT: Duration = Duration::from_secs(60);
/// C Lua's wall-time cut, a step limit (inconclusive) well inside `TIMEOUT` (LR-30).
pub const ORACLE_WALL: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    Ok,
    Syntax,
    Runtime,
    Fuel,
    Memory,
    /// The wrapper itself failed (the program broke something the wrapper needs).
    Host,
    Panic,
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub class: Class,
    pub text: String,
}

const WRAP_HEAD: &str = r##"
local _pcall, _xpcall, _resume, _close, _type, _tostring, _fmt, _concat, _mtype, _next, _sort, _select, _sub, _find, _rep, _move, _error, _pack, _rawget, _load =
  pcall, xpcall, coroutine.resume, coroutine.close, type, tostring, string.format, table.concat, math.type, next, table.sort, select, string.sub, string.find, string.rep, table.move, error, table.pack, rawget, load
local IOF_NAME, IOF_RM
LEFT_OUT
do
  -- Heap sizes and whether a step finished a cycle are each collector's own: still run, the
  -- same answer in both.
  local cg = collectgarbage
  if cg then
    collectgarbage = function(o, ...)
      if o == "count" then cg(o, ...) return 0.0 end
      if o == "step" then cg(o, ...) return false end
      return cg(o, ...)
    end
  end
end
do
  local seed = math.randomseed
  math.randomseed = function(...) if _select("#", ...) == 0 then return seed(0x5eed) end return seed(...) end
  seed(0x5eed)
end
local OUT, NOUT = {}, 0
local function push(s)
  if NOUT < 3000 then NOUT = NOUT + 1; OUT[NOUT] = _sub(s, 1, 3000) end
end
local LIMITS = {LIMITS_LIST}
local function climit(e)
  if _type(e) ~= "string" then return end
  for i = 1, #LIMITS do if _find(e, LIMITS[i], 1, true) then push("\2c-limit") return end end
end
local function chk(ok, ...)
  if not ok then
    local e = ...
    if _type(e) == "string" and _find(e, "step limit", 1, true) then _error(e, 0) end
    climit(e)
  end
  return ok, ...
end
do
  local pcall, xpcall, resume, close = _pcall, _xpcall, _resume, _close
  -- "error in error handling" from C Lua's limit of 220 C levels (`luaE_checkcstack`),
  -- which a protected call reaches past its 200, is marked like the other limits: when
  -- no handler ran, or the handler was given C Lua's "C stack overflow" (its 200th level,
  -- then the handler's own calls to 220). A handler that keeps failing reaches it the same
  -- way in both engines, so both are marked there and the rest still compares.
  local function errerr(ok, e)
    if not ok and e == "error in error handling" then push("\2c-limit") end
  end
  _G.pcall = function(...)
    return (function(...) errerr(...) return chk(...) end)(pcall(...))
  end
  _G.xpcall = function(f, h, ...)
    if _type(h) ~= "function" then return chk(xpcall(f, h, ...)) end
    local ran, limited = false, false
    return (function(...) if not ran or limited then errerr(...) end return chk(...) end)(xpcall(f, function(e)
      ran = true
      if _type(e) == "string" and _find(e, "C stack overflow", 1, true) then limited = true end
      if _type(e) == "string" and _find(e, "step limit", 1, true) then return e end
      return h(e)
    end, ...))
  end
  coroutine.resume = function(...) return chk(resume(...)) end
  coroutine.close = function(...) return chk(close(...)) end
  load = function(...)
    local f, e = _load(...)
    if f == nil then
      -- (The oracle's step limit, raised in a reader, comes back as load's message.)
      if _type(e) == "string" and _find(e, "step limit", 1, true) then _error(e, 0) end
      climit(e)
    end
    return f, e
  end
end
do
  local rep, move = _rep, _move
  string.rep = function(...)
    local s, n, sep = ...
    if (_type(s) == "string" or _type(s) == "number") and _type(n) == "number" then
      local len = #_tostring(s) + 1
      if _type(sep) == "string" or _type(sep) == "number" then len = len + #_tostring(sep) end
      if n * len > 20000 then _error("sweep1: rep too large", 0) end
    end
    return (rep(...)) -- not a tail call: C Lua and RemedyLua place errors differently there (own finding)
  end
  table.move = function(...)
    local _, f, e = ...
    if _type(f) == "number" and _type(e) == "number" and (e * 1.0) - f > 100000 then _error("sweep1: move too large", 0) end
    return (move(...))
  end
end
local function keyless(a, b)
  local ta, tb = _type(a), _type(b)
  if ta ~= tb then return ta < tb end
  return a < b
end
local function sh(v, inner)
  local t = _type(v)
  if t == "number" then
    if v ~= v then return "nan" end
    if _mtype(v) == "integer" then return _tostring(v) end
    return _tostring(v) .. "~" .. _fmt("%.17g", v)
  elseif t == "string" then return _fmt("%q", v)
  elseif t == "table" then
    if inner then return "{..}" end
    local ks, n, other = {}, 0, 0
    for k in _next, v do
      local tk = _type(k)
      if (tk == "number" and k == k) or tk == "string" then n = n + 1; ks[n] = k else other = other + 1 end
    end
    _sort(ks, keyless)
    local parts = {}
    for i = 1, (n < 40 and n or 40) do parts[i] = sh(ks[i], true) .. "=" .. sh(_rawget(v, ks[i]), true) end
    return "{" .. _concat(parts, ",") .. "}" .. (other > 0 and ("+" .. other) or "") .. (n > 40 and "..." or "")
  elseif t == "nil" or t == "boolean" then return _tostring(v)
  else return t end
end
function __emit(...)
  local p = {}
  for i = 1, _select("#", ...) do p[i] = sh((_select(i, ...))) end
  push(_concat(p, " "))
end
print = function(...)
  local p = {}
  for i = 1, _select("#", ...) do p[i] = _tostring((_select(i, ...))) end
  push("print " .. _concat(p, "\t"))
end
local CALLS = 0
function __enter() CALLS = CALLS + 1; return CALLS > 300 end
local gpcall = _G.pcall
function __seg(id, f, ...)
  local r = _pack(gpcall(f, ...))
  if r[1] then push("seg " .. id .. " ok") else push("seg " .. id .. " error " .. sh(r[2])) end
end
function __tag(v)
  if _type(v) == "table" then return _rawget(v, "v") or "tab" end
  return v
end
function __spairs(t)
  local ks, n = {}, 0
  for k in _next, t do
    local tk = _type(k)
    if (tk == "number" and k == k) or tk == "string" then n = n + 1; ks[n] = k end
  end
  _sort(ks, keyless)
  local i = 0
  return function()
    i = i + 1
    if ks[i] ~= nil then return ks[i], _rawget(t, ks[i]) end
  end
end
"##;

const WRAP_TAIL: &str = r##"
local f, e = _load(SRC, NAME, "t")
if not f then return "\1=> SYNTAX " .. _tostring(e) end
local r = _pack(_pcall(f))
local res
if r[1] then
  local p = {}
  for i = 2, r.n do p[#p + 1] = sh(r[i]) end
  res = "\1=> OK " .. _concat(p, " ")
else
  res = "\1=> ERR " .. sh(r[2])
end
if IOF_RM then IOF_RM(IOF_NAME) end
return _concat(OUT, "\n") .. "\n" .. res
"##;

/// Escapes bytes into a Lua short-string literal, byte-exact.
fn lua_quote(src: &[u8]) -> String {
    let mut s = String::with_capacity(src.len() + 16);
    s.push('"');
    for &b in src {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            b'\n' => s.push_str("\\n"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\{b:03}")),
        }
    }
    s.push('"');
    s
}

pub fn wrap(src: &[u8], name: &str) -> String {
    let limits: Vec<String> = crate::common::C_LIMIT_ERRORS.iter().map(|m| lua_quote(m.as_bytes())).collect();
    let head = WRAP_HEAD
        .replace("LIMITS_LIST", &limits.join(", "))
        .replace("LEFT_OUT", if all_libs() { LEFT_OUT_ALL } else { LEFT_OUT });
    format!("local SRC = {}\nlocal NAME = {}\n{head}{WRAP_TAIL}", lua_quote(src), lua_quote(name.as_bytes()))
}

/// What RemedyLua leaves out (`Libs::ALL`), taken out of C Lua's globals too.
const LEFT_OUT: &str = "io, debug, package, require, dofile, loadfile = nil, nil, nil, nil, nil, nil
os.execute, os.exit, os.getenv, os.remove, os.rename, os.setlocale, os.tmpname = nil, nil, nil, nil, nil, nil, nil";

/// With `SWEEP1_ALL_LIBS=1`: every `Libs` flag (`ALL_LIBS`); only what no flag gives is
/// taken out. Our host map has C's `HOME` and `PATH`.
/// `IOF` is the one file programs may open (a temporary file in C Lua, `sweep1.io` here),
/// made empty first; any other name becomes one neither engine can open.
const LEFT_OUT_ALL: &str = "IOF = os.tmpname and os.tmpname() or 'sweep1.io'
IOF_NAME, IOF_RM = IOF, os.remove
do local f = io.open(IOF, 'w') f:close() end
for _, k in _next, {'open', 'lines', 'input', 'output'} do
  local g = io[k]
  io[k] = function(n, ...)
    if (_type(n) == 'string' or _type(n) == 'number') and n ~= IOF_NAME then n = '/nonexistent-dir/x' end
    return g(n, ...)
  end
end
dofile, loadfile, io.popen, io.tmpfile = nil, nil, nil, nil
os.execute, os.exit, os.remove, os.rename, os.tmpname = nil, nil, nil, nil, nil
for k in _next, debug do if k ~= 'traceback' and k ~= 'getinfo' then debug[k] = nil end end
package.path, package.cpath, package.config, package.searchpath, package.loadlib = nil, nil, nil, nil, nil
package.searchers[3], package.searchers[4] = nil, nil";

/// Every flag, `IO` and `ENV` included.
const ALL_LIBS: Libs =
    Libs::ALL.union(Libs::DEBUG_INFO).union(Libs::REQUIRE).union(Libs::ENV).union(Libs::IO);

pub fn all_libs() -> bool {
    std::env::var_os("SWEEP1_ALL_LIBS").is_some()
}

/// The marker the wrapper writes when a `pcall`, `xpcall`, resume, close or `load` sees
/// one of C Lua's implementation-limit errors.
pub const C_LIMIT_MARK: &str = "\u{2}c-limit";

/// Runs `f` on its own thread with a deadline. `None` when it did not finish in time.
/// (Never detached: `common::with_deadline`.)
pub fn with_timeout<T: Send + 'static>(stack: usize, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    crate::common::with_deadline(stack, TIMEOUT, f)
}

fn classify_text(text: String) -> Outcome {
    // The wrapper marks its verdict with \1 (`%q` text may span lines).
    let last = if text.contains('\u{1}') { text.rsplit('\u{1}').next().unwrap_or("") } else { "" };
    let class = if last.starts_with("=> SYNTAX") {
        Class::Syntax
    } else if last.starts_with("=> ERR") {
        Class::Runtime
    } else if last.starts_with("=> OK") {
        Class::Ok
    } else {
        Class::Host
    };
    Outcome { class, text: normalize(&text) }
}

/// What RemedyLua does with `src` (chunk `name`) under `limits`; also the fuel it used.
pub fn remedy_raw(src: &[u8], name: &str, limits: Limits) -> (Outcome, u64) {
    let (fast, fast_mem) = remedy_raw_with(src, name, limits, true);
    let (slow, slow_mem) = remedy_raw_with(src, name, limits, false);
    // The fast loop only shortcuts: the same outcome, fuel and memory either way.
    let timed_out = fast.0.class == Class::Timeout || slow.0.class == Class::Timeout;
    if !timed_out && (fast.0.text != slow.0.text || fast.1 != slow.1 || fast_mem != slow_mem) {
        // Output that differs between two runs in the same mode (`os.clock()` read at another
        // moment, and so another string length, fuel and memory too: long-run finding)
        // cannot be compared across modes: no fast/slow difference.
        // A program whose fast-loop runs agree is held to the check exactly.
        if remedy_raw_with(src, name, limits, true).0.0.text != fast.0.text {
            return fast;
        }
        let text = format!(
            "<panic> fast/slow differ: {:?} {} {fast_mem} vs {:?} {} {slow_mem}",
            fast.0.text, fast.1, slow.0.text, slow.1
        );
        return (Outcome { class: Class::Panic, text }, fast.1);
    }
    fast
}

/// `remedy_raw` with the fast loop on or off; also the memory held after the run.
fn remedy_raw_with(src: &[u8], name: &str, limits: Limits, fast: bool) -> ((Outcome, u64), usize) {
    let w = wrap(src, name);
    let mem = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mem2 = std::sync::Arc::clone(&mem);
    // On a 64 KiB stack: data-driven native recursion overflows here, not in production.
    let r = with_timeout(crate::common::SMALL_STACK, move || {
        remedy_lua::set_test_stack_cap(Some(crate::common::SMALL_STACK_CAP));
        catch_unwind(AssertUnwindSafe(|| {
            // A fixed hash seed: the two runs compared must replay exactly.
            let limits = if limits.hash_seed.is_none() { limits.hash_seed(0x5eed_7e57) } else { limits };
            let mut lua = Lua::new(limits, if all_libs() { ALL_LIBS } else { Libs::ALL });
            lua.set_fast_loop(fast);
            if all_libs() {
                let fs = crate::common::MemoryFiles::new();
                lua.set_file_opener(Some(move |n: &str, m: &str| {
                    if n == "sweep1.io" {
                        fs.open(n, m)
                    } else {
                        Err(remedy_lua::IoError::new("No such file or directory", 2))
                    }
                }));
                for k in ["HOME", "PATH"] {
                    lua.set_env(k, std::env::var(k).ok().as_deref().map(str::as_bytes));
                }
            }
            // "=t", as the oracle names the wrapper chunk: positions inside the wrapper must match.
            let r = lua.exec(&w, "=t");
            let used = limits.fuel.saturating_sub(lua.fuel_left());
            mem2.store(lua.memory_used(), std::sync::atomic::Ordering::Relaxed);
            let out = match r {
                Ok(v) => classify_text(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
                Err(Error::Limit(LimitKind::Fuel)) => Outcome { class: Class::Fuel, text: "<fuel>".into() },
                Err(Error::Limit(LimitKind::Memory)) => {
                    Outcome { class: Class::Memory, text: "<memory>".into() }
                }
                Err(e) => Outcome { class: Class::Host, text: format!("<host> {e}") },
            };
            // The state must stay usable after any outcome (with room to run again).
            lua.set_limits(remedy_limits());
            let again = lua.exec("return 1 + 1", "=again");
            let usable = matches!(again.as_deref(), Ok([remedy_lua::Value::Int(2)]));
            (out, used, usable)
        }))
    });
    let out = match r {
        None => (Outcome { class: Class::Timeout, text: "<timeout>".into() }, 0),
        Some(Err(p)) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                .unwrap_or_default();
            (Outcome { class: Class::Panic, text: format!("<panic> {msg}") }, 0)
        }
        Some(Ok((out, used, usable))) => {
            if usable {
                (out, used)
            } else {
                (Outcome { class: Class::Host, text: format!("<state unusable after> {}", out.text) }, used)
            }
        }
    };
    (out, mem.load(std::sync::atomic::Ordering::Relaxed))
}

pub fn remedy_limits() -> Limits {
    // The parser's native-stack budget makes a debug build reject ~15 nested blocks (a
    // finding of its own, see `nesting_*` tests): raise it so the fuzz sees past that.
    let fuel = std::env::var("SWEEP1_FUEL").ok().and_then(|v| v.parse().ok()).unwrap_or(REMEDY_FUEL);
    let mut l = Limits::default().fuel(fuel).memory(REMEDY_MEM);
    l.stack_bytes = 24 << 20;
    l
}

pub fn remedy(src: &[u8], name: &str) -> Outcome {
    remedy_raw(src, name, remedy_limits()).0
}

pub fn oracle(src: &[u8]) -> Outcome {
    match if close_goto_shape(src) { oracle_in_child(src) } else { oracle_raw(src) } {
        None => Outcome { class: Class::Timeout, text: "<timeout>".into() },
        Some(Ok(text)) => {
            let o = classify_text(text);
            if o.text.contains("step limit") { Outcome { class: Class::Fuel, text: o.text } } else { o }
        }
        Some(Err(e)) if e.contains("step limit") => Outcome { class: Class::Fuel, text: e },
        Some(Err(e)) => Outcome { class: Class::Host, text: format!("<host> {e}") },
    }
}

/// C Lua's result for `src` in this process (`None`: it did not finish in time).
pub fn oracle_raw(src: &[u8]) -> Option<Result<String, String>> {
    let w = wrap(src, "=t");
    with_timeout(64 << 20, move || remedy_lua_oracle::run_within(&w, ORACLE_STEPS, ORACLE_WALL))
}

/// `oracle_raw` in a child process (`sweep1_fuzz::oracle_child`): C Lua 5.4.7 can crash
/// on LR-19's shape (a segfault, `parity_26`), which in this process would end the driver.
/// A crash is the error `<crash>`.
fn oracle_in_child(src: &[u8]) -> Option<Result<String, String>> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let mut c = Command::new(std::env::current_exe().unwrap())
        .args(["sweep1_fuzz::oracle_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("oracle child");
    c.stdin.take().unwrap().write_all(src).unwrap();
    let out = c.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    match text
        .split_once(CHILD_OPEN)
        .and_then(|(_, r)| r.rsplit_once(CHILD_CLOSE))
        .map(|(r, _)| r.split_at(1))
    {
        Some(("O", t)) => Some(Ok(t.into())),
        Some(("E", t)) => Some(Err(t.into())),
        Some(_) => None,
        None => Some(Err(format!("<crash> {}", out.status))),
    }
}

/// The brackets around `oracle_child`'s result.
pub const CHILD_OPEN: &str = "\u{2}oracle{";
pub const CHILD_CLOSE: &str = "}oracle\u{2}";

/// Masks what legitimately differs: object addresses, NaN spellings, the `.rlua` chunk name.
pub fn normalize(s: &str) -> String {
    // Known difference, reported once (sweep1 fuzz-eof-quote): RemedyLua says near '<eof>'.
    let mut s = s
        .replace("-nan(ind)", "nan")
        .replace("nan(ind)", "nan")
        .replace("-nan", "nan")
        .replace("t.rlua:", "t:")
        .replace("near '<eof>'", "near <eof>");
    for kind in ["table: ", "function: ", "thread: ", "userdata: ", "builtin: ", "MyType: "] {
        let mut out = String::with_capacity(s.len());
        let mut rest = s.as_str();
        while let Some(i) = rest.find(kind) {
            out.push_str(&rest[..i + kind.len()]);
            rest = &rest[i + kind.len()..];
            let end = rest.find(|c: char| !c.is_ascii_alphanumeric()).unwrap_or(rest.len());
            out.push_str("ADDR");
            rest = &rest[end..];
        }
        out.push_str(rest);
        s = out;
    }
    // Any other `__name`'s address (`tostring` of an object whose metatable names it):
    // ": 0x" and 8 hex digits or more (shorter hex values still compare). Also ": 0X":
    // `string.upper(tostring(t))` gives "TABLE: 0X7F25..." (long-run finding).
    let mut out = String::with_capacity(s.len());
    let mut rest = s.as_str();
    while let Some(i) = rest.find(": 0x").into_iter().chain(rest.find(": 0X")).min() {
        out.push_str(&rest[..i + 2]);
        rest = &rest[i + 2..];
        let hex = rest[2..].find(|c: char| !c.is_ascii_hexdigit()).unwrap_or(rest.len() - 2);
        if hex >= 8 {
            out.push_str("ADDR");
            rest = &rest[2 + hex..];
        }
    }
    out.push_str(rest);
    out
}

/// How two outcomes differ, as a short signature (None when they agree or the comparison
/// says nothing, e.g. C Lua hit its step limit).
pub fn compare(ours: &Outcome, theirs: &Outcome) -> Option<String> {
    use Class::*;
    match (ours.class, theirs.class) {
        (Panic, _) => return Some(format!("critical:panic:{}", strip(&ours.text))),
        (Timeout, _) => return Some("critical:remedy-timeout".into()),
        (_, Timeout) => return Some("oracle-timeout".into()),
        (_, Host) if theirs.text.starts_with("<host> <crash>") => return Some("oracle-crash".into()),
        (_, Fuel) | (_, Host) => return None,
        (Fuel, _) => return Some("fuel:remedy-stopped".into()),
        (Memory, _) => return Some("memory:remedy-stopped".into()),
        _ => {}
    }
    if ours.text == theirs.text {
        return None;
    }
    let (a, b) = first_diff(&ours.text, &theirs.text);
    if crate::common::c_limit_only(&a, &b) {
        return None;
    }
    let minor = ours.class == theirs.class && is_message_only(&a, &b);
    let sev = if minor { "minor" } else { "major" };
    Some(format!("{sev}:{:?}/{:?}:{}|{}", ours.class, theirs.class, strip(&a), strip(&b)))
}

/// Takes the known non-bugs out of a finding: `"expected:<kind>"` for one, or the
/// signature itself. `src` (chunk `name`) is what RemedyLua ran, `c_src` what C Lua ran. Only for differences `compare` reports (not panics, timeouts or limit
/// stops of RemedyLua's), and each rule needs its own evidence, so a real difference in
/// the same program still shows:
/// - `c-limit`: C Lua hit one of its implementation limits (the wrapper's mark) at or
///   before the first differing line, and RemedyLua did not there;
/// - `order`: the program walks tables (`pairs`, `next`) or its output changes with
///   RemedyLua's hash seed, and C Lua's output is the same lines in another order (or each
///   differing line the same bytes in another order). Such a program is not excused
///   otherwise;
/// - `c-nondeterministic`: C Lua's output differs from one run to the next (clocks, `%p`,
///   the key `next` gives first under its random seed; up to three more runs, six for a
///   program that walks tables), and RemedyLua's differs only on lines that do (for a
///   program that walks tables, only with a line C Lua printed there in one of its runs);
pub fn classify(
    src: &[u8],
    name: &str,
    c_src: &[u8],
    ours: &Outcome,
    theirs: &Outcome,
    sig: String,
) -> String {
    // C Lua crashing on LR-19's shape (RemedyLua's result there is pinned, `parity_26`).
    if sig == "oracle-crash" && close_goto_shape(c_src) {
        return "expected:c-lua-close-goto-bug".into();
    }
    if sig.starts_with("critical")
        || sig.starts_with("fuel:")
        || sig.starts_with("memory:")
        || sig.starts_with("oracle-")
    {
        return sig;
    }
    let (la, lb): (Vec<&str>, Vec<&str>) = (ours.text.lines().collect(), theirs.text.lines().collect());
    let at = (0..la.len().max(lb.len())).find(|&i| la.get(i) != lb.get(i)).unwrap_or(0);
    // C Lua hit one of its limits where the outputs part (its line there is the mark and
    // ours is not). Limits both hit at the same point before, as a refused resume (LR-3),
    // are part of the common prefix; a mark on our side only stays a finding.
    let is_mark = |l: Option<&&str>| l.is_some_and(|l| l.contains(C_LIMIT_MARK));
    if is_mark(lb.get(at)) && !is_mark(la.get(at)) {
        return "expected:c-limit".into();
    }
    // C Lua stopped the whole program at one of its limits (its verdict is that error),
    // and everything it printed before is RemedyLua's too.
    let verdict_limit = lb
        .last()
        .is_some_and(|l| l.contains("=> ERR") && crate::common::C_LIMIT_ERRORS.iter().any(|m| l.contains(m)));
    if theirs.class == Class::Runtime && verdict_limit {
        // (An empty output is an empty first line; RemedyLua may print more after the point
        // C Lua stopped at.)
        let before: Vec<&&str> = lb[..lb.len() - 1].iter().filter(|l| !l.is_empty()).collect();
        let ours: Vec<&&str> = la.iter().filter(|l| !l.is_empty()).collect();
        if before.len() <= ours.len() && before.iter().zip(&ours).all(|(b, a)| a == b) {
            return "expected:c-limit".into();
        }
    }
    // A compile verdict is deterministic: one engine rejecting the program and the other
    // not is never excused below (C Lua's reruns "explained" an invalid `.rlua` twin as
    // `c-nondeterministic` when RemedyLua's run printed nothing the runs disagreed on).
    if (ours.class == Class::Syntax) != (theirs.class == Class::Syntax) {
        return sig;
    }
    // C Lua 5.4.7's goto-out-of-a-to-be-closed-block bug (LR-19): RemedyLua is right.
    if close_goto_shape(c_src) && close_goto_symptom(&la, &lb) {
        return "expected:c-lua-close-goto-bug".into();
    }
    // C Lua 5.4.7 compiles a store to an upvalue table under a comparison key wrong
    // (`t[a < b] = v` in a closure: the boolean lands in the table's register, "attempt to
    // index a number value", LR-25). Its difference goes away in both engines when each
    // such key is `({(key)})[1]`, the same value.
    if let Some(fixed) = compare_keys_wrapped(c_src) {
        if oracle(fixed.as_bytes()).text == ours.text && remedy(fixed.as_bytes(), "=t").text == ours.text {
            return "expected:c-lua-compare-key-bug".into();
        }
    }
    // A function named by another of its names: C Lua's error messages name a function by
    // the first global or `package.loaded` field it finds holding it, walking in its
    // hash-seeded order (`obj = setmetatable` then `pcall(setmetatable, 1)`: 'obj' or
    // 'setmetatable', LR-15). Every line that differs must differ only in one quoted name,
    // and the two names must hold the same function once the program has run.
    if la.len() == lb.len() {
        let pairs: Vec<(String, String)> =
            la.iter().zip(&lb).filter(|(a, b)| a != b).filter_map(|(a, b)| quoted_name_swap(a, b)).collect();
        let differing = la.iter().zip(&lb).filter(|(a, b)| a != b).count();
        if differing > 0
            && pairs.len() == differing
            && pairs.iter().all(|(x, y)| same_function(src, name, x, y))
        {
            return "expected:name-alias".into();
        }
    }
    // Order: does RemedyLua's own output depend on its hash seed (or does the program walk
    // a table with `pairs` or `next`, whose order for integer and other keys is placement
    // C Lua and RemedyLua do differently)?
    // (C Lua's output being RemedyLua's under another seed is no evidence of its own: a
    // bug that shows under some seeds only would pass for order.)
    let text = String::from_utf8_lossy(src);
    let walks = text.contains("pairs") || text.contains("next");
    // (Output that differs under its own seed, a clock's, says nothing of the seed: LR-24.)
    let run = |l: Limits| remedy_raw(src, name, l).0.text != ours.text;
    let varies = !walks
        && !run(remedy_limits())
        && [1u64, 2, 0xdead_beef].into_iter().any(|seed| run(remedy_limits().hash_seed(seed)));
    if (varies || walks) && ours.class == theirs.class && permutation(&ours.text, &theirs.text) {
        return "expected:order".into();
    }
    // C Lua disagreeing with itself (a clock, `%p`, an address, the key `next` gives first,
    // the name an error finds for a function by walking `package.loaded`: the last two
    // under its random seed). C Lua runs again, up to 16 times whatever the program (a
    // name walk has no `pairs` in the source: LR-15), and RemedyLua's output must be
    // explained by what those runs printed (`c_runs_explain`). A program that walks tables
    // must take each varying value from C Lua's runs; any other (a clock) may put any value
    // where C Lua's runs vary.
    let free = !(varies || walks);
    let mut runs: Vec<Vec<String>> = vec![lb.iter().map(|l| (*l).to_owned()).collect()];
    // A table walked: RemedyLua's own runs under other hash seeds count as runs too, of the
    // program and of its plain twin (a `.rlua` program and its twin compile to different
    // code, so their tables can be laid out differently under one seed: LR-20). C Lua's 17
    // runs alone missed RemedyLua's first key among four about once in 130 findings (LR-10
    // again, `fuzz_mutated` seed 10017); with 8 seeds more, about once in 1,300. Each varying
    // value must still be one some run printed, and the runs must disagree.
    if walks {
        for seed in [1u64, 2, 3, 4, 5, 6, 7, 0xdead_beef] {
            let twins: &[(&[u8], &str)] =
                if src == c_src { &[(src, name)] } else { &[(src, name), (c_src, "=t")] };
            for &(code, nm) in twins {
                let o = remedy_raw(code, nm, remedy_limits().hash_seed(seed)).0;
                runs.push(o.text.lines().map(str::to_owned).collect());
            }
        }
        if c_runs_explain(&la, &runs, free) {
            return "expected:c-nondeterministic".into();
        }
    }
    for _ in 0..16 {
        let again = oracle(c_src);
        runs.push(again.text.lines().map(str::to_owned).collect());
        if c_runs_explain(&la, &runs, free) {
            return "expected:c-nondeterministic".into();
        }
    }
    // An address the program took apart past `normalize` (`tostring(t):gsub("", ...)` splits
    // its digits, LR-31). One table more made first moves C Lua's addresses: a line that moves
    // with them may differ from RemedyLua's in hex digits only, in 4 places or more (an
    // address's digits, not one letter: a `next` key "a" against "c"), and every other line
    // must match.
    let moved = oracle(&[b"local _ = {} ".as_slice(), c_src].concat()).text;
    let moved: Vec<&str> = moved.lines().collect();
    let digits_only = |a: &str, b: &str| {
        let d: Vec<(char, char)> = a.chars().zip(b.chars()).filter(|(x, y)| x != y).collect();
        a.chars().count() == b.chars().count()
            && d.len() >= 4
            && d.iter().all(|(x, y)| x.is_ascii_hexdigit() && y.is_ascii_hexdigit())
    };
    if la.len() == lb.len()
        && moved.len() == lb.len()
        && moved != lb
        && la.iter().zip(&lb).zip(&moved).all(|((a, b), m)| a == b || (m != b && digits_only(a, b)))
    {
        return "expected:c-address".into();
    }
    sig
}

/// Whether C Lua's runs of one program (`runs`, each its output lines) disagree with each
/// other and explain RemedyLua's output `ours` (long-run findings LR-10, LR-14, LR-15):
/// - the lines RemedyLua shares with every run at the start and at the end are set aside;
/// - when the runs all have the same number of lines, each line between is RemedyLua's
///   at its place where the runs agree there, and where they do not, token by token
///   (split at blanks): a token the runs agree on is RemedyLua's, a token they vary on is
///   one some run printed there (any value when `free`). Tokens: `next` over two tables
///   on one line varies in both, and RemedyLua's pair need not be one run's (LR-10);
/// - when the runs' lengths vary (output that follows an address's digits: LR-14), each
///   line between is one some run printed between.
/// A difference on a line or token C Lua printed the same way in every run stays.
pub fn c_runs_explain(ours: &[&str], runs: &[Vec<String>], free: bool) -> bool {
    let Some(first) = runs.first() else { return false };
    if runs.iter().all(|r| r == first) {
        return false;
    }
    // The start and the end RemedyLua shares with every run (a difference there is left to
    // the middle, which then has to explain it).
    let min = runs.iter().map(Vec::len).min().unwrap_or(0).min(ours.len());
    let prefix = (0..min).take_while(|&i| runs.iter().all(|r| r[i] == ours[i])).count();
    let suffix = (0..min - prefix)
        .take_while(|&k| runs.iter().all(|r| r[r.len() - 1 - k] == ours[ours.len() - 1 - k]))
        .count();
    let mid = &ours[prefix..ours.len() - suffix];
    if runs.iter().all(|r| r.len() == first.len()) {
        if ours.len() != first.len() {
            return false;
        }
        return mid.iter().enumerate().all(|(k, line)| {
            let i = prefix + k;
            let cands: Vec<&str> = runs.iter().map(|r| r[i].as_str()).collect();
            if cands.iter().all(|c| c == line) {
                return true;
            }
            if cands.iter().all(|c| *c == cands[0]) {
                return false;
            }
            let toks = |s: &str| s.split([' ', '\t']).map(str::to_owned).collect::<Vec<_>>();
            let ct: Vec<Vec<String>> = cands.iter().map(|c| toks(c)).collect();
            let lt = toks(line);
            if ct.iter().any(|t| t.len() != lt.len()) {
                // (Token counts vary: whole lines, any when free.)
                return free || cands.contains(line);
            }
            lt.iter().enumerate().all(|(j, tok)| {
                let stable = ct.iter().all(|t| t[j] == ct[0][j]);
                if stable { *tok == ct[0][j] } else { free || ct.iter().any(|t| t[j] == *tok) }
            })
        });
    }
    let seen: std::collections::HashSet<&str> =
        runs.iter().flat_map(|r| r[prefix..r.len() - suffix].iter().map(String::as_str)).collect();
    mid.iter().all(|l| seen.contains(l))
}

/// The shape of C Lua 5.4.7's bug LR-19 in the program C Lua ran:
/// a `goto L` that leaves a block in which a `<close>` local is in scope, to a label `::L::`
/// that ends an enclosing block (the next token closes it), with a local holding a run-time
/// value (not a literal, not `<close>`) declared at the label's level between the two. C Lua
/// then skips the close (and calls a missing `__close` later, or leaves the loop early).
pub fn close_goto_shape(c_src: &[u8]) -> bool {
    use super::mutate::{Kind, tokenize};
    let toks = tokenize(&String::from_utf8_lossy(c_src));
    let t = |i: usize| toks.get(i).map_or("", |t| t.text.as_str());
    // Block depth before each token.
    let mut depth = Vec::with_capacity(toks.len());
    let mut d = 0i32;
    for tk in &toks {
        depth.push(d);
        match tk.text.as_str() {
            "do" | "then" | "function" | "repeat" => d += 1,
            "end" | "until" | "elseif" => d -= 1,
            _ => {}
        }
    }
    let is_close_local =
        |i: usize| t(i) == "local" && t(i + 2) == "<" && t(i + 3) == "close" && t(i + 4) == ">";
    for g in 0..toks.len() {
        if t(g) != "goto" || toks.get(g + 1).is_none_or(|k| !matches!(k.kind, Kind::Name)) {
            continue;
        }
        let name = t(g + 1);
        let Some(l) = (g + 2..toks.len()).find(|&i| t(i) == "::" && t(i + 1) == name && t(i + 2) == "::")
        else {
            continue;
        };
        let dl = depth[l];
        // The label ends its block.
        if !matches!(t(l + 3), "end" | "until" | "elseif" | "else") {
            continue;
        }
        // The goto is deeper, inside a block with a `<close>` local in scope (declared
        // deeper than the label and not closed before the goto).
        if depth[g] <= dl {
            continue;
        }
        let mut min = depth[g];
        let close_in_scope = (0..g).rev().any(|i| {
            min = min.min(depth[i]);
            is_close_local(i) && depth[i] > dl && depth[i] <= min
        });
        if !close_in_scope {
            continue;
        }
        // Between the goto's block's end and the label: a local at the label's level with a
        // run-time value.
        let Some(out) = (g..l).find(|&i| depth[i] == dl) else { continue };
        let runtime_local = (out..l).any(|i| {
            if depth[i] != dl || t(i) != "local" || is_close_local(i) {
                return false;
            }
            if t(i + 1) == "function" {
                return true;
            }
            let Some(eq) = (i..l).find(|&j| t(j) == "=") else { return false };
            let v = &toks[eq + 1];
            let literal = matches!(v.kind, Kind::Num | Kind::Str)
                || matches!(v.text.as_str(), "true" | "false" | "nil");
            !literal || !matches!(t(eq + 2), "" | ";" | "::" | "local" | "end")
        });
        if runtime_local {
            return true;
        }
    }
    false
}

/// C Lua's symptom of LR-19: its error names the missing `close` metamethod, or it left the
/// loop early (its output is RemedyLua's with lines missing, ending the same way).
/// (Either way, every line C Lua printed before that is RemedyLua's too, in order.)
/// `src` with each key of a store `[key] =` that holds a comparison at its top level written
/// `({(key)})[1]` (LR-25); `None` when there is none.
pub fn compare_keys_wrapped(src: &[u8]) -> Option<String> {
    let mut toks = super::mutate::tokenize(&String::from_utf8_lossy(src));
    let mut changed = false;
    let mut i = 0;
    while i < toks.len() {
        if toks[i].text == "[" {
            let (mut d, mut cmp, mut j) = (0, false, i + 1);
            while j < toks.len() && (d > 0 || toks[j].text != "]") {
                match toks[j].text.as_str() {
                    "[" | "(" | "{" => d += 1,
                    "]" | ")" | "}" => d -= 1,
                    "==" | "~=" | "<" | ">" | "<=" | ">=" if d == 0 => cmp = true,
                    _ => {}
                }
                j += 1;
            }
            if cmp && toks.get(j + 1).is_some_and(|t| t.text == "=") {
                toks[i].text = "[({(".into();
                toks[j].text = ")})[1]]".into();
                changed = true;
            }
        }
        i += 1;
    }
    changed.then(|| super::mutate::join(&toks))
}

fn close_goto_symptom(ours: &[&str], theirs: &[&str]) -> bool {
    let within = |part: &[&str]| {
        let mut it = ours.iter();
        part.iter().all(|l| it.any(|o| o == l))
    };
    // (The error may also be a segment's, mid-output: the misplaced jump calls a nil value,
    // `seg N error "... attempt to call a nil value"`, where RemedyLua's segment ran.)
    let err = |l: &str| {
        l.contains("(metamethod 'close')")
            || (l.contains(" error \"") && l.contains("attempt to call a nil value"))
    };
    if let Some(i) = theirs.iter().position(|l| err(l)) {
        return within(&theirs[..i]);
    }
    match theirs.split_last() {
        Some((last, _)) => theirs.len() < ours.len() && Some(last) == ours.last() && within(theirs),
        None => false,
    }
}

/// The two names when lines `a` and `b` differ only in one quoted name (`'obj'` against
/// `'setmetatable'`, or `'string.rep'`).
pub fn quoted_name_swap(a: &str, b: &str) -> Option<(String, String)> {
    let p = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let start = a[..p].rfind('\'')? + 1;
    let (ra, rb) = (&a[start..], &b[start..]);
    let (ea, eb) = (ra.find('\'')?, rb.find('\'')?);
    let (x, y) = (&ra[..ea], &rb[..eb]);
    let name =
        |n: &str| !n.is_empty() && n.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.');
    (name(x) && name(y) && x != y && ra[ea..] == rb[eb..]).then(|| (x.to_owned(), y.to_owned()))
}

/// Whether `x` and `y` (a global, or `module.field` as C Lua names `package.loaded`'s
/// entries) hold the same function after `src` has run in RemedyLua.
fn same_function(src: &[u8], name: &str, x: &str, y: &str) -> bool {
    let check = format!(
        "local f = load({}, {}) if f then pcall(f) end \
         local function r(n) local m, k = n:match('^(.-)%.([^.]+)$') \
         if m then local t = (package and package.loaded[m]) or rawget(_G, m) return type(t) == 'table' and rawget(t, k) or nil end \
         return rawget(_G, n) end \
         local a, b = r({}), r({}) __emit('ALIAS-CHECK', type(a) == 'function' and rawequal(a, b))",
        lua_quote(src),
        lua_quote(name.as_bytes()),
        lua_quote(x.as_bytes()),
        lua_quote(y.as_bytes()),
    );
    let o = remedy(check.as_bytes(), "=alias");
    o.text.lines().any(|l| l.contains("\"ALIAS-CHECK\" true"))
}

/// The same lines in another order, each line pair that differs at its place the same
/// bytes in another order (a traversal's output).
fn permutation(a: &str, b: &str) -> bool {
    let (mut la, mut lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    if la.len() != lb.len() {
        return false;
    }
    let bytes_sorted = |s: &str| {
        let mut v = s.as_bytes().to_vec();
        v.sort_unstable();
        v
    };
    if la.iter().zip(&lb).all(|(x, y)| x == y || bytes_sorted(x) == bytes_sorted(y)) {
        return true;
    }
    la.sort_unstable();
    lb.sort_unstable();
    la == lb
}

fn first_diff(a: &str, b: &str) -> (String, String) {
    let (la, lb): (Vec<_>, Vec<_>) = (a.lines().collect(), b.lines().collect());
    for i in 0..la.len().max(lb.len()) {
        let (x, y) = (la.get(i).copied().unwrap_or("<none>"), lb.get(i).copied().unwrap_or("<none>"));
        if x != y {
            return (x.to_owned(), y.to_owned());
        }
    }
    (String::new(), String::new())
}

/// Both lines report an error and differ only in the message text.
fn is_message_only(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches('\u{1}'), b.trim_start_matches('\u{1}'));
    let err = |s: &str| {
        s.starts_with("=> ERR")
            || s.starts_with("=> SYNTAX")
            || s.contains(" error \"")
            || s.starts_with("print ")
    };
    err(a) && err(b) && a.split(' ').next() == b.split(' ').next()
}

/// A diff line with the details that vary between programs taken out (for grouping).
pub fn strip(s: &str) -> String {
    let mut out = String::new();
    let mut in_q = false;
    let mut last_digit = false;
    for c in s.chars().take(160) {
        if c == '\'' {
            in_q = !in_q;
            out.push('\'');
            continue;
        }
        if in_q {
            continue;
        }
        if c.is_ascii_digit() {
            if !last_digit {
                out.push('#');
            }
            last_digit = true;
            continue;
        }
        last_digit = false;
        out.push(c);
    }
    out
}

/// The dialect a chunk name selects, for callers that load `.rlua` directly.
pub fn dialect_of(name: &str) -> Dialect {
    Dialect::for_name(name.trim_start_matches('='))
}
