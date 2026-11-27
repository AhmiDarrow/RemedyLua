//! The security and metering walk after sweep 6: work a script
//! shapes that is not charged, memory that is not accounted, and the budgets under
//! hostile output, `load`, nesting and error paths. One test per finding (`sec_m*`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Error, LimitKind, Limits, Lua, Value};

/// Peak resident memory of this process, in bytes (Linux; 0 elsewhere).
fn peak_rss() -> usize {
    status_kb("VmHWM")
}

/// A `/proc/self/status` figure, in bytes.
fn status_kb(key: &str) -> usize {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0)
        * 1024
}

/// Runs `src` under a `mem` limit in a child process (alone, so its peak memory is its
/// own): the peak resident memory past the child's baseline, and the result.
fn in_child(src: &str, mem: usize) -> (usize, String) {
    in_child_after(None, src, mem)
}

/// `in_child`, with `pre` (source, fuel) run first in the same state under that fuel.
fn in_child_after(pre: Option<(&str, u64)>, src: &str, mem: usize) -> (usize, String) {
    let _heavy = super::common::memory_heavy();
    let exe = std::env::current_exe().unwrap();
    let (pre_src, pre_fuel) = pre.unwrap_or_default();
    let out = std::process::Command::new(exe)
        .args(["sec_metering::sec_m_child", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
        .env("SECM_CHILD_PRE", pre_src)
        .env("SECM_CHILD_PRE_FUEL", pre_fuel.to_string())
        .env("SECM_CHILD_SRC", src)
        .env("SECM_CHILD_MEM", mem.to_string())
        // glibc's mmap threshold fixed (it rises as big blocks are freed, and then 1 MiB
        // blocks go on the heap among small ones and fragment it): the child's resident
        // memory is then what the state holds, not the allocator's layout (SEC-M15; the
        // same program peaked 4 to 8 MiB higher with the threshold left to move).
        .env("MALLOC_MMAP_THRESHOLD_", "131072")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let field =
        |k: &str| text.lines().find_map(|l| l.split_once(k).map(|(_, v)| v)).unwrap_or_default().to_owned();
    let peak: usize = field("SECM_PEAK=")
        .parse()
        .unwrap_or_else(|_| panic!("child failed: {text}\n{}", String::from_utf8_lossy(&out.stderr)));
    (peak, field("SECM_RESULT="))
}

/// The child of `in_child` (not a test on its own).
#[test]
#[ignore = "run by in_child"]
fn sec_m_child() {
    let Ok(src) = std::env::var("SECM_CHILD_SRC") else { return };
    let mem = std::env::var("SECM_CHILD_MEM").ok().and_then(|m| m.parse().ok()).unwrap_or(64 << 20);
    let (base, code) = (peak_rss(), status_kb("RssFile"));
    let mut lua = state(mem);
    if let Ok(pre) = std::env::var("SECM_CHILD_PRE")
        && !pre.is_empty()
    {
        let full = lua.limits();
        let fuel =
            std::env::var("SECM_CHILD_PRE_FUEL").ok().and_then(|f| f.parse().ok()).unwrap_or(full.fuel);
        lua.set_limits(full.fuel(fuel));
        println!("\nSECM_PRE={:?}", lua.exec(&pre, "=pre").map(|v| v.len()));
        lua.set_limits(full);
    }
    let r = lua.exec(&src, "=t");
    let shown = match &r {
        Ok(v) => format!("ok {:?}", v.first().and_then(|x| lua.to_string_lossy(*x))),
        Err(e) => format!("{e:?}"),
    };
    // Less the program's own code paged in as it first runs (file-backed, 1 to 2 MiB of
    // it in a debug build): that is no memory the state holds, and with it a state at its
    // limit read 66.1 MiB under 64 with 64.0 MiB of it anonymous (SEC-M15).
    let code = status_kb("RssFile").saturating_sub(code);
    println!("\nSECM_PEAK={}\nSECM_RESULT={shown}", peak_rss().saturating_sub(base).saturating_sub(code));
}

fn state(mem: usize) -> Lua {
    let mut lua = Lua::new(Limits::default().memory(mem).fuel(1 << 34), super::common::ALL_OPT_IN);
    lua.set_print(|_| {});
    lua
}

/// SEC-M1 (critical): a library function's buffer held while a callback runs was outside
/// the memory account, so callbacks nested to the depth of library calls each held one of
/// up to `string_len` bytes: `gsub`'s result while its replacement function ran, `load`'s
/// source while its reader ran, `table.concat`'s while `__index` ran, `string.format`'s
/// while `__tostring` ran, `require`'s message while a searcher ran. Each reached 0.8 to
/// 1.2 GB of process memory under a 64 MiB limit and ended with an ordinary `C stack
/// overflow`. Every such buffer is counted now: each stops at the memory limit.
#[test]
fn sec_m1_buffers_held_across_callbacks_are_counted() {
    let _heavy = super::common::memory_heavy();
    let nest = |inner: &str| {
        format!(
            "S = ('x'):rep(1 << 20) local D = 0 {inner} local ok, e = pcall(f, 190) return D .. ' ' .. tostring(e):sub(1, 40)"
        )
    };
    let cases = [
        (
            "gsub",
            nest(
                "local function f(d) local n = 0 return (('a'):rep(15):gsub('a', function() n = n + 1 \
                 if n == 15 and d > 0 then D = D + 1 f(d - 1) end return S end)) end",
            ),
        ),
        (
            "load reader",
            nest(
                "function f(d) local n = 0 return load(function() n = n + 1 if n == 16 then \
                 if d > 0 then D = D + 1 f(d - 1) end return nil end return S end) end",
            ),
        ),
        (
            "table.concat",
            nest(
                "local mt = {} function f(d) local t = setmetatable({}, mt) mt.__index = function(_, k) \
                 if k == 15 and d > 0 then D = D + 1 f(d - 1) end return S end return table.concat(t, '', 1, 15) end",
            ),
        ),
        (
            "string.format",
            nest(
                "function f(d) local o = setmetatable({}, {__tostring = function() if d > 0 then D = D + 1 f(d - 1) end \
                 return 'y' end}) return string.format(('%s'):rep(16), S, S, S, S, S, S, S, S, S, S, S, S, S, S, S, o) end",
            ),
        ),
        (
            "require",
            "S = ('x'):rep(1 << 20) package.searchers = {} local D = 0 \
             for i = 1, 15 do package.searchers[i] = function() return S end end \
             package.searchers[16] = function(n) local d = tonumber(n) if d > 0 then D = D + 1 pcall(require, tostring(d - 1)) end end \
             local ok, e = pcall(require, '190') return D"
                .to_string(),
        ),
    ];
    for (what, src) in cases {
        let mut lua = state(64 << 20);
        let r = lua.exec(&src, "=t");
        let shown = match &r {
            Ok(v) => format!("ok {:?}", v.first().and_then(|x| lua.to_string_lossy(*x))),
            Err(e) => format!("{e:?}"),
        };
        assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{what}: {shown}");
    }
    // Without nesting, the buffers are given back: the same work under the limit still
    // succeeds, and nothing stays counted after the call.
    let mut lua = state(64 << 20);
    let before = lua.memory_used();
    lua.exec(
        "local S = ('x'):rep(1 << 20) local n = 0 \
         local a = ('a'):rep(15):gsub('a', function() return S end) \
         local f = load(function() n = n + 1 if n < 16 then return '--' .. S .. '\\n' end end) \
         local c = table.concat(setmetatable({}, {__index = function() return S end}), '', 1, 15) \
         local s = string.format(('%s'):rep(15), S, S, S, S, S, S, S, S, S, S, S, S, S, S, S) \
         assert(#a == 15 << 20 and f and #c == 15 << 20 and #s == 15 << 20)",
        "=t",
    )
    .unwrap();
    lua.exec("collectgarbage() collectgarbage()", "=gc").unwrap();
    let after = lua.memory_used();
    assert!(after < before + (1 << 20), "{before} before, {after} after");
}

/// SEC-M1, the same through results: `require` kept every result of the module's loader
/// while it stored the module in `package.loaded`, whose `__newindex` (a script's) may
/// `require` again: a loader returning a million values held 16 MB outside the account at
/// each level, 1.1 GB of process memory under a 64 MiB limit. Only the first result is
/// kept now.
#[test]
fn sec_m1_require_keeps_one_loader_result() {
    let (peak, r) = in_child(
        "local big = {} for i = 1, 1 << 20 do big[i] = i end local D = 0 \
         package.searchers = {function(n) return function() return table.unpack(big) end end} \
         setmetatable(package.loaded, {__newindex = function(t, k, v) local d = tonumber(k) \
           if d > 0 then D = D + 1 pcall(require, tostring(d - 1)) end rawset(t, k, v) end}) \
         pcall(require, '190') return D",
        64 << 20,
    );
    assert!(peak < 400 << 20, "{} MiB of process memory under a 64 MiB limit ({r})", peak >> 20);
}

/// SEC-M2 (medium): a prototype was charged by the lengths of its code, line, constant and
/// debug vectors while it kept them at their capacity, grown by doubling: functions just
/// past a power of two of instructions held up to twice what was charged (chunks kept
/// until memory ran out reached 116 MB under a 64 MiB limit; 83 MB now, the rest the
/// compile's own transients). Each vector is held at its length now.
#[test]
fn sec_m2_prototypes_hold_what_they_are_charged() {
    let (peak, r) = in_child(
        "local src = 'local x = 0 ' .. ('x = x + 1 '):rep(4100) local F = {} local n = 0 \
         while true do n = n + 1 F[n] = load(src) end",
        32 << 20,
    );
    assert!(r.contains("Limit(Memory)"), "{r}");
    assert!(peak < 50 << 20, "{} MiB of process memory under a 32 MiB limit", peak >> 20);
}

/// SEC-M3 (high): `table.unpack` checked that its values fit in memory, gave the room back,
/// then gathered them in a list outside the account while `__index` ran; an `__index` that
/// unpacked the same table again held one such list per level: 2 GB of process memory
/// under a 64 MiB limit (bounded by fuel, 16 bytes a unit: a host's larger fuel made it
/// worse). The list counts against memory until its values move to the stack.
#[test]
fn sec_m3_unpack_counts_the_values_it_gathers() {
    let _heavy = super::common::memory_heavy();
    let mut lua = state(64 << 20);
    let r = lua.exec(
        "local N = 1000000 local D = 0 local t \
         t = setmetatable({}, {__index = function(_, k) if k == N and D < 40 then D = D + 1 \
           pcall(table.unpack, t, 1, N) end return k end}) \
         pcall(table.unpack, t, 1, N) return D",
        "=t",
    );
    assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{r:?}");
    // Without nesting, the same unpack fits as before.
    let r = lua.exec(
        "local t = setmetatable({}, {__index = function(_, k) return k end}) \
         return select('#', table.unpack(t, 1, 1000000))",
        "=t",
    );
    assert!(matches!(r.as_deref(), Ok([remedy_lua::Value::Int(1_000_000)])), "{r:?}");
}

/// How many times one call of `body` (after `setup`) reads the time limit's clock.
fn clock_reads(setup: &str, body: &str) -> usize {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut lua = state(256 << 20);
    lua.exec(setup, "=setup").unwrap();
    let f = lua.load(body, "=body").unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let r2 = Arc::clone(&reads);
    let clock: remedy_lua::Clock = Arc::new(move || {
        r2.fetch_add(1, Ordering::Relaxed);
        std::time::Duration::ZERO
    });
    lua.set_time_limit(Some((clock, std::time::Duration::from_secs(3600))));
    lua.call(f.value(), &[]).unwrap();
    reads.load(Ordering::Relaxed)
}

/// SEC-M4 (medium): byte-by-byte library loops over a long string looked at the host's
/// interrupt and deadline only before and after: on a 16 MiB string (the default string
/// limit) `string.format('%q')` stopped 29 ms past a time limit, `utf8.len` and
/// `string.reverse` 12 ms, `upper` and `lower` 7 ms (the reference promises a fraction of
/// a millisecond of work). Each looks every megabyte or so now: within 0.5 to 3 ms. Checked
/// by the looks themselves (the clock is read at each), which does not depend on the
/// machine's speed.
#[test]
fn sec_m4_long_string_loops_look_at_the_deadline() {
    let setup = "S = ('x'):rep((16 << 20) - 16)";
    let base = clock_reads(setup, "local n = #S");
    for body in ["string.format('%q', S)", "utf8.len(S)", "S:reverse()", "S:upper()", "S:lower()"] {
        let reads = clock_reads(setup, &format!("local r = {body}"));
        assert!(reads >= base + 8, "{body}: the clock was read {reads} times ({base} for #S)");
    }
    // The chunks join up: the same strings as one pass makes.
    let mut lua = state(256 << 20);
    let r = lua
        .exec(
            "local parts = {} for i = 1, 300000 do parts[i] = string.char(65 + i % 26, 97 + i % 7, 48 + i % 10, 32) end \
             local s = table.concat(parts) local r = s:reverse() \
             for i = 1, #s, 4099 do assert(r:byte(#s - i + 1) == s:byte(i)) end \
             assert(s:upper():lower() == s:lower() and s:lower():upper() == s:upper()) \
             assert(r:reverse() == s and utf8.len(s) == #s) return #s",
            "=t",
        )
        .unwrap();
    assert!(matches!(r.as_slice(), [remedy_lua::Value::Int(1_200_000)]), "{r:?}");
}

/// A block of `n` locals and a closure over each, made from the last local down (`down`)
/// or from the first up.
fn closures_over_locals(n: usize, down: bool) -> String {
    let names: Vec<String> = (1..=n).map(|i| format!("a{i}")).collect();
    let mut src = format!("local {} = 1 ", names.join(", "));
    let order: Vec<usize> = if down { (1..=n).rev().collect() } else { (1..=n).collect() };
    for i in order {
        src.push_str(&format!("local f{i} = function() return a{i} end "));
    }
    src
}

/// SEC-M6 (critical): a thread's open upvalues are a list sorted by stack slot, and making
/// one below others already open moved them all up, uncharged: a frame of 8,000 locals
/// (registers past 255 are a RemedyLua superset) closed over from the last local down ran
/// at 22x plain code per unit of fuel (the same closures made from the first local up,
/// 4.5x), growing with the frame (up to 32,768 registers). The entries moved are paid now,
/// on both paths from one count: about 1x.
#[test]
fn sec_m6_open_upvalues_moved_are_charged() {
    super::common::small_stack(|| {
        let _m = super::common::measuring();
        let cpu = |down: bool, fast: bool| {
            let body = closures_over_locals(6000, down);
            let mut best = f64::MAX;
            for _ in 0..3 {
                let mut lua = state(512 << 20);
                lua.set_fast_loop(fast);
                lua.set_limits(lua.limits().fuel(4_000_000));
                let f = lua.load(format!("while true do {body} end"), "=t").unwrap();
                let t = remedy_lua_oracle::thread_cpu();
                let r = lua.call(f.value(), &[]);
                let spent = remedy_lua_oracle::thread_cpu().saturating_sub(t) as f64;
                assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{r:?}");
                best = best.min(spent / (4_000_000 - lua.fuel_left()) as f64);
            }
            best
        };
        for fast in [true, false] {
            let ratio = cpu(true, fast) / cpu(false, fast);
            assert!(
                ratio < 2.0,
                "fast {fast}: closing over locals from the last down costs {ratio:.1}x per unit what from the first up does"
            );
        }
        // The same fuel with the fast loop on and off and under any seed.
        let src = format!("for r = 1, 3 do {} end", closures_over_locals(700, true));
        let fuel = |seed: u64, fast: bool| {
            let mut lua = Lua::new(Limits::default().fuel(1 << 40).hash_seed(seed), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(&src, "=t").unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        let f0 = fuel(1, true);
        for (seed, fast) in [(1, false), (2, true), (0xdead_beef, false), (u64::MAX, true)] {
            assert_eq!(fuel(seed, fast), f0, "seed {seed}, fast {fast}");
        }
    });
}

/// CPU per unit of fuel of `while true do <body> end` after `setup` (the best of three),
/// with the fast loop on or off.
fn cpu_per_unit(setup: &str, body: &str, fuel: u64, fast: bool) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let mut lua = state(512 << 20);
        lua.set_fast_loop(fast);
        lua.exec(setup, "=setup").unwrap();
        lua.set_limits(lua.limits().fuel(fuel));
        let f = lua.load(format!("while true do {body} end"), "=t").unwrap();
        let t = remedy_lua_oracle::thread_cpu();
        let r = lua.call(f.value(), &[]);
        let spent = remedy_lua_oracle::thread_cpu().saturating_sub(t) as f64;
        assert!(matches!(r, Err(Error::Limit(LimitKind::Fuel))), "{body}: {r:?}");
        best = best.min(spent / (fuel - lua.fuel_left()).max(1) as f64);
    }
    best
}

/// SEC-M7 (critical): instructions and calls that write a list of registers counted as
/// one step however long the list, and frames past 255 registers (a RemedyLua superset)
/// make it up to 65,536: with 30,000 locals, `local a1, ..., a30000` (one `LOADNIL`) ran at
/// 80x plain code per unit of fuel in the fast loop and 241x on the full path, `= ...`
/// (`VARARG` with a fixed count) 239x and 255x, the same locals from one call's results
/// (padded with nils up to the count wanted, by the Lua return's shortcut, a native's
/// return, `move_results`) 46x to 70x, a generic `for` with 30,000 variables 48x to 77x,
/// a call (or tail call, in the fast loop) that leaves 30,000 parameters missing likewise. Each is paid now past 16 values,
/// a unit per 8, as argument and result lists already were (the fast loop leaves those to
/// the full path, so fuel stays the same either way): 0.2x to 0.6x.
#[test]
fn sec_m7_register_lists_are_charged() {
    super::common::small_stack(|| {
        let _m = super::common::measuring();
        let n = 10_000;
        let names = |p: &str| (1..=n).map(|i| format!("{p}{i}")).collect::<Vec<_>>().join(", ");
        let (a, p) = (names("a"), names("p"));
        let setup = format!(
            "function NILS() local {a} end \
             function VARARGS(...) local {a} = ... end \
             function ONE() return 1 end function ADJUST() local {a} = ONE() end \
             function IT(s, c) if c == nil then return 1 end end function TFOR() for {a} in IT do end end \
             function PARAMS({p}) end function MISSING() PARAMS() end function TAIL() return PARAMS() end \
             T = {{}} function NATIVE() T[1] = 1 local {a} = table.remove(T) end \
             function MATH() local {a} = math.abs(1) end \
             CO = coroutine.wrap(function() while true do coroutine.yield(1) end end) function RESUME() local {a} = CO() end"
        );
        let plain = |fast: bool| cpu_per_unit("I = 0", "I = I + 1 local x = I * 2", 2_000_000, fast);
        let mut slow = Vec::new();
        for fast in [true, false] {
            let base = plain(fast);
            for f in ["NILS", "VARARGS", "ADJUST", "TFOR", "MISSING", "TAIL", "NATIVE", "MATH", "RESUME"] {
                let ratio = cpu_per_unit(&setup, &format!("{f}()"), 2_000_000, fast) / base;
                if ratio > 5.0 {
                    slow.push(format!("fast {fast}: {f}() ran at {ratio:.1}x plain code per unit of fuel"));
                }
            }
        }
        assert!(slow.is_empty(), "{slow:#?}");
        // The same fuel with the fast loop on and off and under any seed, and as before for
        // lists of 16 or fewer (`local a1, ..., a16` and its friends are free as they were).
        let small = (1..=16).map(|i| format!("b{i}")).collect::<Vec<_>>().join(", ");
        let src = format!(
            "{setup} local function S() local {small} local {small} = ONE() for {small} in IT do end end \
             for r = 1, 3 do NILS() VARARGS(1, 2) ADJUST() TFOR() MISSING() TAIL() NATIVE() MATH() RESUME() S() end"
        );
        let fuel = |seed: u64, fast: bool| {
            let mut lua = Lua::new(Limits::default().fuel(1 << 40).hash_seed(seed), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(&src, "=t").unwrap();
            (1u64 << 40) - lua.fuel_left()
        };
        let f0 = fuel(1, true);
        for (seed, fast) in [(1, false), (2, true), (0xdead_beef, false), (u64::MAX, true)] {
            assert_eq!(fuel(seed, fast), f0, "seed {seed}, fast {fast}");
        }
    });
}

/// SEC-M8 (high): `print` and `warn` join their arguments before writing them, bounded by
/// the output limit only: a host that lifts `Limits::output` (keeping the memory limit)
/// let `warn(table.unpack(t))`, with `t` holding one 4 MiB string a hundred times (a
/// hundred slots of memory), build a 400 MB message outside the account (and `print`
/// alike, before SEC-M1's count of its line). Both lines count against memory now.
#[test]
fn sec_m8_print_and_warn_lines_are_counted() {
    let _heavy = super::common::memory_heavy();
    for f in ["print", "warn"] {
        let mut lua = Lua::new(
            Limits::default().memory(64 << 20).output(usize::MAX).fuel(1 << 34),
            super::common::ALL_OPT_IN,
        );
        lua.set_print(|_| {});
        let r = lua.exec(
            format!("warn('@on') local s = ('x'):rep(4 << 20) local t = {{}} for i = 1, 100 do t[i] = s end {f}(table.unpack(t))"),
            "=t",
        );
        assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{f}: {r:?}");
    }
}

/// The child of `sec_lr17_*`: makes 200,000 objects of `SECM_HOST_KIND` from the host,
/// keeps them alive, and prints resident memory gained against memory counted, per object.
#[test]
#[ignore = "run by sec_lr17_host_objects_are_charged_what_they_take"]
fn sec_m_host_child() {
    let Ok(kind) = std::env::var("SECM_HOST_KIND") else { return };
    let statm = || {
        std::fs::read_to_string("/proc/self/statm")
            .ok()
            .and_then(|s| s.split_whitespace().nth(1).and_then(|x| x.parse::<usize>().ok()))
            .unwrap_or(0)
            * 4096
    };
    let n = 200_000usize;
    let mut lua = Lua::new(Limits::default().memory(1 << 30).fuel(1 << 40), remedy_lua::Libs::ALL);
    let keep = lua.create_table().unwrap();
    let _keep = lua.root(remedy_lua::Value::Table(keep));
    let mut roots = Vec::with_capacity(n);
    lua.collect_garbage().unwrap();
    let (r0, m0) = (statm(), lua.memory_used());
    for i in 0..n {
        let v = match kind.as_str() {
            "userdata" | "userdata+root" => lua.create_userdata(i as u64, None).unwrap(),
            "tracked userdata" => lua.create_userdata_tracked(i as u64, None, i as u64).unwrap(),
            "host function" | "host function+root" => {
                lua.create_function("f", move |_, _| Ok(vec![remedy_lua::Value::Int(i as i64)])).unwrap()
            }
            "register" => {
                lua.register(&format!("g{i}"), move |_, _| Ok(vec![remedy_lua::Value::Int(i as i64)]))
                    .unwrap();
                remedy_lua::Value::Nil
            }
            other => panic!("{other}"),
        };
        if kind.ends_with("+root") {
            roots.push(lua.root(v));
        }
        if !v.is_nil() {
            lua.table_set(keep, remedy_lua::Value::Int(i as i64 + 1), v).unwrap();
        }
        if i % 4096 == 0 {
            lua.exec("", "=s").unwrap();
        }
    }
    lua.exec("", "=s").unwrap();
    lua.collect_garbage().unwrap();
    let (r1, m1) = (statm(), lua.memory_used());
    println!("\nSECM_PEAK={}\nSECM_RESULT={}", r1.saturating_sub(r0) / n, m1.saturating_sub(m0) / n);
    drop(roots);
}

/// LR-17 (from the long fuzz shards; high): objects the host makes were charged less than
/// they take. A userdata was a flat 48 bytes plus its declared size against about 90 (its
/// record, slot and box), a host function 80 against about 120 (its closure's and name's
/// allocations), and a root nothing against about 60 (its `Arc` and list entry): 200,000
/// live userdata with roots held 2.6x their count, host functions with roots 2.2x, so
/// garbage between collections peaked at about 4x an 8 MiB budget. Each is charged what
/// it takes now; per kind, resident memory is within 1.3x of the count.
#[test]
fn sec_lr17_host_objects_are_charged_what_they_take() {
    let _heavy = super::common::memory_heavy();
    let mut bad = Vec::new();
    for kind in
        ["userdata", "tracked userdata", "userdata+root", "host function", "host function+root", "register"]
    {
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args([
                "sec_metering::sec_m_host_child",
                "--exact",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("SECM_HOST_KIND", kind)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let field = |k: &str| -> f64 {
            text.lines()
                .find_map(|l| l.split_once(k).map(|(_, v)| v))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or_else(|| {
                    panic!("child {kind} failed: {text}\n{}", String::from_utf8_lossy(&out.stderr))
                })
        };
        let (resident, counted) = (field("SECM_PEAK="), field("SECM_RESULT="));
        if resident > 1.3 * counted {
            bad.push(format!(
                "{kind}: {resident} B resident, {counted} B counted per object ({:.2}x)",
                resident / counted.max(1.0)
            ));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

/// SEC-M9 (medium): a closure's list of upvalue references was charged 8 bytes a
/// reference, against its own allocation (header and rounding): a closure over one fresh
/// upvalue held 32 bytes there for 8 charged, so 1.6 million live closures were 1.39x
/// resident to counted (1.42x at 800,000). Each list is charged what it takes now (as a
/// library function's list is): within 1.3x.
#[test]
fn sec_m9_closures_are_charged_what_they_take() {
    for (what, body) in [
        ("one fresh upvalue", "t[i] = function() return i end"),
        ("three fresh upvalues", "local a, b, c = i, i, i t[i] = function() return a + b + c end"),
        ("a shared upvalue", "t[i] = function() return up end"),
    ] {
        let (peak, counted) = in_child(
            &format!(
                "local up = 1 local t = {{}} for i = 1, 1600000 do {body} end \
                 collectgarbage() collectgarbage() return tostring(collectgarbage('count') * 1024 // 1)"
            ),
            1 << 30,
        );
        let counted: f64 = counted
            .trim_start_matches("ok Some(\"")
            .trim_end_matches("\")")
            .parse()
            .unwrap_or_else(|_| panic!("{what}: {counted}"));
        let ratio = peak as f64 / counted;
        assert!(
            ratio < 1.3,
            "{what}: {} MiB resident for {} MiB counted ({ratio:.2}x)",
            peak >> 20,
            counted as usize >> 20
        );
    }
}

/// SEC-M10 (low): LR-8 keeps closed and finished coroutines' stacks for new coroutines (at
/// most 8 of 2,048 slots, a sixteenth of the limit), meant to be counted in the account's
/// free-slot part (`Mem::slack`, inside `memory_used`). Retiring a stack took it out of
/// `memory_used` and put it in the slack only, so the spare stacks were resident and
/// uncounted, and the live figure the collector paces on came out that much low. They stay
/// counted now until a new coroutine takes one or a full collection drops them.
#[test]
fn sec_m10_spare_coroutine_stacks_stay_counted() {
    let mut lua = state(64 << 20);
    lua.exec("collectgarbage('stop')", "=t").unwrap();
    let before = lua.memory_used();
    // Eight coroutines, all alive at once, grow their stacks past a window, then finish.
    lua.exec(
        "local function deep(n) if n == 0 then return 0 end return 1 + deep(n - 1) end \
         local cos = {} for i = 1, 8 do cos[i] = coroutine.create(function() deep(60) coroutine.yield() end) \
           assert(coroutine.resume(cos[i])) end \
         for i = 1, 8 do assert(coroutine.resume(cos[i])) assert(coroutine.status(cos[i]) == 'dead') end",
        "=t",
    )
    .unwrap();
    let held = lua.memory_used();
    lua.collect_garbage().unwrap();
    let after = lua.memory_used();
    // With the spare stacks dropped by the full collection, at least their slots go.
    assert!(
        held >= after + 8 * 256 * 16,
        "{before} before, {held} with spare stacks, {after} after a collection"
    );
}

/// SEC-M12 (medium): `string.pack`, `packsize` and `unpack` copied their format (to its
/// first NUL) and read it item by item without a look at the host's interrupt or deadline,
/// and `unpack`'s `z` searched for its end in one go: a 16 MiB format ran 14 to 22 ms past
/// a 5 ms time limit, a 16 MiB `z` string 9 ms. The format is copied a megabyte at a time
/// and read with a look every 64 KiB, a `z` string's end found a megabyte at a time: 0.7
/// to 1.3 ms now. Checked by the clock's reads, as SEC-M4.
#[test]
fn sec_m12_pack_formats_look_at_the_deadline() {
    let _heavy = super::common::memory_heavy();
    let setup = "F = ('x'):rep(16 << 20) Z = ('q'):rep((16 << 20) - 8) .. '\\0'";
    let base = clock_reads(setup, "local n = #F");
    for body in ["string.pack(F)", "string.packsize(F)", "string.unpack('z', Z)"] {
        let reads = clock_reads(setup, &format!("local r = {body}"));
        assert!(reads >= base + 8, "{body}: the clock was read {reads} times ({base} for #F)");
    }
    // The same results as one pass gives.
    let mut lua = state(256 << 20);
    let r = lua
        .exec(
            "local f = ('i4'):rep(300000) local v = {} for i = 1, 300000 do v[i] = i end \
             local s = string.pack(f, table.unpack(v)) assert(#s == 1200000 and string.packsize(f) == 1200000) \
             local u = {string.unpack(f, s)} assert(u[1] == 1 and u[300000] == 300000 and u[300001] == 1200001) \
             local z = ('ab'):rep(1500000) .. '\\0rest' local a, n = string.unpack('z', z) \
             assert(#a == 3000000 and n == 3000002) return true",
            "=t",
        )
        .unwrap();
    assert_eq!(r, [remedy_lua::Value::Bool(true)]);
}

/// FX2-5 (sweep 6 review, S6T-LOAD): a `load` of a
/// 16 MiB string literal had about 20 ms with no look at the deadline. The lexer read the
/// whole token in one go, the parser copied it into the tree, `token_end` walked its text
/// at every `last_end` (22% of the load), and the compiler hashed it for its constant map,
/// compared it with an equal interned string and copied it for the map, each in one go.
/// Now the lexer looks every 64 KiB, the token's bytes are moved and its end is known, and
/// a long constant is settled a piece at a time, as a long key is. Counted by the looks:
/// at least 300 looks per load of a 16 MiB literal (146 before, 401 now).
#[test]
fn fx2_5_a_long_literal_is_compiled_between_looks() {
    let setup = "S = string.rep('a', (16 << 20) - 64) SRC = 'return \"' .. S .. '\"'";
    let base = clock_reads(setup, "local f = load('return 1')");
    // (Copying the chunk and counting its tokens look every 256 KiB: 146 reads before.)
    for (body, loads) in [("local f = load(SRC)", 1), ("local f = load(SRC) local g = load(SRC)", 2)] {
        let reads = clock_reads(setup, body);
        assert!(
            reads >= base + 300 * loads,
            "{body}: the clock was read {reads} times ({base} for a short load)"
        );
    }
    // The constant is the interned string, equal to (and the same key as) S.
    let mut lua = state(256 << 20);
    let r =
        lua.exec(format!("{setup} local t = {{[S] = 1}} local s = load(SRC)() return s == S and t[s]"), "=t");
    assert!(matches!(r.as_deref(), Ok([remedy_lua::Value::Int(1)])), "{r:?}");
}

/// SEC7-N3 (medium): the lexer looked at the deadline only in strings: a 16 MiB comment,
/// name, numeral or run of blanks was read in one go (23 to 46 ms with no look). They look
/// every 64 KiB now, as long strings do: 200 looks more than a short load (about 130 more
/// before, all from copying the chunk).
#[test]
fn sec7_n3_comments_names_numerals_and_blanks_look_at_the_deadline() {
    let base = clock_reads("", "local f = load('return 1')");
    for (pre, c) in [("--", 'a'), ("", ' '), ("return ", 'a'), ("return ", '1')] {
        let setup = format!("SRC = '{pre}' .. string.rep('{c}', (16 << 20) - 64)");
        let reads = clock_reads(&setup, "local f = load(SRC)");
        assert!(reads >= base + 200, "{pre}{c}: the clock was read {reads} times ({base} for a short load)");
    }
}

/// LR-26 (critical): the fast loop's generic `for` over `next` looked up a long key's
/// interned copy by walking the interner, then left the instruction to the full path when
/// the key was in no table: the walk's steps were dropped with it, but the copy it found
/// stayed known, so the full path's `next` found it free. Fuel differed between the fast
/// loop and the full path (355 against 364 at 600 bytes, more with length).
#[test]
fn sec_lr26_a_long_key_given_to_next_costs_the_same_in_both_loops() {
    // 600 and 10,000 bytes; the key in no table (an error, caught) and in one (found).
    for n in [300, 5000] {
        let setup = format!("local mt = {{[string.rep('ab', {n})] = 1}} local k = string.rep('ab', {n})");
        for body in [
            "pcall(function() for v in next, {}, k do end end) return mt.z",
            "for v in next, mt, k do end return mt.z",
        ] {
            // (`remedy` checks result, fuel and memory are the same in both loops.)
            assert_eq!(
                super::common::remedy(&format!("{setup} {body}"), 1 << 20),
                Ok("nil".into()),
                "{n}: {body}"
            );
        }
    }
}

/// SEC-M13 (critical): while `load` called its reader, the syntax tree built so far was in
/// no account, so the reader's allocations came on top of it, and a reader calling `load`
/// with another reader gave the inner parse a room of its own beside the outer tree: 20
/// levels of 40,000 table constructors loaded under a 64 MiB limit at a peak of 135 MiB,
/// 60 levels at 333 MiB (to the C stack's limit). The tree now counts as used while the
/// reader runs, and the parse goes on in the room the reader left.
#[test]
fn sec_m13_a_reader_runs_with_the_tree_counted() {
    // Each reader notes the most memory counted while it runs (`PEAK`, read after).
    let nest = |n: usize, d: usize| {
        format!(
            "PEAK = 0 local piece = ('{{}},'):rep(4096) \
             local function mk(depth) local i = 0 return function() i = i + 1 \
               PEAK = math.max(PEAK, collectgarbage('count') * 1024) \
               if i == 1 then return 'return {{' end if i <= {n} then return piece end \
               if i == {n} + 1 then if depth > 0 then local f, e = load(mk(depth - 1)) assert(f, e) end return '}}' end \
             end end \
             local f, e = load(mk({d})) return f ~= nil, e"
        )
    };
    let run = |n: usize, d: usize| {
        let runs: Vec<_> = [true, false]
            .into_iter()
            .map(|fast| {
                let mut lua = state(64 << 20);
                lua.set_fast_loop(fast);
                let r = lua.exec(&nest(n, d), "=t").map_err(|e| e.to_string());
                let peak = lua.exec("return PEAK", "=p").unwrap()[0];
                let Value::Num(peak) = peak else { panic!("{peak:?}") };
                assert!(peak <= (64 << 20) as f64, "{n} x {d}: {peak} counted");
                (r, lua.fuel_left())
            })
            .collect();
        assert_eq!(runs[0], runs[1], "{n} x {d}: the fast loop and the full path differ");
        runs[0].0.clone()
    };
    // One level in the room loads, as before.
    assert_eq!(run(50, 0), Ok(vec![Value::Bool(true), Value::Nil]));
    // Nested, the trees add up: past the limit, the memory error (they loaded before).
    let memory = Err(Error::Limit(LimitKind::Memory).to_string());
    for d in [1, 3, 6, 20] {
        let r = run(10, d);
        assert!(d < 6 || r == memory, "{d}: {r:?}");
    }
}

/// The memory limit holds in the account at every call of `load`'s reader (with the parse's
/// held bytes counted, SEC-M13), here for a reader that keeps a 1 MiB string every other
/// call while it feeds statements, until the limit stops it. (The process's resident
/// memory can be higher: glibc's fragmentation, documented in the reference.)
#[test]
fn fx2_7_memory_used_holds_at_every_reader_call() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let limit = 64 << 20;
    let worst = Arc::new(AtomicUsize::new(0));
    let w = Arc::clone(&worst);
    let mut lua = state(limit);
    lua.register("used", move |lua, _| {
        w.fetch_max(lua.memory_used(), Ordering::Relaxed);
        Ok(vec![])
    })
    .unwrap();
    let r = lua.exec(
        "local keep, i = {}, 0 local piece = ('if x then y = 1 end '):rep(1024) \
         load(function() used() i = i + 1 \
           if i % 2 == 0 then keep[#keep + 1] = string.rep('x', (1 << 20) + i) end \
           if i <= 3000 then return piece end end)",
        "=t",
    );
    assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{r:?}");
    let worst = worst.load(Ordering::Relaxed);
    assert!(worst <= limit, "memory_used reached {worst} under a limit of {limit}");
    assert!(worst > limit / 2, "the reader ran with {worst} used: not near the limit");
}

/// SEC-M14 (high): natives that make no callback grew their output, or copied an argument,
/// before counting it, so one call held up to a string's length (and its doubling) past
/// the memory limit until its next payment. Under 48 MiB with 41 MiB live, peak process
/// memory past the baseline was: `gsub` of one match with 15 copies of a 1 MiB `%0`, 56
/// MiB; `format('%q')` of 6 MiB, 59; a 6 MiB literal format, 54; `os.date` of a 6 MiB
/// format (copied twice), 60; `pack`'s 6 MiB format, 48 (at the edge). Each output's room
/// is now paid for before it is allocated (`MemHold::reserve`), each copy counted while
/// held: each stops at the limit. Results and fuel are as before.
#[test]
fn sec_m14_outputs_are_paid_for_before_they_grow() {
    let live = "KEEP = {} for i = 1, 40 do KEEP[i] = ('y'):rep((1 << 20) + i) end ";
    for body in [
        "local s = ('x'):rep(1 << 20) local r = s:gsub('^.+', ('%0'):rep(15))",
        "local s = ('x\\0'):rep(3 << 20) local r = string.format('%q', s)",
        "local F = ('z'):rep(6 << 20) local r = string.format(F)",
        "local F = ('z'):rep(6 << 20) local r = os.date(F)",
        "local F = ('z'):rep(6 << 20) local r = string.pack(F)",
    ] {
        let (peak, r) = in_child(&format!("{live}{body}"), 48 << 20);
        assert!(r.contains("Memory"), "{body}: {r}");
        assert!(peak <= 50 << 20, "{body}: peak {} MiB under a 48 MiB limit", peak >> 20);
    }
}

/// SEC-M15 (high): a native's long list of results outlived its call outside the account.
/// `rets` kept its capacity after the results were placed (and a coroutine's results
/// went back into it), so after `{s:byte(1, -1)}` of 900,000 bytes 14 MiB stayed
/// uncounted for good: the script then filled the limit beside it, 78 MiB past the
/// baseline under 64 MiB. `string.byte` and `utf8.codepoint` also built the list after
/// giving back its charge. The list is held counted while built and placed, and a long
/// one is not kept. Fuel is as before (the collector's debt unchanged).
#[test]
fn sec_m15_long_result_lists_are_not_kept() {
    for mk in [
        "{s:byte(1, -1)}",
        "{utf8.codepoint(s, 1, -1)}",
        "{coroutine.wrap(function() return s:byte(1, -1) end)()}",
        "{coroutine.wrap(function() coroutine.yield(s:byte(1, -1)) end)()}",
    ] {
        let src = format!(
            "local s = ('x'):rep(900000) local t = {mk} assert(#t == 900000) t = nil s = nil collectgarbage() \
             KEEP = {{}} for i = 1, 60 do KEEP[i] = ('y'):rep((1 << 20) + i) end"
        );
        let (peak, r) = in_child(&src, 64 << 20);
        assert!(r.contains("Memory"), "{mk}: {r}");
        assert!(peak <= 66 << 20, "{mk}: peak {} MiB under a 64 MiB limit", peak >> 20);
    }
}

/// SEC7-1 (high): a long result list an error cut short stayed outside the account. A
/// native's list left in `rets` when it failed (`table.unpack`'s plain table stopped by a
/// limit as it read, `string.byte` stopped at the deadline, `place_rets`' own memory
/// error) kept its capacity across host calls: after `table.unpack({}, 1, 3500000)` was
/// stopped, a script then filling a 64 MiB limit peaked at 103 MiB. The list is not kept
/// past a failed call, and `table.unpack` reserves the room it checked, not twice it.
#[test]
fn sec7_1_a_list_cut_short_is_not_kept() {
    let fill = "KEEP = {} for i = 1, 70 do KEEP[i] = ('y'):rep((1 << 20) + i) end";
    // `string.byte` stopped by fuel at its results' charge, once it had returned.
    for (pre, fuel) in [("return select('#', ('x'):rep(3500000):byte(1, -1))", 1_000_000)] {
        let (peak, r) = in_child_after(Some((pre, fuel)), fill, 64 << 20);
        assert!(r.contains("Memory"), "{pre}: {r}");
        assert!(peak <= 66 << 20, "{pre}: peak {} MiB under a 64 MiB limit", peak >> 20);
    }
}

/// SEC7-N1 (high): `string.unpack`'s values were built in a list outside the account and
/// then copied into the results, so the limit was met only once both were made: a format
/// of 3M 'c0' items (no data read) peaked at 108 MiB under 48, 8M at 288. The values go
/// into the results counted as the list grows.
#[test]
fn sec7_n1_unpacked_values_are_counted_as_made() {
    for body in [
        "local n = select('#', string.unpack(('c0'):rep(3 << 20), ''))",
        "local n = select('#', string.unpack(('b'):rep(3 << 20), ('\\1'):rep(3 << 20)))",
    ] {
        let (peak, r) = in_child(body, 48 << 20);
        assert!(r.contains("Memory"), "{body}: {r}");
        assert!(peak <= 50 << 20, "{body}: peak {} MiB under a 48 MiB limit", peak >> 20);
    }
    // Within the limit, the same values.
    let mut lua = state(64 << 20);
    let r = lua.exec("local t = {string.unpack('<i2 b c0 s1', '\\1\\0\\255\\3abc')} return #t .. ' ' .. table.concat(t, ',')", "=t").unwrap();
    assert_eq!(r.first().and_then(|v| lua.to_string_lossy(*v)).as_deref(), Some("5 1,-1,,abc,8"));
}

/// SEC7-N2 (high): output grown before it was counted, still, after SEC-M14: gsub's tail
/// copy and a function's or table's replacement (copied whole, with no look at the stop
/// either), and `string.pack`'s 's', 'z' and 'c' items. Under 48 MiB with 41 MiB live each
/// peaked at 51.6 to 51.9 MiB. Each is checked and paid for before it is copied.
#[test]
fn sec7_n2_copies_are_paid_for_before_they_grow() {
    let live = "KEEP = {} for i = 1, 40 do KEEP[i] = ('y'):rep((1 << 20) + i) end ";
    for body in [
        "local s = ('x'):rep(6 << 20) local r = s:gsub('x', 'y', 1)",
        "local s = ('x'):rep(6 << 20) local r = s:gsub('^x', 'y')",
        "local big = ('z'):rep(6 << 20) local r = ('ab'):gsub('a', function() return big end)",
        "local big = ('z'):rep(6 << 20) local r = ('ab'):gsub('a', {a = big})",
        "local big = ('z'):rep(6 << 20) local r = string.pack('s4', big)",
        "local big = ('z'):rep(6 << 20) local r = string.pack('z', big)",
        "local r = string.pack('c' .. (12 << 20), '')",
    ] {
        let (peak, r) = in_child(&format!("{live}{body}"), 48 << 20);
        assert!(r.contains("Memory"), "{body}: {r}");
        assert!(peak <= 50 << 20, "{body}: peak {} MiB under a 48 MiB limit", peak >> 20);
    }
}
