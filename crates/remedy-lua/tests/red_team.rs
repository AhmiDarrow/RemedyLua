//! Adversarial tests: every attack a red team found against an earlier C Lua sandbox,
//! ported, plus RemedyLua's own. Each must end quickly with an error (or a correct
//! result), never hang, crash or escape its limits, and the state must stay usable.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use std::time::{Duration, Instant};

use remedy_lua::{Data, DataLimits, Error, LimitKind, Libs, Limits, Lua, Value};

fn quick<T>(f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    // Debug builds are slow; the budgets below are sized so release takes milliseconds.
    assert!(start.elapsed() < Duration::from_secs(20), "took {:?}", start.elapsed());
    out
}

fn sb() -> Lua {
    Lua::new(Limits::default().fuel(1_000_000).memory(16 << 20), Libs::SAFE)
}

fn alive(lua: &mut Lua) {
    let r = lua.exec("return 1 + 1", "=t").unwrap();
    assert_eq!(r, vec![Value::Int(2)]);
}

fn err_msg(r: Result<Vec<Value>, Error>) -> String {
    match r {
        Err(Error::Runtime { message, .. }) => message,
        other => format!("{other:?}"),
    }
}

const FUEL: Result<Vec<Value>, Error> = Err(Error::Limit(LimitKind::Fuel));
const MEMORY: Result<Vec<Value>, Error> = Err(Error::Limit(LimitKind::Memory));

// ---- ported from the C Lua sandbox's red team ----

#[test]
fn finalizers_are_refused() {
    let mut s = sb();
    let m = err_msg(quick(|| s.exec("setmetatable({}, {__gc = function() while true do end end})", "=t")));
    assert!(m.contains("__gc"), "{m}");
    alive(&mut s);
}

#[test]
fn single_library_calls_cannot_run_forever() {
    let mut s = sb();
    let r = quick(|| s.exec("return string.rep('', 1 << 62)", "=t")).unwrap();
    assert_eq!(s.to_string_lossy(r[0]).unwrap(), "");
    assert_eq!(quick(|| s.exec("table.move({}, 1, 1 << 50, 2)", "=t")), FUEL);
    let m = err_msg(quick(|| s.exec("return string.find(string.rep('a', 40), string.rep('a*', 40) .. 'b')", "=t")));
    // Stopped by the step limit, or by the fuel first: a match's steps are paid for and
    // capped by the fuel left (sweep 2, SEC2-6), and 1M fuel buys fewer steps than the cap.
    assert!(m.contains("pattern too complex") || m == "Err(Limit(Fuel))", "{m}");
    let len = "local t = setmetatable({}, {__len = function() return 1 << 50 end, __index = function() return 'x' end})\n";
    // Shifting writes real entries, so memory may run out before fuel: either stops it.
    let limited = |r: Result<Vec<Value>, Error>| matches!(r, Err(Error::Limit(_)));
    assert!(limited(quick(|| s.exec(format!("{len}table.insert(t, 1, 'x')"), "=t"))));
    assert!(limited(quick(|| s.exec(format!("{len}table.remove(t, 1)"), "=t"))));
    assert_eq!(quick(|| s.exec(format!("{len}return table.concat(t)"), "=t")), FUEL);
    assert_eq!(quick(|| s.exec(format!("{len}table.sort(t)"), "=t")).is_err(), true);
    alive(&mut s);
}

#[test]
fn ordinary_table_and_pattern_use_still_works() {
    let mut s = sb();
    let r = s
        .exec(
            "local t = {1, 2, 3}\n\
             table.insert(t, 4) table.insert(t, 1, 0) local x = table.remove(t, 2)\n\
             table.move(t, 1, 2, 3)\n\
             local trimmed = ('  hi there  '):match('^%s*(.-)%s*$')\n\
             local n = 0 for w in ('a b c'):gmatch('%a') do n = n + 1 end\n\
             return table.concat(t, ','), x, trimmed, n, string.rep('ab', 3, '-')",
            "=t",
        )
        .unwrap();
    let text: Vec<String> = r.iter().map(|v| s.to_string_lossy(*v).unwrap_or_else(|| format!("{v:?}"))).collect();
    assert_eq!(text, ["0,2,0,2", "Int(1)", "hi there", "Int(3)", "ab-ab-ab"]);
}

