//! Standard library coverage against C Lua 5.4.7.
//!
//! The probe sets in `libs_coverage/*.txt` are one expression (or chunk) per line: edge
//! cases of every function both have. Each runs under `pcall` in RemedyLua (every library,
//! the opt-in ones included; fast loop on and off with identical results, fuel and memory)
//! and in C Lua, results rendered with their types, and every line must be equal.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

const FUEL: u64 = 1_000_000_000;

/// The probe runner: `PROBES` is prepended.
const RUNNER: &str = r##"
local function show(...)
  local n = select('#', ...)
  local t = {}
  for i = 1, n do
    local v = select(i, ...)
    local ty = type(v)
    if ty == "number" then t[i] = (math.type(v) == "integer" and "i:" or "f:") .. string.format("%.17g", v)
    elseif ty == "string" then t[i] = string.format("%q", v)
    elseif ty == "table" or ty == "function" or ty == "thread" or ty == "userdata" then t[i] = ty
    else t[i] = tostring(v) end
  end
  return table.concat(t, ",")
end
local out = {}
for _, src in ipairs(PROBES) do
  local f, e = load("return " .. src, "=p")
  if not f then f, e = load(src, "=p") end
  local r
  if not f then r = "COMPILE " .. e
  else r = show(pcall(f)) end
  out[#out + 1] = src .. "  =>  " .. (r:gsub("\n", "\\n"))
end
return table.concat(out, "\n")
"##;

/// The program for a probe set.
pub fn probe_program(probes: &str) -> String {
    let mut s = String::from("PROBES = {\n");
    // `--#glibc <probe>`: the answer is the C library's and Windows' has none to compare
    // (no date before 1970 or past the year 3000), so the probe does not run there.
    let glibc = |l| str::strip_prefix(l, "--#glibc ").map_or(Some(l), |p| (!cfg!(windows)).then_some(p));
    for line in probes.lines().filter_map(glibc).filter(|l| !l.trim().is_empty() && !l.starts_with("--")) {
        assert!(!line.contains("]===]"), "{line}");
        s.push_str("[===[");
        s.push_str(line);
        s.push_str("]===],\n");
    }
    s.push_str("}\n");
    s.push_str(RUNNER);
    s
}

/// Runs a probe set in both and lists every line that differs.
fn same_as_c(probes: &str) {
    let program = probe_program(probes);
    let ours = common::remedy_libs(&program, FUEL, common::ALL_OPT_IN).unwrap();
    let c = common::oracle(&program, FUEL).unwrap();
    let (ours, c): (Vec<_>, Vec<_>) = (ours.lines().collect(), c.lines().collect());
    assert_eq!(ours.len(), c.len(), "{ours:?}\n{c:?}");
    let diffs: Vec<String> =
        ours.iter().zip(&c).filter(|(a, b)| a != b).map(|(a, b)| format!("ours: {a}\n   C: {b}")).collect();
    assert!(diffs.is_empty(), "{} probes differ from C Lua:\n{}", diffs.len(), diffs.join("\n"));
}

#[test]
fn math_and_string_probes_match_c_lua() {
    same_as_c(include_str!("libs_coverage/math_string.txt"));
}

#[test]
fn base_table_utf8_coroutine_os_gc_load_probes_match_c_lua() {
    same_as_c(include_str!("libs_coverage/base_libs.txt"));
}

#[test]
fn argument_errors_and_operators_match_c_lua() {
    same_as_c(include_str!("libs_coverage/args_ops.txt"));
}

#[test]
fn os_time_reads_and_writes_its_table_as_c_lua() {
    same_as_c(include_str!("libs_coverage/os_time.txt"));
}

/// `collectgarbage("step", n)`: C's debt loop, so a step of `n` KiB ends a cycle exactly
/// where C's does on these heaps. (Regression: `n` was ignored, one phase step ran, and
/// `step(1)` on 20,000 live tables returned true because the step started a cycle.)
#[test]
fn gc_step_with_a_size_matches_c_lua() {
    same_as_c(include_str!("libs_coverage/gc_step.txt"));
}

/// The fuel a script's `collectgarbage("step", n)` loops use is the same with the fast loop
/// on and off and under every hash seed: `n` sets a number of steps of the usual size, each
/// charged its work carried (S5C-1), never where cumulative work crosses a mark (which
/// follows how a cycle's work splits into steps, and so the seed).
#[test]
fn gc_step_fuel_is_independent_of_the_seed_and_the_fast_loop() {
    use remedy_lua::{Libs, Limits, Lua};
    let fuel = |src: &str, seed: u64, fast: bool| {
        let mut lua = Lua::new(Limits::default().fuel(FUEL).hash_seed(seed), Libs::ALL);
        lua.set_fast_loop(fast);
        let r = lua.exec(src, "=t").unwrap();
        (FUEL - lua.fuel_left(), r.len())
    };
    for kb in [0, 1, 7, 64, 1000] {
        for n in [10, 500, 4000] {
            let src = format!(
                "local keep = {{}} for i = 1, {n} do keep['k' .. i] = {{i, tostring(i)}} end \
                 local steps = 0 for r = 1, 3 do \
                   local junk = {{}} for i = 1, {n} do junk[i] = {{}} end junk = nil \
                   repeat steps = steps + 1 until collectgarbage('step', {kb}) or steps > 100000 end \
                 return steps"
            );
            let mut fuels = std::collections::BTreeSet::new();
            for seed in [1u64, 2, 3, 4, 5] {
                fuels.insert(fuel(&src, seed, true));
            }
            fuels.insert(fuel(&src, 1, false));
            assert_eq!(fuels.len(), 1, "kb {kb}, n {n}: {fuels:?}");
        }
    }
}

/// `os.setlocale` (`Libs::OS`) as a process that stays in the `"C"` locale answers. (`""`,
/// the environment's locale, is checked in RemedyLua only: C's answer follows the test's
/// environment, and setting it would change the oracle process's locale.)
#[test]
fn setlocale_matches_c_lua_in_the_c_locale() {
    use remedy_lua::{Libs, Limits, Lua};
    same_as_c(include_str!("libs_coverage/setlocale.txt"));
    let mut lua = Lua::new(Limits::default(), Libs::SAFE);
    let r = lua.exec("return os.setlocale(''), os.setlocale('', 'numeric')", "=t").unwrap();
    assert_eq!([common::show(&lua, r[0]), common::show(&lua, r[1])], ["C", "C"]);
    // The process locale is untouched: C's `setlocale(LC_ALL, NULL)` still reads "C".
    lua.exec("os.setlocale('fr_FR') os.setlocale('C', 'numeric')", "=t").unwrap();
    assert_eq!(common::oracle("return os.setlocale()", FUEL).as_deref(), Ok("\"C\""));
}

// ---- os.getenv (`Libs::ENV`)

fn env_state(libs: remedy_lua::Libs, fast: bool) -> remedy_lua::Lua {
    let mut lua =
        remedy_lua::Lua::new(remedy_lua::Limits::default().fuel(FUEL).hash_seed(common::TEST_SEED), libs);
    lua.set_fast_loop(fast);
    lua
}

/// `os.getenv` against C Lua: the host's map holds the variables C Lua reads from the
/// process, so where both have a value it is the same, and a name neither has is nil.
#[test]
fn getenv_matches_c_lua_for_what_the_host_gives() {
    let program = r#"
        local out = {}
        for _, k in ipairs({"PATH", "HOME", "REMEDYLUA_NO_SUCH_VARIABLE", "PATH\0tail", ""}) do
          out[#out + 1] = tostring(os.getenv(k))
        end
        out[#out + 1] = select(2, pcall(os.getenv))
        out[#out + 1] = select(2, pcall(os.getenv, {}))
        out[#out + 1] = tostring(os.getenv(1))
        out[#out + 1] = select('#', os.getenv("REMEDYLUA_NO_SUCH_VARIABLE"))
        return table.concat(out, "|")"#;
    let mut results = Vec::new();
    for fast in [true, false] {
        let mut lua = env_state(common::ALL_OPT_IN | remedy_lua::Libs::ENV, fast);
        for k in ["PATH", "HOME"] {
            let v = std::env::var(k).unwrap_or_else(|_| panic!("the test needs {k} set"));
            lua.set_env(k, Some(v.as_bytes()));
        }
        let r = lua.exec(common::wrap(program), "=wrapper").unwrap();
        results.push((lua.to_string_lossy(r[0]).unwrap(), FUEL - lua.fuel_left()));
    }
    assert_eq!(results[0], results[1], "fast loop and full path differ");
    assert_eq!(Ok(results[0].0.clone()), common::oracle(program, FUEL));
}

/// The flag is off unless a host opts in, the map starts empty (the process environment is
/// never read), and `set_env(name, None)` removes a name.
#[test]
fn getenv_is_opt_in_and_reads_only_the_hosts_map() {
    use remedy_lua::Libs;
    for libs in [Libs::SAFE, Libs::STANDARD, Libs::ALL, common::ALL_OPT_IN] {
        let mut lua = env_state(libs, true);
        lua.set_env("X", Some(b"1"));
        assert_eq!(lua.exec("return os.getenv", "=t").unwrap()[0], remedy_lua::Value::Nil, "{libs:?}");
    }
    let mut lua = env_state(Libs::BASE | Libs::ENV, true);
    assert!(std::env::var_os("PATH").is_some());
    let r = lua.exec("return os.getenv('PATH'), os.time, os.setlocale, type(os.getenv)", "=t").unwrap();
    assert_eq!(r[0], remedy_lua::Value::Nil, "the process environment leaked");
    assert_eq!(
        (r[1], r[2]),
        (remedy_lua::Value::Nil, remedy_lua::Value::Nil),
        "ENV alone opens only os.getenv"
    );
    assert_eq!(common::show(&lua, r[3]), "function");
    lua.set_env("K", Some(b"v\0w"));
    let r = lua.exec("return os.getenv('K')", "=t").unwrap();
    assert_eq!(lua.str_bytes(r[0]).unwrap(), b"v\0w");
    lua.set_env("K", None);
    assert_eq!(lua.exec("return os.getenv('K')", "=t").unwrap()[0], remedy_lua::Value::Nil);
    lua.set_env("K", Some(b"again"));
    lua.clear_env();
    assert_eq!(lua.exec("return os.getenv('K')", "=t").unwrap()[0], remedy_lua::Value::Nil);
}

/// Metered: a name of 16 MiB is not hashed (cheap and nil), a large value is charged per
/// 32 bytes and counted as memory once it is a string, and fuel follows neither the fast
/// loop nor the hash seed.
#[test]
fn getenv_is_metered() {
    use remedy_lua::Libs;
    let run = |seed: u64, fast: bool, src: &str| {
        let mut lua = remedy_lua::Lua::new(
            remedy_lua::Limits::default().fuel(FUEL).hash_seed(seed),
            Libs::ALL | Libs::ENV,
        );
        lua.set_fast_loop(fast);
        lua.set_env("BIG", Some(&vec![b'x'; 1 << 20]));
        lua.set_env("A", Some(b"1"));
        let t = std::time::Instant::now();
        let r = lua.exec(src, "=t").unwrap();
        (common::show(&lua, r[0]), FUEL - lua.fuel_left(), t.elapsed())
    };
    let long = "local n = string.rep('A', 16 << 20) local c = 0 for i = 1, 50 do if os.getenv(n) == nil then c = c + 1 end end return c";
    let (r, _, took) = run(1, true, long);
    assert_eq!(r, "50");
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    let big = "local s = 0 for i = 1, 20 do s = s + #os.getenv('BIG') + #os.getenv('A') end return s";
    let (r, fuel, _) = run(1, true, big);
    assert_eq!(r, (20 * ((1 << 20) + 1)).to_string());
    assert!(fuel >= 20 * (1 << 20) / 32, "a 1 MiB value charged {fuel} for 20 reads");
    for src in [long, big] {
        let base = run(1, true, src).1;
        for (seed, fast) in [(2, true), (3, true), (1, false), (4, false)] {
            assert_eq!(run(seed, fast, src).1, base, "{src}");
        }
    }
    // Its result is memory like any string's.
    let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default().memory(4 << 20), Libs::ALL | Libs::ENV);
    lua.set_env("BIG", Some(&vec![b'x'; 8 << 20]));
    assert!(matches!(lua.exec("return os.getenv('BIG')", "=t"), Err(remedy_lua::Error::Limit(_))));
}

// ---- io (`Libs::IO`)

/// Runs `program` (which uses files `NAME` and `NAME2`, absent at first) here with the
/// host's in-memory files (fast loop on and off: same results and fuel) and in C Lua with
/// two temporary files (removed after). Returns both outcomes. The prefixes are one line,
/// so positions in messages agree.
fn io_both(program: &str) -> (Result<String, String>, Result<String, String>) {
    use common::MemoryFiles;
    use remedy_lua::{Libs, Limits, Lua};
    let ours_src = format!(
        "local NAME, NAME2 = '/tmp/remedylua_io_a', '/tmp/remedylua_io_b' local function body() {program}\nend return body()"
    );
    let mut runs = Vec::new();
    for fast in [true, false] {
        let fs = MemoryFiles::new();
        let mut lua = Lua::new(
            Limits::default().fuel(FUEL).hash_seed(common::TEST_SEED),
            common::ALL_OPT_IN | Libs::IO,
        );
        lua.set_fast_loop(fast);
        let opener = fs.clone();
        lua.set_file_opener(Some(move |n: &str, m: &str| opener.open(n, m)));
        let r = match lua.exec(common::wrap(&ours_src), "=wrapper") {
            Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
            Err(e) => Err(e.to_string()),
        };
        runs.push((r, FUEL - lua.fuel_left()));
    }
    assert_eq!(runs[0], runs[1], "fast loop and full path differ");
    let c_src = format!(
        "local NAME, NAME2 = os.tmpname(), os.tmpname() os.remove(NAME) os.remove(NAME2) local function body() {program}\nend \
         local r = table.pack(pcall(body)) os.remove(NAME) os.remove(NAME2) if not r[1] then error(r[2], 0) end return table.unpack(r, 2, r.n)"
    );
    (runs.remove(0).0, common::oracle(&c_src, FUEL))
}

/// `io` against C Lua: files opened, written, read in every format, seeked, closed,
/// iterated with `lines`; the defaults (`io.input`, `io.output`); every message and
/// error result.
#[test]
fn io_matches_c_lua() {
    let (ours, c) = io_both(include_str!("libs_coverage/io.lua"));
    let (ours, c) = (ours.unwrap(), c.unwrap());
    let (ol, cl): (Vec<_>, Vec<_>) = (ours.lines().collect(), c.lines().collect());
    // The error code is the C library's: Windows' sets none for a read from a file opened
    // to write, and calls an empty name an invalid argument.
    let errno = |l: &&str| {
        cfg!(windows) && ["read from write-only =>", "io.output missing =>"].iter().any(|p| l.starts_with(p))
    };
    let diffs: Vec<String> = ol
        .iter()
        .zip(&cl)
        .filter(|(a, b)| a != b && !errno(a))
        .map(|(a, b)| format!("ours: {a}\n   C: {b}"))
        .collect();
    assert!(
        diffs.is_empty() && ol.len() == cl.len(),
        "{} of {} differ:\n{}",
        diffs.len(),
        ol.len(),
        diffs.join("\n")
    );
}

/// `Libs::IO` is off unless a host opts in; with it and no opener, nothing opens; the
/// default standard streams: stdin empty, stdout and stderr where `print` writes (in order
/// with it).
#[test]
fn io_is_opt_in_and_reaches_only_the_hosts_streams() {
    use remedy_lua::{Libs, Limits, Lua, Value};
    for libs in [Libs::SAFE, Libs::STANDARD, Libs::ALL, common::ALL_OPT_IN | Libs::ENV] {
        let mut lua = Lua::new(Limits::default(), libs);
        assert_eq!(lua.exec("return io", "=t").unwrap()[0], Value::Nil, "{libs:?}");
    }
    let mut lua = Lua::new(Limits::default(), Libs::ALL | Libs::IO);
    let r = lua
        .exec(
            "print('a') io.write('b', 1, 2.5, '\\n') io.stderr:write('e\\n') print('c') \
             local f, e, n = io.open('/etc/passwd') local g = io.open('Cargo.toml') \
             return f, e, n, g, io.read(), io.read('a'), io.popen, io.tmpfile",
            "=t",
        )
        .unwrap();
    assert_eq!(lua.take_output(), "a\nb12.5\ne\nc\n");
    let shown: Vec<String> = r.iter().map(|v| common::show(&lua, *v)).collect();
    assert_eq!(shown, ["nil", "/etc/passwd: No such file or directory", "2", "nil", "nil", "", "nil", "nil"]);
}

/// The host's standard streams and files: what a script writes reaches them, what they
/// hold is read; `MemoryFiles` is shared with the host; a limited one fails a write as a
/// full disk.
#[test]
fn io_uses_the_hosts_streams_and_files() {
    use common::MemoryFiles;
    use remedy_lua::{IoError, Libs, Limits, Lua, Stream};
    use std::sync::{Arc, Mutex};
    struct Shared(Arc<Mutex<Vec<u8>>>);
    impl Stream for Shared {
        fn write(&mut self, data: &[u8]) -> Result<(), IoError> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(())
        }
    }
    let out = Arc::new(Mutex::new(Vec::new()));
    let err = Arc::new(Mutex::new(Vec::new()));
    let fs = MemoryFiles::limited(1000);
    fs.insert("in.txt", "first\n42 rest\n");
    let stdin = fs.open("in.txt", "r").unwrap();
    let mut lua = Lua::new(Limits::default(), Libs::ALL | Libs::IO);
    lua.set_stdout(Some(Box::new(Shared(Arc::clone(&out)))));
    lua.set_stderr(Some(Box::new(Shared(Arc::clone(&err)))));
    lua.set_stdin(Some(stdin));
    let opener = fs.clone();
    lua.set_file_opener(Some(move |n: &str, m: &str| opener.open(n, m)));
    let r = lua
        .exec(
            "local l, n, rest = io.read('l', 'n', 'L') io.write(l, ':', n, ':', rest) io.stderr:write('oops') print('p') \
             local f = assert(io.open('made.txt', 'w')) f:write('by script') f:close() \
             local g = assert(io.open('big', 'w')) local ok, e, code = g:write(string.rep('x', 2000)) g:close() \
             return tostring(ok) .. ' ' .. tostring(e) .. ' ' .. tostring(code)",
            "=t",
        )
        .unwrap();
    assert_eq!(&*out.lock().unwrap(), b"first:42: rest\n");
    assert_eq!(&*err.lock().unwrap(), b"oops");
    assert_eq!(lua.take_output(), "p\n");
    assert_eq!(fs.get("made.txt").as_deref(), Some(&b"by script"[..]));
    assert_eq!(common::show(&lua, r[0]), "nil No space left on device 28");
    assert!(fs.total() <= 1000);
}

/// Metered: reading is charged per 32 bytes, a read past the memory limit (or
/// `string_len`) stops on it, an endless stream read whole stops on the fuel or memory
/// limit, every file handle counts its read-ahead as memory, writes past the output limit
/// are cut, and fuel is the same across seeds and with the fast loop on and off.
#[test]
fn io_is_metered() {
    use common::MemoryFiles;
    use remedy_lua::{Error, IoError, Libs, LimitKind, Limits, Lua, Stream};
    struct Endless;
    impl Stream for Endless {
        fn read(&mut self, buf: &mut [u8]) -> Result<usize, IoError> {
            buf.fill(b'z');
            Ok(buf.len())
        }
    }
    let state = |limits: Limits, seed: u64, fast: bool| {
        let fs = MemoryFiles::new();
        fs.insert("big", vec![b'q'; 4 << 20]);
        let mut lua = Lua::new(limits.hash_seed(seed), Libs::ALL | Libs::IO);
        lua.set_fast_loop(fast);
        lua.set_file_opener(Some(move |n: &str, m: &str| fs.open(n, m)));
        lua.set_stdin(Some(Box::new(Endless)));
        lua
    };
    // 4 MiB read whole: at least 4 MiB / 32 fuel, the same everywhere.
    let src = "local f = io.open('big') local s = f:read('a') f:close() local n = 0 for l in io.lines('big', 4096) do n = n + #l end return #s + n";
    let mut fuels = std::collections::BTreeSet::new();
    for (seed, fast) in [(1, true), (1, false), (2, true), (3, false)] {
        let mut lua = state(Limits::default().fuel(FUEL), seed, fast);
        let r = lua.exec(src, "=t").unwrap();
        assert_eq!(common::show(&lua, r[0]), (8 << 20).to_string());
        fuels.insert(FUEL - lua.fuel_left());
    }
    assert_eq!(fuels.len(), 1, "{fuels:?}");
    assert!(*fuels.first().unwrap() >= 2 * (4 << 20) / 32, "{fuels:?}");
    // Past the memory limit, and past `string_len`.
    let mut lua = state(Limits::default().memory(2 << 20), 1, true);
    assert!(matches!(
        lua.exec("return io.open('big'):read('a')", "=t"),
        Err(Error::Limit(LimitKind::Memory))
    ));
    let mut lua = state(Limits::default().string_len(1 << 20), 1, true);
    assert!(matches!(
        lua.exec("return io.open('big'):read('a')", "=t"),
        Err(Error::Limit(LimitKind::Memory))
    ));
    // An endless stream: `a`, a line, a huge count.
    for src in [
        "return io.read('a')",
        "return io.read('l')",
        "return io.read(1 << 40)",
        "for l in io.lines(nil, 100) do end",
    ] {
        let mut lua = state(Limits::default().fuel(200_000), 1, true);
        let t = std::time::Instant::now();
        assert!(
            matches!(lua.exec(src, "=t"), Err(Error::Limit(LimitKind::Fuel | LimitKind::Memory))),
            "{src}"
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{src}: {:?}", t.elapsed());
    }
    // File handles count their read-ahead: 10,000 open files need 40 MB.
    let mut lua = state(Limits::default().memory(8 << 20), 1, true);
    assert!(matches!(
        lua.exec("local t = {} for i = 1, 10000 do t[i] = io.open('big') end", "=t"),
        Err(Error::Limit(LimitKind::Memory))
    ));
    // The output limit cuts `io.write` as it cuts `print`.
    let mut lua = state(Limits::default().output(10), 1, true);
    lua.exec("io.write(string.rep('o', 100)) print('p')", "=t").unwrap();
    assert_eq!(lua.take_output(), "oooooooooo");
}

/// Runs a program that returns one line per check in both engines (fast loop on and off
/// here) and lists every line that differs.
fn lines_same_as_c(program: &str) {
    let ours = common::remedy_libs(program, FUEL, common::ALL_OPT_IN).unwrap();
    let c = common::oracle(program, FUEL).unwrap();
    let (ol, cl): (Vec<_>, Vec<_>) = (ours.lines().collect(), c.lines().collect());
    let diffs: Vec<String> =
        ol.iter().zip(&cl).filter(|(a, b)| a != b).map(|(a, b)| format!("ours: {a}\n   C: {b}")).collect();
    assert!(
        diffs.is_empty() && ol.len() == cl.len(),
        "{} of {} ({} in C) differ:\n{}",
        diffs.len(),
        ol.len(),
        cl.len(),
        diffs.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// `string.format`'s whole spec matrix: every conversion, flag set, width and precision,
/// over values of each kind, the error cases included.
#[test]
fn format_spec_matrix_matches_c_lua() {
    lines_same_as_c(include_str!("libs_coverage/format_matrix.lua"));
}

/// `string.pack`/`unpack`/`packsize`: every option and size, the endianness and alignment
/// prefixes, values in and out of range, unpacking at offsets and of truncated data.
#[test]
fn pack_option_matrix_matches_c_lua() {
    lines_same_as_c(include_str!("libs_coverage/pack_matrix.lua"));
}
