//! Sweep 2, area "equiv": the fast loop, the full path and C Lua agree on everything that
//! changed in cycle 2. Every program runs three ways (fast loop on, off, and in C Lua):
//! results and error messages with their positions must match, and the two RemedyLua runs
//! must use the same fuel and hold the same memory. Then each program is cut short by fuel
//! at many points (every point for short programs) and by memory at many caps, and the
//! two RemedyLua runs must stop the same way, at the same fuel, holding the same memory.

use remedy_lua::{Libs, Limits, Lua};

use super::common;

#[path = "sweep1_fuzz/grammar.rs"]
#[allow(dead_code)]
mod grammar;
#[path = "sweep1_fuzz/rng.rs"]
#[allow(dead_code)]
mod rng;

type Run = (Result<String, String>, u64, usize);

const FUEL: u64 = 50_000_000;

fn run(program: &str, fuel: u64, memory: usize, fast: bool) -> Run {
    run_with(program, fuel, memory, fast, false)
}

/// `run`, optionally with a whole collection at every allocation.
fn run_with(program: &str, fuel: u64, memory: usize, fast: bool, stress: bool) -> Run {
    let mut lua = Lua::new(Limits::default().fuel(fuel).memory(memory).hash_seed(common::TEST_SEED), Libs::ALL);
    lua.set_fast_loop(fast);
    lua.set_gc_stress(stress);
    let r = match lua.exec(common::wrap(program), "=wrapper") {
        Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
        Err(e) => Err(e.to_string()),
    };
    (r, fuel.saturating_sub(lua.fuel_left()), lua.memory_used())
}

fn both(program: &str, fuel: u64, memory: usize) -> Run {
    let f = run(program, fuel, memory, true);
    let s = run(program, fuel, memory, false);
    assert_eq!(f, s, "fast loop and full path differ at fuel {fuel}, memory {memory}:\n{program}");
    f
}

/// Three-way at full budget, then fuel and memory cut points.
fn equiv(program: &str) {
    equiv_vs(program, program);
}

/// `equiv`, with C Lua running `in_c` (the same program within C Lua's own limits).
fn equiv_vs(program: &str, in_c: &str) {
    let full = both(program, FUEL, 256 << 20);
    {
        let c = common::oracle(in_c, FUEL);
        assert_eq!(full.0, c, "RemedyLua and C Lua differ:\n{program}");
    }
    let used = full.1;
    let cuts: Vec<u64> = if used <= 2500 {
        (0..used).collect()
    } else {
        (0..400).map(|i| used * i / 400).chain(used - 40..used).collect()
    };
    for cut in cuts {
        let r = both(program, cut, 256 << 20);
        assert!(r.0.is_err(), "fuel {cut} of {used} did not stop the program: {r:?}\n{program}");
    }
    // The smallest cap that lets it finish, then caps below it.
    let (mut lo, mut hi) = (0usize, 256 << 20);
    while hi - lo > 64 {
        let mid = (lo + hi) / 2;
        if run(program, FUEL, mid, true).0 == full.0 { hi = mid } else { lo = mid }
    }
    let step = (hi / 60).max(1);
    for cap in (0..60).map(|i| hi - i * step).chain((1..64).map(|d| hi - d)) {
        let _ = both(program, FUEL, cap);
    }
}

fn all(programs: &[&str]) {
    for p in programs {
        equiv(p);
    }
}