#[test]
fn the_limit_cannot_be_caught() {
    let mut s = sb();
    let nested = "local function lvl(f) return function() while true do pcall(coroutine.wrap(f)) end end end\n\
                  local f = function() while true do end end\n\
                  for _ = 1, 4 do f = lvl(f) end\n\
                  while true do pcall(f) end";
    assert_eq!(quick(|| s.exec(nested, "=t")), FUEL);
    let resumed = "while true do coroutine.resume(coroutine.create(function() while true do end end)) end";
    assert_eq!(quick(|| s.exec(resumed, "=t")), FUEL);
    let handled = "while true do xpcall(function() while true do end end, function() return 'fine' end) end";
    assert_eq!(quick(|| s.exec(handled, "=t")), FUEL);
    let once = "pcall(coroutine.wrap(function() while true do end end)) return 'done'";
    assert_eq!(quick(|| s.exec(once, "=t")), FUEL);
    let closing = "do local x <close> = setmetatable({}, {__close = function() while true do end end}) end";
    assert_eq!(quick(|| s.exec(closing, "=t")), FUEL);
    let in_close = "pcall(function() local x <close> = setmetatable({}, {__close = function() return 'swallow' end}) while true do end end) return 'escaped'";
    assert_eq!(quick(|| s.exec(in_close, "=t")), FUEL);
    alive(&mut s);
}

#[test]
fn shared_tables_cannot_blow_up_on_the_rust_side() {
    let mut s = sb();
    let shared = s.exec("local s = string.rep('x', 1 << 16) local t = {} for i = 1, 300 do t[i] = s end return t", "=t").unwrap();
    assert!(s.to_data(shared[0], DataLimits::default()).is_err());
    let doubled = s.exec("local x = {} for _ = 1, 22 do x = {x, x} end return x", "=t").unwrap();
    assert!(quick(|| s.to_data(doubled[0], DataLimits::default())).is_err());
    // Data passed in is bounded by depth.
    let mut deep = Data::Nil;
    for _ in 0..200 {
        deep = Data::Table(vec![(Data::Int(1), deep)]);
    }
    assert!(s.from_data(&deep).is_err());
    alive(&mut s);
}

#[test]
fn memory_bomb_is_stopped_and_the_state_recovers() {
    let mut s = Lua::new(Limits::default().memory(1 << 20), Libs::SAFE);
    let r = s.exec("local t = {} for i = 1, 1e8 do t[i] = string.rep('x', 100) .. i end", "=t");
    assert_eq!(r, MEMORY);
    assert!(s.memory_used() <= 1 << 20);
    alive(&mut s);
}

// ---- RemedyLua's own ----

#[test]
fn memory_blowups_of_every_shape() {
    let cases = [
        "local s = 'x' while true do s = s .. s end",
        "local t = {} local i = 0 while true do i = i + 1 t[i] = {} end",
        "local t = {} local i = 0 while true do i = i + 1 t['k' .. i] = i end",
        "return string.rep('x', 1 << 30)",
        "return string.rep('abc', 1e7, ',')",
        "local s = string.rep('x', 1 << 19) return table.concat({s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s, s})",
        "local s = string.rep('x', 1 << 16) while true do s = s:gsub('x', 'xx') end",
        "local t = {} for i = 1, 1e7 do t[#t + 1] = function() return i end end",
        "local co = {} for i = 1, 1e7 do co[i] = coroutine.create(function() end) end",
        "local t = setmetatable({}, {__index = function(t, k) local v = {} rawset(t, k, v) return v end}) local i = 0 while true do i = i + 1 local _ = t[i] end",
        "return string.format('%s%s%s%s', string.rep('a', 1 << 20), string.rep('a', 1 << 20), string.rep('a', 1 << 20), string.rep('a', 1 << 20))",
        "return string.pack(string.rep('i16', 1e6), table.unpack({}, 1, 1e6))",
    ];
    for c in cases {
        let mut s = Lua::new(Limits::default().memory(2 << 20).fuel(50_000_000), Libs::SAFE);
        let r = quick(|| s.exec(c, "=t"));
        assert!(r.is_err(), "{c}: {r:?}");
        assert!(s.memory_used() <= 2 << 20, "{c}: {} bytes", s.memory_used());
        alive(&mut s);
    }
}

#[test]
fn deep_recursion_is_an_error_not_a_crash() {
    let mut s = Lua::new(Limits::default().fuel(50_000_000), Libs::SAFE);
    let cases = [
        "local function f() return f() + 1 end return f()",
        "local function f(n) return 1 + f(n + 1) end return f(1)",
        "local t = setmetatable({}, {}) getmetatable(t).__index = function(t, k) return t[k] end return t.x",
        "local mt = {} mt.__add = function(a, b) return a + b end local x = setmetatable({}, mt) return x + 1",
        "local mt = {} mt.__tostring = function(v) return tostring(v) end return tostring(setmetatable({}, mt))",
        "local function f() table.sort({3, 2, 1}, function(a, b) f() return a < b end) end f()",
        "local function f(s) return (s:gsub('.', function() return f(s) end)) end return f('ab')",
        "local function f() return pcall(f) end return f()",
        "local function f() local co = coroutine.wrap(f) return co() end return f()",
        "local t = setmetatable({}, {__eq = function(a, b) return a == b end}) return t == setmetatable({}, getmetatable(t))",
        "local mt = {} mt.__index = mt setmetatable(mt, mt) return mt.missing",
        "local a = {} local b = setmetatable({}, {__index = a}) setmetatable(a, {__index = b}) return a.x",
        "local mt = {} mt.__call = mt setmetatable(mt, mt) return mt()",
        "local mt = {} mt.__newindex = mt setmetatable(mt, mt) mt.x = 1",
        "local function f() local x <close> = setmetatable({}, {__close = f}) end f()",
    ];
    for c in cases {
        let r = quick(|| s.exec(c, "=t"));
        assert!(r.is_err() || c.contains("pcall"), "{c}: {r:?}");
        alive(&mut s);
    }
}

#[test]
fn pathological_patterns_stop() {
    let mut s = sb();
    let cases = [
        "return string.find(string.rep('a', 5000), string.rep('a?', 5000) .. string.rep('a', 5000))",
        "return string.match(string.rep('a', 1e5), '(.-)%1b')",
        "return (string.rep('a', 1e5)):gsub('(a*)*b', '')",
        "return string.find(string.rep('(', 1000), string.rep('%b()', 100))",
        "return string.match('x', string.rep('(', 40) .. 'x' .. string.rep(')', 40))",
        "return string.match(string.rep('a', 300), string.rep('(a)', 33))",
        "for k in string.gmatch(string.rep('a', 1e6), '()') do end",
    ];
    for c in cases {
        let r = quick(|| s.exec(c, "=t"));
        assert!(r.is_err() || c.contains("gmatch") || c.contains("%b"), "{c}: {r:?}");
        alive(&mut s);
    }
}