#[test]
fn calls_and_returns() {
    all(&[
        r#"local function f0() end
           local function f1() return 1 end
           local function f3() return 1, 2, 3 end
           local function fv(...) return ... end
           local function fn_(n) local t = {} for i = 1, n do t[i] = i end return table.unpack(t) end
           local r = {}
           r[1] = {f0()} r[2] = {f1()} r[3] = {f3()} r[4] = {(f3())} r[5] = {f3(), f3()}
           r[6] = {fv()} r[7] = {fv(nil, nil)} r[8] = select('#', fv(nil, nil, nil))
           r[9] = {fv(f3())} r[10] = {fv(1, f3())} r[11] = {fv(f3(), 1)}
           local a, b, c, d = f3() r[12] = {a, b, c, d}
           local x, y = f0() r[13] = {x == nil, y == nil}
           r[14] = select('#', fn_(300)) r[15] = {select(-2, fn_(260))}
           r[16] = select('#', fv(fn_(250)))
           return r"#,
        r#"local function tail(n, ...) if n == 0 then return ... end return tail(n - 1, n, ...) end
           local function deep(n) if n == 0 then return 0 end return 1 + deep(n - 1) end
           local t = {tail(20)}
           return #t, t[1], t[20], deep(150), select('#', tail(0)), select('#', tail(3, nil))"#,
        r#"local function v(...) local a, b = ... local t = {...} local n = select('#', ...)
             return n, a, b, #t, ... end
           return v(), v(1), v(nil), v(1, nil, 3), (v(1, 2, 3))"#,
        r#"local function err(n) if n == 0 then error("boom") end return err(n - 1) end
           local ok, e = pcall(err, 5)
           local ok2, e2 = pcall(function() local x = nil; return x.y end)
           local ok3, e3 = pcall(function() return 1 + {} end)
           local ok4, e4 = pcall(function() local t = setmetatable({}, {__call = 1}); return t() end)
           local ok5, e5 = pcall(error, {code = 1})
           local ok6, e6 = pcall(error)
           return ok, e, ok2, e2, ok3, e3, ok4, e4, ok5, type(e5), ok6, e6"#,
        r#"local o = setmetatable({}, {__call = function(self, a, ...) return a, select('#', ...), ... end})
           local function t() return o(1, 2, 3) end
           local c = setmetatable({}, {__call = o})
           return o(), o(9), t(), pcall(o, 5, 6), select('#', o(nil, nil))"#,
        r#"local co = coroutine.wrap(function(...) local a = {...}
             local b = {coroutine.yield(#a, ...)} return #b, table.unpack(b) end)
           local r1 = {co(1, 2, nil)}
           local r2 = {co()}
           return r1, r2"#,
        // A call with many arguments and results crossing the register-window width.
        r#"local function many(...) return select('#', ...), ... end
           local t = {} for i = 1, 600 do t[i] = i end
           local r = {many(table.unpack(t))}
           local s = 0 for i = 1, #r do s = s + r[i] end
           return #r, s, r[1], r[601]"#,
    ]);
}

#[test]
fn closures_and_upvalues() {
    all(&[
        r#"local fs = {}
           for i = 1, 5 do local j = i * 2 fs[i] = function() j = j + 1 return i, j end end
           local r = {} for i = 1, 5 do r[i] = {fs[i](), fs[i]()} end
           local k = 0
           local function inc() k = k + 1 return k end
           inc() inc()
           local g = function() return function() k = k * 10 return k end end
           r[6] = {g()(), inc(), k}
           return r"#,
        r#"local function counter() local c = 0
             return function() c = c + 1 return c end, function() return c end end
           local a, b = counter()
           a() a() local x = b()
           local w = {} local i = 1
           while i <= 3 do local v = i w[i] = function() v = v + 100 return v end i = i + 1 end
           return x, w[1](), w[1](), w[2](), w[3]()"#,
        r#"local fs = {}
           do local x = 1
             for i = 1, 3 do
               local y = i
               fs[#fs + 1] = function() x = x + y return x end
               if i == 2 then goto skip end
               fs[#fs + 1] = function() return y end
               ::skip::
             end
           end
           local r = {} for i = 1, #fs do r[i] = fs[i]() end return r"#,
        r#"local function mk() local a, b, c = 1, 2, 3
             local function f() a, b, c = b, c, a return a, b, c end
             return f end
           local f = mk() f() return f(), f()"#,
        // Upvalues closed by a return from a frame inside the fast loop, then by an error.
        r#"local keep
           local function f(n) local u = n keep = function() u = u + 1 return u end
             if n > 3 then error("e" .. n) end return u end
           local r = {f(1), keep(), keep(), pcall(f, 5), keep()}
           return r"#,
    ]);
}

#[test]
fn metamethods_on_fast_ops() {
    all(&[
        r#"local log = {}
           local mt = {}
           for _, e in ipairs({"add","sub","mul","div","mod","pow","unm","idiv","band","bor","bxor","shl","shr","bnot","concat","len","lt","le"}) do
             mt["__" .. e] = function(a, b) log[#log + 1] = e return e end
           end
           mt.__eq = function() log[#log + 1] = "eq" return true end
           local a, b = setmetatable({}, mt), setmetatable({}, mt)
           local r = {a + 1, 1 - a, a * b, a / 2, a % 3, a ^ 2, -a, a // 1, a & 1, 1 | a, a ~ a, a << 1, a >> 1, ~a,
                      a .. "x", "x" .. a, #a, a < b, a <= b, a > 1, 1 >= a, a == b, a ~= b, a == a}
           return r, table.concat(log, ",")"#,
        r#"local mt = {__index = function(t, k) return k .. "!" end, __newindex = function(t, k, v) rawset(t, k, v * 2) end}
           local t = setmetatable({}, mt)
           t.x = 5 t[1] = 2 t[1] = 7
           local base = {y = 1} base.__index = base
           local mid = setmetatable({z = 2}, base) mid.__index = mid
           local o = setmetatable({}, mid)
           local s = 0 for i = 1, 50 do s = s + o.y + o.z end
           base.y = 10
           local s2 = 0 for i = 1, 5 do s2 = s2 + o.y end
           mid.y = 100
           local s3 = o.y
           setmetatable(mid, nil)
           return t.x, t[1], t.q, s, s2, s3, o.y, rawget(o, "y")"#,
        // `__index` hints invalidated by a metatable change, a key removal, a new key.
        r#"local C = {} C.__index = C
           function C.get(s) return s.v end
           local o = setmetatable({v = 1}, C)
           local r = {}
           for i = 1, 6 do
             r[#r + 1] = o:get()
             if i == 2 then C.get = function(s) return -s.v end end
             if i == 3 then rawset(o, "get", function() return "own" end) end
             if i == 4 then o.get = nil end
             if i == 5 then setmetatable(o, {__index = function() return function() return "fn" end end}) end
           end
           return r"#,
        r#"local nmt = {__newindex = function(t, k, v) rawset(t, k, "n" .. tostring(v)) end}
           local t = setmetatable({1, 2, a = 1}, nmt)
           t[1] = 10 t.a = 20 t[3] = 30 t.b = 40 t[2] = nil t[2] = 50
           local u = setmetatable({}, {__newindex = t})
           u.c = 1 u[1] = 2
           return t, u"#,
        r#"local eqc = 0
           local m1 = {__eq = function(a, b) eqc = eqc + 1 return false end}
           local m2 = {__eq = function(a, b) eqc = eqc + 1 return true end}
           local a, b, c = setmetatable({}, m1), setmetatable({}, m2), {}
           return a == b, b == a, a == c, c == b, a ~= b, a == a, eqc"#,
        r#"local lt = {__lt = function(a, b) return rawlen(a) < rawlen(b) end, __le = function(a, b) return rawlen(a) <= rawlen(b) end}
           local a, b = setmetatable({1}, lt), setmetatable({1, 2}, lt)
           local r = {a < b, b < a, a <= b, b <= a, a > b, a >= b}
           r[#r + 1] = pcall(function() return a < 1 end)
           r[#r + 1] = select(2, pcall(function() return {} < {} end))
           r[#r + 1] = select(2, pcall(function() return 1 < "2" end))
           r[#r + 1] = select(2, pcall(function() return "a" <= 1 end))
           return r"#,
        r#"local t = setmetatable({}, {__len = function() return 42 end})
           local s = setmetatable({}, {__concat = function(a, b) return (type(a) == "table" and "T" or a) .. (type(b) == "table" and "T" or b) end})
           return #t, s .. 1 .. 2, 1 .. 2 .. s, 1 .. s .. 2, "a" .. "b" .. s .. "c" .. "d",
             select(2, pcall(function() return 1 .. {} end)), select(2, pcall(function() return #nil end))"#,
        // Errors raised from the fast ops: messages with variable names and positions.
        r#"local function e(f) local ok, m = pcall(f) return m end
           local t, s, n = {}, "x", nil
           local up = nil
           return e(function() return t.a.b end), e(function() return n + 1 end),
             e(function() return s + 1 end), e(function() return t < t end),
             e(function() return up.x end), e(function() return t.x.y.z end),
             e(function() t[nil] = 1 end), e(function() t[0/0] = 1 end),
             e(function() return 1 // 0 end), e(function() return 1 % 0 end),
             e(function() return 2^63 | 0 end), e(function() return "1.5" | 0 end),
             e(function() return ("x"):bad() end), e(function() return math.huge // 0 end),
             e(function() local a = {} return a.b.c end), e(function() return -{} end)"#,
    ]);
}

#[test]
fn numbers_and_for_loops() {
    all(&[
        r#"local r = {}
           for i = math.maxinteger - 2, math.maxinteger do r[#r + 1] = i end
           for i = math.mininteger + 2, math.mininteger, -1 do r[#r + 1] = i end
           for i = 1, 3, math.maxinteger do r[#r + 1] = i end
           for i = -1, -3, math.mininteger do r[#r + 1] = i end
           for i = 1, 0 do r[#r + 1] = "never" end
           for i = 0.1, 0.35, 0.1 do r[#r + 1] = i end
           for i = 1, 2.5 do r[#r + 1] = i end
           for i = 1.0, 3 do r[#r + 1] = i end
           for i = 3, 1.5, -1 do r[#r + 1] = i end
           for i = 1, math.huge do if i > 3 then break end r[#r + 1] = i end
           for i = 1, -math.huge do r[#r + 1] = "never" end
           for i = math.maxinteger, 1e100 do if i < 0 then break end r[#r + 1] = i break end
           r[#r + 1] = select(2, pcall(function() for i = 1, 10, 0 do end end))
           r[#r + 1] = select(2, pcall(function() for i = 1.0, 10, 0.0 do end end))
           r[#r + 1] = select(2, pcall(function() for i = "a", 2 do end end))
           r[#r + 1] = select(2, pcall(function() for i = 1, {} do end end))
           r[#r + 1] = select(2, pcall(function() for i = 1, 2, "x" do end end))
           for i = 1, "3" do r[#r + 1] = i end
           return r"#,
        r#"local mi, ma = math.mininteger, math.maxinteger
           return ma + 1, mi - 1, ma * 2, mi // -1, mi % -1, 5 // 0.0, -5 // 0.0, 0/0 ~= 0/0,
             5 % -3, -5 % 3, 5.5 % -2, -5.5 % 2, 5 % math.huge, -5 % math.huge, 5 % -math.huge,
             1e308 * 10, -0.0, 0.0 == -0.0, 1 / -0.0, 3 // 2, 3.0 // 2, -3 // 2, 7 // -2.0,
             2^53 + 1, (2^53 + 1) | 0, math.tointeger(2^53), 1 << 63, 1 << 64, -1 >> 1, 1 << -1,
             mi == mi + 0.0, ma + 0.0 == ma, ma < ma + 0.0, mi <= mi + 0.0, 1 < 1.5, -0.0 < 0,
             "10" + 1, "0x10" * 1, "1e1" // 1, 10 .. "", 1.5 .. "", -0.0 .. "", 1e15 .. "", 1e16 .. "",
             2^63 == ma + 1.0, ma // 1.0, 3 - 0.0, -(0.0) - 0, 0 - 0.0"#,
        r#"local s = 0
           for i = 1, 1000 do s = s + i * 0.5 - i // 3 + i % 7 end
           local x = 1.0 for i = 1, 200 do x = x * 1.5 x = x - x // 1 end
           local n = 0 for i = 10, 1, -1 do n = n * 3 + i end
           local f = 0 for v = 1.5, -1.5, -0.25 do f = f + v end
           return s, x, n, f"#,
        r#"local function cmp(a, b) return {a < b, a <= b, a > b, a >= b, a == b} end
           local mi, ma = math.mininteger, math.maxinteger
           return cmp(1, 1.0), cmp(ma, 2^63), cmp(mi, -2^63), cmp(ma, 0/0), cmp(2^53, 2^53 + 1),
             cmp(ma - 1, ma + 0.0), cmp(-0.0, 0), cmp(math.huge, ma), cmp(-math.huge, mi), cmp("a", "b"), cmp("", "\0")"#,
    ]);
}

#[test]
fn strings_at_the_short_boundary() {
    all(&[
        r#"local r = {}
           for n = 38, 43 do
             local a = string.rep("a", n)
             local b = string.rep("a", n - 1) .. "a"
             local c = a:sub(1, n - 1) .. "b"
             local t = {[a] = n}
             r[#r + 1] = {n, #a, a == b, a < c, t[b], t[c], (a .. ""):len(), (a .. 1):len(),
                          a:upper():sub(-3), a:byte(-1), rawequal(a, b), #(a .. a)}
             t[b] = nil
             r[#r + 1] = next(t)
           end
           return r"#,
        r#"local r = {}
           local s19, s20, s21 = string.rep("x", 19), string.rep("y", 20), string.rep("z", 21)
           r[1] = #(s20 .. s20) r[2] = #(s20 .. s21) r[3] = #(s19 .. s21) r[4] = #(s20 .. 1 .. s19)
           r[5] = s20 .. s20 == string.rep("y", 40)
           r[6] = (s20 .. s21) == (s20 .. "z" .. s20)
           local k = s20 .. s21
           local t = {} t[k] = 1 t[s20 .. s21] = (t[s20 .. s21] or 0) + 1
           for kk, v in pairs(t) do r[#r + 1] = #kk .. "=" .. v end
           r[#r + 1] = select(2, pcall(function() return s20 .. {} end))
           r[#r + 1] = string.format("%s|%s", k, 1.5)
           r[#r + 1] = (k):find("z", 1, true)
           r[#r + 1] = k:rep(2, ",") == k .. "," .. k
           return r"#,
        // Long strings as keys through rawget/rawset/next/__index/__newindex in a loop.
        r#"local keys = {}
           for i = 1, 30 do keys[i] = string.rep("k", 35 + i % 10) .. i end
           local t = {}
           for i = 1, 30 do t[keys[i]] = i end
           local s = 0
           for i = 1, 30 do s = s + t[string.rep("k", 35 + i % 10) .. i] end
           local p = setmetatable({}, {__index = t})
           for i = 1, 30 do s = s + p[keys[i] .. ""] end
           local cnt = 0 for k, v in pairs(t) do cnt = cnt + v end
           return s, cnt"#,
        r#"local a = string.rep("ab", 30)
           return a < a .. "", a <= a, a == a:sub(1), a:sub(1, 40) < a, a:sub(1, 41) .. "" == a:sub(1, 41),
             ("\0"):rep(41) < ("\0"):rep(42), ("\0"):rep(41) == "\0" .. ("\0"):rep(40), #tostring(12345678901234567890)"#,
    ]);
}

// `%a` is left out: its text is the C library's (C Lua's build here refuses its modifiers).
#[test]
fn format_against_c() {
    all(&[
        r#"local vals = {0, -0.0, 1, -1, 0.1, 1/3, 2/3, 1e-5, 1e-4, 123456, 1234567, 1e15, 1e16, 1e17, 1e100, 1e-300,
                         5e-324, 2.5, 0.5, 1.5, 3.5, 1e21, 123.456e7, 9.999999, 9.9999999, 0.000099999, math.pi, -math.pi,
                         2^53, 2^63, math.maxinteger, math.mininteger, 1e308, 99.5, 0.05, 0.15, 0.25, 0.35}
           local fmts = {"%g", "%.0g", "%.1g", "%.3g", "%.10g", "%.17g", "%#g", "%#.3g", "%e", "%.0e", "%#.0e", "%.3e", "%.20e",
                         "%f", "%.0f", "%#.0f", "%.3f", "%.20f", "%10.3f", "%-10.2e|", "%+g", "% g", "%+.3e", "%010.2f", "%-+12g|",
                         "%5.1f", "%G", "%E", "%.1G", "%#x", "%5s", "%.99f"}
           local out = {}
           for _, f in ipairs(fmts) do
             local row = {}
             for _, v in ipairs(vals) do
               local ok, s = pcall(string.format, f, v)
               row[#row + 1] = ok and s or ("E:" .. s)
             end
             out[#out + 1] = f .. " " .. table.concat(row, " ")
           end
           return table.concat(out, "\n")"#,
        r#"local inf = math.huge -- (NaN's text is the C library's: not compared)
           local r = {}
           for _, f in ipairs({"%g", "%e", "%f", "%.3f", "%10g", "%-10e|", "%+f", "%010g"}) do
             r[#r + 1] = string.format(f, inf) .. "," .. string.format(f, -inf)
           end
           return r"#,
        r#"return string.format("%d %5d %-5d| %05d %+d %x %X %#o %o %c%c %i", 42, -42, 7, -3, 0, 255, 255, 8, 8, 72, 105, 3.0),
             string.format("%q", "a\0b\n\r\t\"\\\1\127\200z"), string.format("%q", 1/3), string.format("%q", math.mininteger),
             string.format("%q", 1/0), string.format("%q", -1/0), string.format("%q", 0/0) ~= nil, string.format("%q", 2^63),
             string.format("%s %s %s %s", nil, true, 1.0, -0.0), string.format("%10.3s|%-5s|", "abcdef", "x"),
             string.format("%%"), string.format("%.0s", "abc"), string.format("%s", 12)"#,
        r#"local function e(...) local ok, m = pcall(string.format, ...) return m end
           return e("%d", 1.5), e("%d", "x"), e("%y", 1), e("%", 1), e("%10"), e("%d"), e("%c", "a"),
             e("%123d", 1), e("%.123f", 1), e("%x", -1.0), e("%d", 2^63), e("%s"), e("%q", {}), e("%q", print),
             e("%.3q", "a"), e("%#q", 1), e("%-5c|", 65), e("%5.2s|", "abc"), e("%lld", 1)"#,
    ]);
}

#[test]
fn patterns_against_c() {
    all(&[
        r#"local r = {}
           local function add(...) local t = table.pack(...) for i = 1, t.n do r[#r + 1] = tostring(t[i]) end r[#r + 1] = "|" end
           add(("hello world"):find("o w")) add(("hello world"):find("o", 6)) add(("hello"):find("l+"))
           add(("hello"):find("")) add(("hello"):find("", 10)) add(("hello"):find("", 6)) add(("hello"):find("", -2))
           add(("a.b"):find(".", 1, true)) add(("a+b"):find("+", 1, true)) add(("abc"):find("b", -1)) add(("abc"):find("b", -10))
           add(("key = value"):match("(%w+)%s*=%s*(%w+)")) add(("  trim  "):match("^%s*(.-)%s*$"))
           add(("[[x]]"):match("%[(%b[])%]")) add(("THE (quick) fox"):find("%((%a+)%)"))
           add(("abc"):match("()b()")) add(("aaa"):match("a-b")) add(("aaab"):match("a-b")) add(("x"):match("x*?"))
           add(("hello"):match(".-(l+)(.*)")) add(("f(a,b)"):match("%f[%w]%w+")) add(("THE END"):gsub("%f[%w]%w+", "W"))
           add(("abc"):gsub("", "-")) add(("abc"):gsub(".", {a = 1, b = true, c = false}))
           add(("hello world"):gsub("o", "0", 1)) add(("abc"):gsub("%w", "%0%0")) add(("abc"):gsub("(a)(b)", "%2%1"))
           add(("hello"):gsub("l", function(c) return nil end)) add(("x=1 y=2"):gsub("(%w)=(%w)", "%2=%1"))
           add(("abc"):gsub("b*", "X")) add(("a,b,,c"):gsub(",", ";", 2)) add(("äö"):gsub("[\128-\255]", "?"))
           add(("%d"):gsub("%%", "%%%%")) add(("a b c"):gsub(" ", "%%"))
           for k, v in ("a=1, b=2, c=3"):gmatch("(%w+)=(%w+)") do add(k, v) end
           for w in ("one two  three"):gmatch("%a+") do add(w) end
           for p in ("abc"):gmatch("()") do add(p) end
           for a in ("xyz"):gmatch(".", 2) do add(a) end
           add(("  x"):find("^%s*")) add(("x"):match("^(x?)(x?)$")) add(("0x1F"):match("^0[xX](%x+)$"))
           add(("a\0b"):find("\0")) add(("a\0b"):match("%z")) add(("-12.5e3"):match("^[+-]?%d+%.?%d*[eE]?[+-]?%d*$"))
           add(("aXb"):match("%u")) add(("a1_"):match("[%w_]+")) add(("a]b"):match("[]]")) add(("a-b"):match("[a-]+"))
           add(("^a"):match("[%^a]+")) add(("caab"):match("[^c]+")) add(string.rep("a", 50):match("(a*)b?$") == string.rep("a", 50))
           return table.concat(r, " ")"#,
        r#"local function e(...) local ok, m = pcall(...) return m end
           return e(string.find, "a", "%"), e(string.find, "a", "[a"), e(string.find, "a", "(()"), e(string.find, "a", "%b"),
             e(string.find, "a", "%f"), e(string.gsub, "a", "(a)", "%2"), e(string.gsub, "a", "a", "%x"),
             e(string.match, "a", ")"), e(string.gsub, "a", ".", 1), e(string.gsub, "abc", ".", {a = {}}),
             e(string.gsub, "a", "a", "x", -1), e(string.rep, "x", -1), e(string.find, "a", "%g"), e(string.gmatch, "a", "(")()"#,
        r#"local s = string.rep("ab", 200)
           local n1 = select(2, s:gsub("ab", "x"))
           local n2 = select(2, s:gsub("(a)(b)", "%2%1"))
           local out = s:gsub("b", function(c) return c:upper() end)
           local cnt = 0 for _ in s:gmatch("a") do cnt = cnt + 1 end
           return n1, n2, #out, out:sub(1, 10), cnt, s:find("ba", 300), s:match(".-(bab)", 100), (s .. "c"):find("c")"#,
    ]);
}

#[test]
fn traversal_with_modification() {
    all(&[
        r#"local t = {} for i = 1, 20 do t[i] = i t["k" .. i] = i end
           local seen, sum = 0, 0
           for k, v in pairs(t) do
             seen = seen + 1 sum = sum + v
             t[k] = nil
           end
           return seen, sum, next(t)"#,
        r#"local t = {} for i = 1, 20 do t["k" .. i] = i end
           local c = 0
           for k, v in pairs(t) do c = c + 1 t[k] = v * 2 end
           local s = 0 for k, v in pairs(t) do s = s + v end
           return c, s"#,
        r#"local t = {1, 2, 3, x = 1}
           local r = {}
           r[1] = select(2, pcall(next, t, "nope"))
           r[2] = select(2, pcall(next, t, 99))
           r[3] = select(2, pcall(next, {}, nil))
           r[4] = select('#', next({}))
           local k = next(t) t[k] = nil
           r[5] = pcall(next, t, k)
           r[6] = select(2, pcall(function() for k in pairs(nil) do end end))
           r[7] = select(2, pcall(ipairs))
           r[8] = select(2, pcall(next))
           return r"#,
        r#"local t = setmetatable({}, {__index = function(_, i) if i <= 5 then return i * 10 end end})
           local r = {} for i, v in ipairs(t) do r[#r + 1] = v end
           local u = {1, 2, nil, 4}
           for i, v in ipairs(u) do r[#r + 1] = i end
           local w = {1, 2, 3}
           for i, v in ipairs(w) do r[#r + 1] = v if i == 1 then w[3] = nil w[2] = "b" end end
           local p = setmetatable({}, {__pairs = function(t) return function(_, k) if not k then return 1, "one" end end, t, nil end})
           for k, v in pairs(p) do r[#r + 1] = v end
           local x = {} for i = 1, 10 do x[i] = i end
           for k, v in next, x do r[#r + 1] = k + v end
           return r"#,
        r#"local t = {}
           for i = 1, 8 do t[i] = i end
           local r = {}
           for k, v in pairs(t) do r[#r + 1] = k if k == 4 then t[8] = nil t[7] = nil end end
           return r"#,
    ]);
}

#[test]
fn table_edges() {
    all(&[
        r#"local r = {}
           local t = {1, 2, 3, nil, 5} r[#r + 1] = #t == 5 or #t == 3
           local u = {} u[1] = 1 u[2] = 2 u[4] = 4 r[#r + 1] = (#u == 2 or #u == 4)
           local v = {n = 1} for i = 1, 100 do v[i] = i end v[50] = nil r[#r + 1] = #v
           for i = 100, 51, -1 do v[i] = nil end r[#r + 1] = #v
           local w = {} w[-1] = "neg" w[0] = "zero" w[1.0] = "one" w[2^53] = "big" w[-0.0] = "negzero" w[1.5] = "half"
           r[#r + 1] = w[-1] .. w[0] .. w[1] .. w[2^53 | 0] .. w[1.5] .. #w
           r[#r + 1] = select(2, pcall(function() w[0/0] = 1 end))
           r[#r + 1] = select(2, pcall(rawset, w, 0/0, 1))
           r[#r + 1] = w[0/0] == nil and rawget(w, 0/0) == nil
           r[#r + 1] = select(2, pcall(function() w[nil] = 1 end))
           r[#r + 1] = w[nil] == nil
           local x = {[1] = "a", [2] = "b", [3.0] = "c", ["4"] = "d"} r[#r + 1] = #x
           local y = {math.mininteger, math.maxinteger} y[math.maxinteger] = 1 y[math.mininteger] = 2
           r[#r + 1] = y[math.maxinteger] + y[math.mininteger]
           local z = table.pack(nil, nil) r[#r + 1] = z.n
           r[#r + 1] = select('#', table.unpack({1, nil, 3}))
           r[#r + 1] = table.concat({1, 2.5, "x"}, "-")
           r[#r + 1] = select(2, pcall(table.concat, {1, {}, 3}))
           r[#r + 1] = select(2, pcall(table.insert, {1}, 5, 2))
           local q = {} table.insert(q, 1) table.insert(q, 1, 0) table.remove(q, 1) r[#r + 1] = q
           r[#r + 1] = table.remove({}) == nil
           return r"#,
        r#"local t = {}
           for i = 1, 300 do t[i] = i end
           for i = 1, 300, 2 do t[i] = nil end
           local n = 0 for k, v in pairs(t) do n = n + 1 end
           local h = {} for i = 1, 200 do h[i * 1.5] = i h[-i] = i h["s" .. i] = i end
           local m = 0 for k, v in pairs(h) do m = m + v end
           for i = 1, 200 do h[i * 1.5] = nil end
           local m2 = 0 for k, v in pairs(h) do m2 = m2 + v end
           local g = {} for i = 1, 64 do g[i] = true end for i = 1, 64 do g[i] = nil end g[100] = 1
           return n, m, m2, next(g)"#,
        r#"local t = {10, 20, 30}
           local r = {}
           for i = 1, 5 do t[#t + 1] = i * 100 r[#r + 1] = #t end
           t[#t] = nil r[#r + 1] = #t
           local s = {} s[1] = 1 s[3] = 3 s[2] = 2 r[#r + 1] = #s
           table.sort(t, function(a, b) return a > b end) r[#r + 1] = table.concat(t, ",")
           r[#r + 1] = select(2, pcall(table.sort, {3, 1, 2}, function(a, b) return true end))
           r[#r + 1] = table.concat(table.move({1, 2, 3}, 1, 3, 2), ",")
           r[#r + 1] = table.concat(table.move({1, 2, 3}, 2, 3, 1), ",")
           return r"#,
        r#"local t = setmetatable({}, {__mode = "k"})
           local mt = {__index = {a = 1}}
           local objs = {} for i = 1, 50 do objs[i] = setmetatable({}, mt) end
           local s = 0 for i = 1, 50 do s = s + objs[i].a end
           mt.__index = {a = 2}
           for i = 1, 50 do s = s + objs[i].a end
           mt.__index = nil
           for i = 1, 50 do s = s + (objs[i].a or 0) end
           return s"#,
    ]);
}

#[test]
fn wide_registers() {
    // More than 255 locals (registers past the narrow window), with closures, calls,
    // loops and errors in that function.
    let mut decl = String::new();
    for i in 0..300 {
        decl.push_str(&format!("local v{i} = {i}\n"));
    }
    let prog = format!(
        r#"{decl}
        local function f(...) return select('#', ...), ... end
        local s = 0
        for i = 1, 20 do s = s + v299 + v0 + v150 v299 = v299 + 1 end
        local g = function() v280 = v280 * 2 return v280 + v1 end
        local t = {{v290, v291, f(v295, v296)}}
        local ok, e = pcall(function() return v270 .. {{}} end)
        local ok2, e2 = pcall(function() local x = v260 + nil end)
        local str = v299 .. "x" .. v1
        local mt = setmetatable({{}}, {{__index = function(_, k) return k + v298 end}})
        for k, v in pairs({{a = v297}}) do s = s + v end
        return s, g(), g(), #t, t[3], t[4], e, e2, str, mt[2], v299 < v298, v299 == v299"#
    );
    // C Lua allows 200 locals: it runs the same program with globals instead.
    equiv_vs(&prog, &prog.replace("local v", "v"));
}

/// Programs generated by the sweep-1 grammar, cut by fuel and memory (the fuzzer already
/// runs them fully three ways; this adds the cut points).
#[test]
fn generated_cut_points() {
    let n = std::env::var("SWEEP2_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(4u64);
    let seed = std::env::var("SWEEP2_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1u64);
    for i in 0..n {
        let mut r = rng::Rng::new(seed.wrapping_mul(7919).wrapping_add(i));
        let p = grammar::Gen::new(&mut r, false).program(6 + (i % 10) as i32).l;
        let full = both(&p, FUEL, 256 << 20);
        let used = full.1;
        for k in 0..60 {
            let _ = both(&p, used * k / 60, 256 << 20);
        }
        let mut hi = full.2.max(1 << 16);
        while hi > 4096 {
            let _ = both(&p, FUEL, hi);
            hi = hi * 7 / 8;
        }
    }
}

// ---- The op-matrix fuzzer: every fast-path operation over edge-case operands. ----

/// Shared definitions: edge values `V`, fresh tables `P(i)`, keys `K`, functions `F`, and
/// `show` (canonical text for any results). One line each, so error positions are stable.
const PRELUDE: &str = r#"local log = {}
local function show(...) local o = {} for i = 1, select('#', ...) do local v = select(i, ...) local t = type(v) if t == "number" then if v ~= v then o[i] = "nan" elseif math.type(v) == "integer" then o[i] = tostring(v) else o[i] = string.format("%.17g", v) .. "f" end elseif t == "string" then o[i] = string.format("%q", (v:gsub(": 0x%x+", ": ADDR"):gsub(": " .. string.rep("%x", 16), ": ADDR"))) elseif t == "table" then o[i] = "T" .. tostring(rawget(v, "id")) elseif t == "boolean" or t == "nil" then o[i] = tostring(v) else o[i] = t end end return table.concat(o, ",") end
local MT = {} for _, e in ipairs({"add","sub","mul","div","mod","pow","unm","idiv","band","bor","bxor","shl","shr","bnot","concat"}) do MT["__" .. e] = function(a, b) log[#log + 1] = e return e end end
MT.__eq = function() log[#log + 1] = "eq" return true end MT.__lt = function(a, b) log[#log + 1] = "lt" return false end MT.__le = function() log[#log + 1] = "le" return true end
MT.__len = function() return 7 end MT.__index = function(t, k) return type(k) == "number" and k or "ix" end MT.__call = function(self, ...) return select('#', ...), ... end
local Base = {id = 30, m = function(s, x) return (s.v or 0) + (x or 0) end, y = 2} Base.__index = Base
local Mid = setmetatable({id = 31, z = 3}, Base) Mid.__index = Mid
local LS = string.rep("L", 45)
local V = {0, 1, -1, 2, 7, -7, math.maxinteger, math.mininteger, math.maxinteger - 1, 0.0, -0.0, 0.5, -2.5, 1.5, 1e308, -1e308, math.huge, -math.huge, 0/0, 2^53, 2^63, -2^63, 1e15, 3.0, "10", "0x10", " 5 ", "1e2", "abc", "", "-0", "1.5", string.rep("a", 40), string.rep("a", 41), string.rep("b", 39) .. "c", true, false, setmetatable({id = 1}, MT), {id = 2, 1, 2, 3}, setmetatable({id = 3}, {__index = Base}), print, coroutine.create(print), "9007199254740993", "-9223372036854775808", 255, 64, 63, -64}
V[#V + 2] = 1
local function P(i)
  if i == 1 then return {id = 11, 1, 2, 3, x = 1, [LS] = 5, [1.5] = "h", [-1] = "n"}
  elseif i == 2 then return setmetatable({id = 12, 1}, MT)
  elseif i == 3 then return setmetatable({id = 13, v = 5}, Mid)
  elseif i == 4 then local back = {id = 140} return setmetatable({id = 14, 1}, {__newindex = back, __index = back})
  elseif i == 5 then return {id = 15, 1, nil, 3, nil, 5}
  elseif i == 6 then return setmetatable({id = 16}, {__newindex = function(t, k, v) rawset(t, k, v) log[#log + 1] = "ni" end, __index = function(t, k) return nil end})
  elseif i == 7 then return "abc"
  elseif i == 8 then return 5
  elseif i == 9 then return nil
  elseif i == 10 then local t = {id = 20} for j = 1, 20 do t[j] = j t["k" .. j] = j end return t
  else return setmetatable({id = 21}, {__index = setmetatable({}, {__index = setmetatable({}, {__index = Base})})}) end
end
local K = {1, 2, 3, 4, 0, -1, 1.0, 1.5, -0.0, 2^53, 0/0, "x", "y", "v", "m", LS, true, "len", "upper", "id", 6, 21, 2^63}
K[#K + 2] = 1
local F = {function(...) return ... end, function() end, function() return 1, 2, 3 end, function(a, ...) return select('#', ...), a end,
  function(...) local t = {...} return #t, ... end, function(n, ...) if n == nil then return ... end return select(2, ...) end,
  function(a) return a + 1 end, function(a) error(a) end, function(a) error(a, 2) end, function(a, b) return a .. b end,
  setmetatable({id = 50}, MT), function(...) return table.unpack({...}, 1, 300) end, function() return nil, nil end}
local FMT = {"%d", "%5.2f", "%g", "%.3g", "%e", "%.0e", "%10.4g", "%-8d|", "%x", "%q", "%s", "%.3s", "%c", "%+.1f", "%#g", "%o", "%i", "%.14g", "%5s", "%.20f"}
local PAT = {"a", "%a+", "(%w+)", "^a", "a$", ".-", "(.)%1", "%d*", "[^a]", "%f[%w]", "%b()", "()", "(a)(b)?", "[%a_][%w_]*", "%s*(.-)%s*$", "x*", "[a-c]+", "%%", ".", ""}
local STR = {"abc", "aaa", "  hi there  ", "a1b2c3", "(x(y)z)", "", string.rep("ab", 21), "a.b", "\0a\0", "THE end", "aabb"}
local r = {}
"#;

const BINOPS: &[&str] = &["+", "-", "*", "/", "//", "%", "^", "&", "|", "~", "<<", ">>", "..", "==", "~=", "<", "<=", ">", ">=", "and", "or"];
const UNOPS: &[&str] = &["-", "not ", "#", "~"];
const LITS: &[&str] = &["1", "-1", "0", "2", "7", "255", "2.5", "0.0", "-0.0", "1e308", "9223372036854775807", "0x7fffffffffffffff", "\"10\"", "\"x\"", "true", "nil", "3.0", "-7", "63", "64", "1e15", "0.1"];

fn stat(rng: &mut rng::Rng) -> String {
    let nv = 49; // #V (holes included)
    let v = |rng: &mut rng::Rng| rng.range(1, nv);
    let body = match rng.below(14) {
        0 => {
            let op = rng.pick(BINOPS);
            format!("local a, b = V[{}], V[{}] local x for _ = 1, 2 do x = a {op} b end return x", v(rng), v(rng))
        }
        1 => {
            let (op, l) = (rng.pick(BINOPS), rng.pick(LITS));
            if rng.pct(50) { format!("local a = V[{}] return a {op} {l}", v(rng)) } else { format!("local a = V[{}] return {l} {op} a", v(rng)) }
        }
        2 => format!("local a = V[{}] return {}a", v(rng), rng.pick(UNOPS)),
        3 => {
            let op = rng.pick(&["==", "~=", "<", "<=", ">", ">="]);
            format!("local a, b = V[{}], V[{}] local n = 0 for i = 1, 3 do if a {op} b then n = n + i else n = n - 1 end end return n", v(rng), v(rng))
        }
        4 => {
            let k = rng.range(1, 24);
            match rng.below(4) {
                0 => format!("local t, k = P({}), K[{k}] local s for _ = 1, 3 do s = t[k] end return s", rng.range(1, 11)),
                1 => format!("local t = P({}) return t.x, t.v, t.y, t.z, t.m, t.id", rng.range(1, 11)),
                2 => format!("local t = P({}) return t:m({}), t:len()", rng.range(1, 11), rng.pick(LITS)),
                _ => format!("local t = P({}) local s = 0 for i = 1, 4 do s = s + (t[i] or 0) end return s, #t", rng.range(1, 11)),
            }
        }
        5 => {
            // (No `#t` after writes: with holes, which border `#` finds differs from C Lua,
            // finding EQ-1.)
            let (t, k) = (rng.range(1, 11), rng.range(1, 24));
            format!("local t, k = P({t}), K[{k}] for i = 1, 2 do t[k] = V[{}] end t.x = V[{}] t[4] = 9 return t[k], t.x, rawget(t, k), t[4]", v(rng), v(rng))
        }
        6 => {
            let n = rng.range(2, 5);
            let ops: Vec<String> = (0..n).map(|_| if rng.pct(70) { format!("V[{}]", v(rng)) } else { (*rng.pick(LITS)).to_owned() }).collect();
            format!("return {}", ops.join(" .. "))
        }
        7 => {
            let (a, b) = (rng.pick(LITS), rng.pick(LITS));
            let c = if rng.pct(50) { (*rng.pick(&["1", "-1", "2", "0.5", "-0.25", "0", "math.maxinteger", "math.mininteger", "0.0", "\"2\""])).to_owned() } else { format!("V[{}]", v(rng)) };
            format!("local n, last = 0 for i = {a}, {b}, {c} do n = n + 1 last = i if n > 4 then break end end return n, last")
        }
        8 => {
            let f = rng.range(1, 13);
            let args: Vec<String> = (0..rng.below(4)).map(|_| format!("V[{}]", v(rng))).collect();
            let args = args.join(", ");
            match rng.below(5) {
                0 => format!("return F[{f}]({args})"),
                1 => format!("return select('#', F[{f}]({args}))"),
                2 => format!("local t = {{F[{f}]({args})}} return #t, t[1]"),
                3 => format!("local a, b, c = F[{f}]({args}) return a, b, c"),
                _ => format!("local function g(...) return F[{f}](...) end return g({args})"),
            }
        }
        9 => {
            let act = rng.pick(&["t[k] = nil", "t[k] = v * 2", "if type(k) == 'number' then t[k] = nil end", "t[k] = v"]);
            format!("local t = P(10) local n, s = 0, 0 for k, v in pairs(t) do n = n + 1 s = s + v {act} end local m = 0 for k, v in next, t do m = m + v end return n, s, m")
        }
        // (NaN's text is the C library's.)
        10 => format!("local x = V[{}] if x ~= x then return 'nan' end return string.format(FMT[{}], x)", v(rng), rng.range(1, 20)),
        11 => {
            let (s, p) = (rng.range(1, 11), rng.range(1, 20));
            match rng.below(4) {
                0 => format!("return STR[{s}]:find(PAT[{p}], {})", rng.range(-3, 5)),
                1 => format!("return STR[{s}]:match(PAT[{p}])"),
                2 => format!("return STR[{s}]:gsub(PAT[{p}], {})", rng.pick(&["\"<%0>\"", "\"%1\"", "string.upper", "{a = 1, b = false}", "\"%%\"", "function(c) return nil end"])),
                _ => format!("local o = {{}} for a, b in STR[{s}]:gmatch(PAT[{p}]) do o[#o + 1] = tostring(a) .. tostring(b) if #o > 20 then break end end return table.concat(o, '|')"),
            }
        }
        12 => {
            let s = rng.range(1, 11);
            format!("local s = STR[{s}] .. V[{}] return #s, s:sub({}, {}), s:upper(), s:rep({}, '-'), s:byte(1, 3)", v(rng), rng.range(-50, 50), rng.range(-50, 50), rng.range(-1, 3))
        }
        _ => {
            // A closure over a loop variable and an upvalue written from a nested function.
            format!("local acc = V[{}] local fs = {{}} for i = 1, 3 do fs[i] = function() acc = acc + i return acc end end return fs[1](), fs[3](), acc", v(rng))
        }
    };
    format!("r[#r + 1] = show(pcall(function() {body} end))\n")
}

fn matrix_program(rng: &mut rng::Rng, n: usize) -> String {
    let mut p = String::from(PRELUDE);
    for _ in 0..n {
        p.push_str(&stat(rng));
    }
    p.push_str("return table.concat(r, '\\n') .. '\\n' .. table.concat(log, ',')\n");
    p
}

/// Random op-matrix programs, three ways, with fuel and memory cut points.
/// `SWEEP2_MATRIX` programs (default 40), seed `SWEEP2_SEED`.
#[test]
fn op_matrix_fuzz() {
    let n = std::env::var("SWEEP2_MATRIX").ok().and_then(|v| v.parse().ok()).unwrap_or(40u64);
    let seed = std::env::var("SWEEP2_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(1u64);
    for i in 0..n {
        let mut r = rng::Rng::new(seed.wrapping_mul(0x5851_F42D).wrapping_add(i));
        let p = matrix_program(&mut r, 30);
        let full = both(&p, FUEL, 256 << 20);
        {
        let c = common::oracle(&p, FUEL);
            if full.0 != c {
                let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
                std::fs::write(dir.join("sweep2-matrix-fail.lua"), &p).unwrap();
                let text = |r: &Result<String, String>| r.clone().unwrap_or_else(|e| e);
                let (a, b) = (text(&full.0), text(&c));
                let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
                let d = (0..la.len().max(lb.len())).find(|&j| la.get(j) != lb.get(j)).unwrap_or(0);
                let stat = p.lines().nth(PRELUDE.lines().count() + d).unwrap_or("");
                panic!("program {i} (seed {seed}) differs from C Lua at result {d}:\n{stat}\nours: {:?}\nC:    {:?}", la.get(d), lb.get(d));
            }
        }
        for k in 0..40 {
            let _ = both(&p, full.1 * k / 40 + r.below(50) as u64, 256 << 20);
        }
        for k in 0..10 {
            let _ = both(&p, FUEL, full.2 * (k + 4) / 14);
        }
        // A collection at every allocation changes no result, either way.
        if i % 8 == 0 {
            let (f, s) = (run_with(&p, FUEL, 256 << 20, true, true), run_with(&p, FUEL, 256 << 20, false, true));
            assert_eq!(f, s, "program {i} under collector stress: fast loop and full path differ");
            assert_eq!(f.0, full.0, "program {i} under collector stress: result changed");
        }
    }
}

/// One program per finding fixed in this sweep.
#[test]
fn regressions() {
    let _heavy = super::common::memory_heavy();
    all(&[
        // EQ-2: `rawset`'s nil/NaN key error has no position (raised inside the C function).
        r#"local function e(f) return select(2, pcall(f)) end
           return e(function() rawset({}, 0/0, 1) end), e(function() rawset({}, nil, 1) end),
             e(function() local t = setmetatable({}, {__newindex = function(t, k, v) rawset(t, k, v) end}) t[0/0] = 1 end),
             e(function() local t = {} t[0/0] = 1 end), select(2, pcall(rawset, {}, nil, 1))"#,
        // EQ-3: a `..` type error after a `__concat` returned names its operand.
        r#"local function e(f) return select(2, pcall(f)) end
           local m = setmetatable({}, {__concat = function() return {} end})
           local V = {{}}
           return e(function() return V[1] .. 1 .. m end), e(function() local a = {} return "x" .. a .. m end),
             e(function() return "x" .. m .. "y" end), e(function() local x = {} return x .. m .. m end)"#,
        // EQ-4: `next`'s invalid-key error has no position, called or as a `for` iterator.
        r#"local function e(f) return select(2, pcall(f)) end
           return e(function() return next({}, 1) end), e(function() for k in next, {}, 1 do end end),
             e(function() local t = {a = 1} for k in pairs(t) do t[k] = nil t.zz, t.yy, t.xx = 1, 2, 3 end end),
             select(2, pcall(next, {}, "x"))"#,
        // EQ-5: `load` takes a number as a string chunk.
        r#"return select(2, load(1)), select(2, load(1.5, "=x")), load(12) == nil, load(7.0, "=n", "b")"#,
        // EQ-6: line numbers as C Lua gives them: a call is on the line where its arguments
        // start (a string argument: where it ends), also for methods and tail calls; a `..`
        // chain is on the line of its last `..`.
        r#"local function run(s) local f, e = load(s, "=c") if not f then return e end return select(2, pcall(f)) end
           return run("local t = {}\nreturn t[[\nx]]"), run("local t = {}\nreturn t\"\\z\n   x\""),
             run("local a\nreturn 'x' .. a ..\n 'y' .. 'z'"), run("local a\nreturn a .. 'x' ..\n 'y'"),
             run("local t = {}\nreturn t\n(1)"), run("local t = {}\nlocal x = t\n(1)"), run("local t = {}\nreturn t{\n}"),
             run("local t = {x = 1}\nlocal y = t:x\n(1)"), run("local t = {}\nreturn t:x\n(1)"),
             run("local t = {}\nlocal y = (t\n'a')")"#,
        // EQ-7: syntax messages: after `return`, the enclosing block's terminator is what is
        // expected; a string token is quoted with its escapes applied (cut at `\0`) and on
        // the line where it ends.
        r#"local out = {}
           for _, s in ipairs({"local function f() return 1 ) end", "function o()\nreturn 'x' o", "return 1 2",
               "if x then return end end", "while x do return 1 x = 2 end", "repeat return until x y",
               'local "\\0"', 'return "\\9" "\\0659"', 't { 3"\\z"', "g[ function() '\\\\'", 'return 1 [[a\nb]]',
               'return 1 "x\\ny"', 'x = 1 "\\x41\\u{42}"', "f(\n'a\\\nb' 1)"}) do
             out[#out + 1] = select(2, load(s, "=c"))
           end
           return table.concat(out, "\n")"#,
        // EQ-8: the table library's table check (`checktab`): a string has `__index` but no
        // `__len`, so it is refused before anything else; `load` asks for a function.
        r#"local function e(...) return select(2, pcall(...)) end
           return e(table.insert, "a"), e(table.insert, "a", 1), e(table.concat, "}"), e(table.remove, "x"),
             e(table.sort, "abc"), e(table.move, "abc", 1, 1, 1), e(table.unpack, "ab"), e(load), e(load, nil),
             table.concat(setmetatable({}, {__len = function() return 2 end, __index = function(t, i) return i end}))"#,
        // EQ-12: a pending `goto`/`break` is reported where its function ends; a long
        // string is quoted without the line break after its opening bracket.
        r#"local out = {}
           for _, s in ipairs({"k = 1 if 4 then break end\nfunction o() end\nx = 1", "local function f()\n  goto L\n  local x = 1\nend",
               "do break end", "for i = 1, 2 do local f = function() break end end", "x = 1 [[\n\na]]", "x = 1 [==[\r\nb]==]"}) do
             out[#out + 1] = select(2, load(s, "=c"))
           end
           return table.concat(out, "\n")"#,
        // EQ-14: lexer errors in a short string quote C Lua's buffer (escapes applied so far,
        // then the bad escape as written) on the current line; a parameter list's `)` is a
        // plain check.
        r#"local out = {}
           for _, s in ipairs({'"\\\\Lua\\\nreturn\n', "'\"\\z  \n__emit(g1)\n", '"Lua \\z  \n197\n', '"\\z  \n   \\}',
               "'\\z  \n   \\ x6b", '"a\\65\\300"', '"\\x4g"', '"ab\\u{110000000}"', "( function(p408\nif", '"abc', '"\\0abc\n'}) do
             out[#out + 1] = select(2, load(s, "=c"))
           end
           return table.concat(out, "\n")"#,
        // EQ-13: C Lua 5.4.7 library details: a pack format ends at a NUL byte; `randomseed`
        // wants an integer; a call through a constant integer key outside 0..255 is "field '?'".
        r#"local function e(...) return select(2, pcall(...)) end
           local fs = {}
           return string.packsize("\0"), string.packsize("i4\0i8"), e(string.pack, "\0z"), e(math.randomseed, 2^63, 2),
             e(math.randomseed, 2.5), (e(function() return fs[1000000]() end)), (e(function() return fs[255]() end)),
             (e(function() return fs[-1]() end)), (e(function() return fs[256]() end))"#,
        // EQ-9: calling a non-function as the generic `for` iterator names it as C Lua does.
        r#"local function e(f) return select(2, pcall(f)) end
           local t = {d = 4}
           return e(function() for k in (t) do end end), e(function() for k in nil do end end),
             e(function() for w in ("abc"):gmatch "x"() do end end)"#,
    ]);
}

/// The collector running while the fast loop writes new objects into old tables and
/// upvalues (the barriers), appends, builds short strings and closures; then everything
/// written is read back. Run with the incremental collector at its most eager settings too.
#[test]
fn collector_under_fast_writes() {
    let _heavy = super::common::memory_heavy();
    for setup in ["", "collectgarbage('incremental', 10, 400)", "collectgarbage('generational')"] {
        equiv(&format!(
            r#"{setup}
            local keep, arr = {{}}, {{}}
            local up = {{}}
            local function setup(v) up = v end
            local get = function() return up end
            local mt = {{__index = {{tag = "cls"}}}}
            for i = 1, 3000 do
              local t = {{i, name = "n" .. (i % 37)}}
              keep[i % 101 + 1] = t
              keep["k" .. (i % 53)] = {{i}}
              arr[#arr + 1] = setmetatable({{v = i}}, mt)
              if #arr > 200 then arr = {{arr[200]}} end
              setup({{i}})
              local f = function() return i end
              keep[(i % 7) + 200] = f
              if i % 500 == 0 then collectgarbage("step", 1) end
            end
            local s = 0
            for j = 1, 101 do local t = keep[j] if t then s = s + t[1] + #t.name end end
            for j = 0, 52 do s = s + keep["k" .. j][1] end
            for j = 200, 206 do s = s + keep[j]() end
            for _, o in ipairs(arr) do s = s + o.v + #o.tag end
            return s, get()[1], #arr"#
        ));
    }
}

/// Coroutines whose bodies run in the fast loop, wide and narrow frames calling each other
/// with varargs and multiple results, yields from inside loops and `pairs`.
#[test]
fn coroutines_and_mixed_widths() {
    let mut decl = String::new();
    for i in 0..270 {
        decl.push_str(&format!("local w{i} = {i} "));
    }
    let wide = format!("local function wide(...) {decl} local n = select('#', ...) local a, b = ... return n, a, b, w269 + w0, ... end");
    let prog = format!(
        r#"{wide}
        local function narrow(...) return wide(...) end
        local co = coroutine.wrap(function(...)
          local acc = 0
          for i = 1, 5 do
            local t = {{wide(i, nil, i * 2)}}
            acc = acc + #t + t[4]
            local got = {{coroutine.yield(acc, narrow(i))}}
            acc = acc + (got[1] or 0)
          end
          for k, v in pairs({{a = 1, b = 2, c = 3}}) do acc = acc + v coroutine.yield(k == nil) end
          return "done", acc
        end)
        local out = {{}}
        for i = 1, 9 do out[#out + 1] = table.concat({{tostring(co(i * 10))}}, ",") end
        local ok, e = pcall(co)
        out[#out + 1] = tostring(ok) .. " " .. tostring(e)
        local c2 = coroutine.create(function() local x = wide() error({{code = x}}) end)
        local ok2, e2 = coroutine.resume(c2)
        out[#out + 1] = tostring(ok2) .. " " .. type(e2) .. " " .. tostring(e2.code) .. " " .. coroutine.status(c2)
        return table.concat(out, ";")"#
    );
    let in_c = prog.replace("local w", "w");
    equiv_vs(&prog, &in_c);
}

/// Recursion until the call-depth limit, through each kind of call the fast loop places
/// (Lua calls, `pairs`/`ipairs`/`next` loops, natives, `__index` functions, `__call`):
/// the same stop, message, fuel and memory with the fast loop on and off, and the state
/// still usable after.
#[test]
fn depth_limit_through_fast_calls() {
    for body in [
        "return f(n + 1) + 1",
        "for k in pairs({1}) do end return f(n + 1) + 1",
        "for i, v in ipairs({1}) do end return (f(n + 1))",
        "for k in next, {1} do end local x = #tostring(n) return f(n + x) + 1",
        "return setmetatable({}, {__index = function() return f(n + 1) end}).x",
        "return setmetatable({}, {__call = function() return f(n + 1) + 1 end})()",
        "return select(2, pcall(f, n + 1))",
        "return string.gsub('a', 'a', function() return f(n + 1) end)",
    ] {
        let prog = format!("local f f = function(n) {body} end return f(1)");
        // At a set call depth.
        let r = both_with(&prog, FUEL, 256 << 20, Limits::default().call_depth(2000));
        assert!(r.0.as_ref().is_ok_and(|s| s.contains("overflow") || s.contains("error")), "{body}: {r:?}");
        // At the default one, which memory bounds (here 8 MiB): the frames filling it are a
        // stack overflow; the tables some bodies make at each level fill it first, which is
        // the memory limit.
        let r = both_with(&prog, FUEL, 8 << 20, Limits::default());
        let ok = r.0.as_ref().is_ok_and(|s| s.contains("overflow") || s.contains("error"))
            || r.0.as_ref().is_err_and(|e| e.contains("too much memory"));
        assert!(ok, "{body}: {r:?}");
    }
}

/// `both` with other limits than the default.
fn both_with(program: &str, fuel: u64, memory: usize, limits: Limits) -> Run {
    let one = |fast: bool| {
        let mut lua = Lua::new(limits.fuel(fuel).memory(memory).hash_seed(common::TEST_SEED), Libs::ALL);
        lua.set_fast_loop(fast);
        let r = match lua.exec(common::wrap(program), "=wrapper") {
            Ok(v) => Ok(v.first().and_then(|x| lua.to_string_lossy(*x)).unwrap_or_default()),
            Err(e) => Err(e.to_string()),
        };
        (r, fuel.saturating_sub(lua.fuel_left()), lua.memory_used())
    };
    let (f, s) = (one(true), one(false));
    assert_eq!(f, s, "fast loop and full path differ:\n{program}");
    f
}

/// Lines C Lua gives instructions and compile errors (EQ-10, EQ-16, EQ-17), and the
/// order of compile and syntax errors (EQ-15).
#[test]
fn lines_and_error_order() {
    all(&[
        // EQ-10: a store is numbered where its values end, a comparison where its right
        // side ends; EQ-17: `error(msg, 2)` in a metamethod names the line that ran it.
        r#"local function run(s) local f, e = load(s, "=c") if not f then return e end return select(2, pcall(f)) end
           local cases = {
               "local g2 = 1\ng2['k'] = function()\n  return 1\nend",
               "local t = 1\nt.x, t.y = 1,\n 2",
               "local t = 1\nt.x = {\n}",
               "return {} <\n{}",
               "return {}\n<\n{}",
               "if {} < {\n} then end",
               "local a = {} return a >= (\n{})",
               "local a = {} return a == a and a <\n a",
               "local t = 1\nt.x = 'a' ..\n 'b'",
               "local t = setmetatable({}, {__newindex = function() error('ni', 2) end})\nt.x = 1 +\n 2",
               "local t = {}\nreturn t[[\nx]]",
               "local t = {}\nreturn t\n(1)",
               "local t = {}\nreturn t:m\n(1)",
               "local f = 1\nlocal x = f(\n1)",
           }
           local out = {}
           for i, c in ipairs(cases) do out[i] = tostring(run(c)) end
           return table.concat(out, "\n")"#,
        r#"local function e(f) return select(2, pcall(f)) end
           return e(function() local t = setmetatable({}, {__newindex = function() error('ni', 2) end}) t.x = 1 end),
             e(function() local t = setmetatable({}, {__index = function() error('ix', 2) end}) return t.x end),
             e(function() local t = setmetatable({}, {__add = function() error('ad', 2) end}) return t + 1 end),
             e(function() local t = setmetatable({}, {__call = function() error('cc', 2) end}) t() end),
             e(function() do local x <close> = setmetatable({}, {__close = function() error('cl', 2) end}) end end),
             e(function() return tostring(setmetatable({}, {__tostring = function() error('ts', 2) end})) end)"#,
        // EQ-16: label errors give the line of the token after the label (and the labels
        // and `;` after it, which C Lua checks first).
        r#"local cases = {
               "::L6::\ndo\nx = 1\nend\n::L6::\n\ny = 1",
               "::a::\n::a::",
               "::a::\n::b::\n;\n::a::\n::b::\n\n\nz = 1",
               "do goto top end\nlocal n\n::top::\n\nn = n",
               "do goto top end\nlocal n\n::top::\n;\n::other::\nn = n",
               "while x do goto t end local q ::t:: ;\n\nprint(1)",
               "repeat goto t local q ::t:: until\nq",
               "do goto t local q ::t:: end",
               "::x:: do ::y:: ::x:: end",
           }
           local out = {}
           for i, c in ipairs(cases) do local f, e = load(c, "=c") out[i] = tostring(e) end
           return table.concat(out, "\n")"#,
        // EQ-18: the generic `for` call is numbered where the token after `in` ends; a NUL
        // byte token has no "near" in syntax messages; an escape error at the end of the
        // text still quotes the string read so far.
        r#"local cases = {
             "for--x\ni in iter do end",
             "for k in\nnil do end",
             "for k, v in next,\n{}, 1 do end",
             "return 1 > \0",
             "local \0",
             "for i = 1, 2, i \0",
             "x = '\\z  \nhello\\u{0",
             "x = 'ab\\x4",
             "x = 'ab\\300",
             "x = 'ab\\",
             "x = 'ab\\u{110000000",
           }
           local out = {}
           for i, c in ipairs(cases) do
             local f, e = load(c, "=c")
             if f then e = select(2, pcall(f)) end
             out[i] = tostring(e)
           end
           return table.concat(out, "\n")"#,
        // EQ-19: a numeric or generic `for`'s preparation is numbered at its `do`; `os.date`
        // shows the format from the bad conversion on; `.0x` starts a hexadecimal numeral as
        // in C Lua.
        r#"local cases = {
             "for i = 1, 10,\n0 do end",
             "for i = 1, 10, 0\ndo end",
             "for i = {}, 2\ndo end",
             "for i = 1, 'x',\n1 do end",
             "for k in next, {}, nil, 1\ndo end",
             "return os.date('% MMS', 0)",
             "return os.date('%Ez rest', 0)",
             "return os.date('ab%', 0)",
             "return os.date('%q\\0zz', 0)",
             "return .0x.p1",
             "return .0x1p4",
             "return 0x.8p1, .5e2, 3.e-1",
           }
           local out = {}
           for i, c in ipairs(cases) do
             local f, e = load(c, "=c")
             if f then e = select(2, pcall(f)) end
             out[i] = tostring(e)
           end
           return table.concat(out, "\n")"#,
        // EQ-21: a field read is numbered where its key ends.
        r#"local function run(s) local f, e = load(s, "=c") if not f then return e end return select(2, pcall(f)) end
           return run("return x.\ny"), run("return x\n.y"), run("local t = {}\nreturn t.a\n.b"), run("return x[\n1\n]"),
             run("local v = x.\ny + 1"), run("return x.\ny()"), run("local t = {}\nreturn t.a.\nb.c")"#,
        // EQ-15: whichever error C Lua meets first: a compile error in what was read before
        // the syntax error, or the syntax error.
        r#"local cases = {
               "( function()\nif true then break end\nend\ng3",
               "function f() goto x end )",
               "local x <const> = 1\nx = 2\n)",
               "local x <const> = 1\nx = 2 +",
               "::a:: ::a:: )",
               "do goto top end\nlocal n\n::top::\n)",
               "local function f()\n  goto L\nend\nlocal y = = 1",
               "for i = 1, 2 do end break ;; )",
               "while true do local f = function() break end end x",
               "local t <const> = {} t.x = 1 t = 2 return",
               "local c <close> = nil c = 1 local",
               "\"unfinished\nlocal x <const> = 1 x = 2",
               "local x <const> = 1 x = 2 \"unfinished\n",
               "( function()\nif true then break\nend\ng3",
               "function f() goto x )",
               "local x <const> = 1\nx = )",
               "local x <const> = 1\nx\n)",
               "::a:: ) ::a::",
               "while x do break )",
               "repeat goto q until )",
               "local x <const> = 1 local function g() x = 2",
               "f( function() x = 1 break end",
               "local a = { function() goto z end",
               "for k in",
               "local x <const> = 1 for x in pairs({}) do x = 1 end y y",
           }
           local out = {}
           for i, c in ipairs(cases) do local f, e = load(c, "=c") out[i] = tostring(e) end
           return table.concat(out, "\n")"#,
    ]);
}

/// `check()`'s diagnostics: a start and an end (line and column each, the end just past
/// the text) and the line Lua's message gives, never an end line with a start column.
#[test]
fn check_diagnostic_spans() {
    use remedy_lua::{Pos, check};
    let p = |line, col| Pos { line, col };
    // (source, start, end, Lua's line, message start)
    let cases: &[(&str, Pos, Pos, u32, &str)] = &[
        // A short string over two lines (escaped newline) where a statement can't go on.
        ("x = 1 \"a\\\nbc\"", p(1, 7), p(2, 4), 2, "unexpected symbol near"),
        // A long string spanning lines.
        ("x = 1 [[\nab\ncd]] y", p(1, 7), p(3, 5), 3, "unexpected symbol near"),
        // After a long comment over three lines.
        ("--[[ one\ntwo\nthree ]] x x", p(3, 12), p(3, 13), 3, "syntax error near 'x'"),
        // An unfinished long string: from its start to the end of the text.
        ("x = [[ab\ncd", p(1, 5), p(2, 3), 2, "unfinished long string"),
        // An unfinished long comment.
        ("x = 1 --[==[ab\ncd\n", p(1, 7), p(3, 1), 3, "unfinished long comment"),
        // A string broken by a newline: from its quote to the line end.
        ("x = 'ab\ny'", p(1, 5), p(1, 8), 1, "unfinished string"),
        // A bad escape after a continued line: the escape as written.
        ("x = \"a\\z  \n  \\q\"", p(2, 3), p(2, 5), 2, "invalid escape sequence"),
        // One token, one line.
        ("local = 1", p(1, 7), p(1, 8), 1, "<name> expected near '='"),
        // The end of the text.
        ("f(", p(1, 3), p(1, 3), 1, "unexpected symbol near <eof>"),
    ];
    let mut bad = Vec::new();
    for (src, start, end, line, msg) in cases {
        let d = check(src.as_bytes(), "t");
        assert_eq!(d.len(), 1, "{src:?}: {d:?}");
        let d = &d[0];
        assert!(d.end >= d.pos, "{src:?}: end before start");
        if !d.message.starts_with(msg) || (d.pos, d.end, d.line) != (*start, *end, *line) {
            bad.push(format!("{src:?}: {d:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