#[test]
fn coroutine_tricks() {
    let mut s = Lua::new(Limits::default().fuel(20_000_000), Libs::SAFE);
    let cases = [
        // Resuming itself, a dead one, the main thread.
        ("local co co = coroutine.create(function() return coroutine.resume(co) end) return coroutine.resume(co)", true),
        ("local co = coroutine.create(function() end) coroutine.resume(co) return coroutine.resume(co)", true),
        ("return coroutine.resume(coroutine.running())", true),
        ("return coroutine.close(coroutine.running())", false),
        // Yielding from places that cannot.
        ("return coroutine.yield()", false),
        ("return coroutine.wrap(function() table.sort({1, 2, 3}, function() coroutine.yield() end) end)()", false),
        ("return coroutine.wrap(function() return tostring(setmetatable({}, {__tostring = function() coroutine.yield() end})) end)()", false),
        // Yielding from Lua metamethods works.
        ("local t = setmetatable({}, {__index = function(_, k) return coroutine.yield(k) end}) local co = coroutine.wrap(function() return t.x end) return co(), co(5)", true),
        // Chains of coroutines resuming each other.
        ("local function chain(n) if n == 0 then return 0 end return coroutine.wrap(function() return chain(n - 1) + 1 end)() end return chain(150)", true),
        // A coroutine left suspended forever, closed, collected.
        ("for i = 1, 1000 do local co = coroutine.create(function() coroutine.yield() end) coroutine.resume(co) end collectgarbage() return 'ok'", true),
        ("for i = 1, 100 do local co = coroutine.wrap(function() local x <close> = setmetatable({}, {__close = function() end}) coroutine.yield() end) co() end return 'ok'", true),
    ];
    for (c, ok) in cases {
        let mut lua = Lua::new(Limits::default().fuel(20_000_000), Libs::STANDARD);
        let r = quick(|| lua.exec(c, "=t"));
        assert_eq!(r.is_ok(), ok, "{c}: {r:?}");
        alive(&mut lua);
    }
    alive(&mut s);
}

#[test]
fn collector_abuse() {
    let mut s = Lua::new(Limits::default().fuel(5_000_000).memory(4 << 20), Libs::STANDARD);
    let cases = [
        "while true do collectgarbage() end",
        "while true do collectgarbage('step') end",
        "local w = setmetatable({}, {__mode = 'kv'}) while true do w[{}] = {} end",
        "local w = setmetatable({}, {__mode = 'k'}) local strong = {} for i = 1, 1e6 do local k = {} w[k] = {k} if i % 2 == 0 then strong[#strong + 1] = k end end",
        "local a = {} for i = 1, 1e6 do local b = {a = a} a.b = b a = b end",
    ];
    for c in cases {
        let r = quick(|| s.exec(c, "=t"));
        assert!(r.is_err(), "{c}: {r:?}");
        alive(&mut s);
    }
}

#[test]
fn source_text_attacks() {
    let mut s = Lua::new(Limits::default(), Libs::ALL);
    let deep_parens = format!("return {}1{}", "(".repeat(100_000), ")".repeat(100_000));
    let deep_tables = format!("return {}{}", "{".repeat(100_000), "}".repeat(100_000));
    let long_chain = format!("return 1{}", " + 1".repeat(100_000));
    let deep_blocks = format!("{}{}", "do ".repeat(100_000), "end ".repeat(100_000));
    let deep_funcs = format!("{}{}", "function f() ".repeat(10_000), "end ".repeat(10_000));
    let bytecode = "\x1bLua\x54\x00 not really bytecode".to_owned();
    // Nesting is bounded by memory alone (the front end does not recurse, and registers
    // past one window are reached through windows): these run, or stop at the memory cap.
    for src in [deep_parens, long_chain, deep_blocks, deep_funcs] {
        let r = quick(|| s.exec(&src, "=t"));
        assert!(r.is_ok(), "{}…: {r:?}", &src[..20.min(src.len())]);
        alive(&mut s);
    }
    let r = quick(|| s.exec(&deep_tables, "=t"));
    assert!(r.is_ok() || matches!(r, Err(Error::Limit(LimitKind::Memory))), "deep tables: {r:?}");
    alive(&mut s);
    for src in [bytecode] {
        let r = quick(|| s.exec(&src, "=t"));
        assert!(matches!(r, Err(Error::Syntax(_))), "{}…: {r:?}", &src[..20.min(src.len())]);
        alive(&mut s);
    }
    // `load` refuses bytecode and a binary mode, and its errors are values.
    let r = s.exec("return load('\\27Lua'), load('return 1', 'x', 'b')", "=t").unwrap();
    assert!(r[0].is_nil() && r[1].is_nil(), "{r:?}");
    // `load` itself is a capability.
    let mut safe = sb();
    assert_eq!(safe.exec("return load == nil, dofile == nil, require == nil, io == nil, os.execute == nil", "=t").unwrap(), vec![Value::Bool(true); 5]);
}

#[test]
fn huge_values_and_argument_counts() {
    let mut s = Lua::new(Limits::default().fuel(20_000_000).memory(8 << 20), Libs::SAFE);
    let cases = [
        "return table.unpack({}, 1, 1e8)",
        "return select('#', table.unpack({}, 1, 1e6))",
        "return string.char(table.unpack({}, 1, 1e6))",
        "return utf8.char(table.unpack({}, 1, 1e6))",
        "local t = {} t[1e300] = 1 t[-1e300] = 2 t[math.mininteger] = 3 t[math.maxinteger] = 4 return #t",
        "local t = {} t[2^53] = 1 return next(t)",
        "return ('x'):rep(1e9)",
        "return string.format('%099d', 1)",
        "return #string.format('%99.99f', 1e308)",
        "local t = {} for i = 1, 300 do t[i] = 'x' end return string.format(table.concat(t, '%s'), table.unpack(t))",
    ];
    for c in cases {
        let r = quick(|| s.exec(c, "=t"));
        let _ = r;
        alive(&mut s);
    }
}

#[test]
fn host_functions_errors_and_panics_are_contained() {
    let mut s = sb();
    s.register("host.add", |_, args| {
        let a = args.first().and_then(|v| v.as_int()).ok_or_else(|| Error::runtime("need numbers"))?;
        let b = args.get(1).and_then(|v| v.as_int()).ok_or_else(|| Error::runtime("need numbers"))?;
        Ok(vec![Value::Int(a + b)])
    })
    .unwrap();
    s.register("host.reenter", |lua, _| {
        let r = lua.exec("return 40 + 2", "=inner")?;
        Ok(r)
    })
    .unwrap();
    s.register("host.limit", |_, _| Err(Error::Limit(LimitKind::Fuel))).unwrap();
    assert_eq!(s.exec("return host.add(2, 3)", "=t").unwrap(), vec![Value::Int(5)]);
    let m = err_msg(s.exec("return host.add('x')", "=t"));
    assert!(m.contains("need numbers"), "{m}");
    assert_eq!(s.exec("local ok = pcall(host.add) return ok", "=t").unwrap(), vec![Value::Bool(false)]);
    assert_eq!(s.exec("return host.reenter()", "=t").unwrap(), vec![Value::Int(42)]);
    // A limit a host function reports cannot be caught either.
    assert_eq!(s.exec("pcall(host.limit) return 'escaped'", "=t"), FUEL);
    alive(&mut s);
}

#[test]
fn stale_values_are_refused_by_the_host_api() {
    let mut s = sb();
    let t = s.exec("return {}", "=t").unwrap()[0];
    s.exec("return 1", "=t").unwrap();
    s.collect_garbage().unwrap();
    s.collect_garbage().unwrap();
    // The table was collected: using it is an error, not undefined behaviour.
    assert!(matches!(s.call(t, &[]), Err(Error::Host(_))));
    let rooted = s.exec("return {}", "=t").unwrap()[0];
    let root = s.root(rooted);
    s.collect_garbage().unwrap();
    assert!(s.set(root.value(), Value::Int(1), Value::Int(2)).is_ok());
}
