//! Strings: short ones (up to 40 bytes) interned, long ones made without hashing and
//! interned only as table keys, and `..` building each result once. Each program runs in
//! RemedyLua with the fast loop on and off (results, fuel and memory identical) and is
//! compared with C Lua.

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua};

use super::common;

fn same_as_c(program: &str) {
    let ours = common::remedy(program, 100_000_000);
    assert!(ours.as_ref().is_ok_and(|s| !s.starts_with("error")), "{ours:?}");
    let c = common::oracle(program, 100_000_000);
    {
        assert_eq!(ours, c, "{program}");
    }
}

/// The C library under the oracle drops the zeros `#` must keep when `%#g` rounds up to
/// a new power of ten (`%#.5g` of 99999.95 is `1.e+05` from glibc 2.39; C11 7.21.6.1 and
/// MSVC give `1.0000e+05`). True when the oracle's C library has that defect.
fn oracle_libc_drops_hash_zeros() -> bool {
    common::oracle("return string.format('%#.5g', 99999.95)", 1_000_000).is_ok_and(|s| s.contains("1.e+05"))
}

/// `c` is the defective form of the conforming `ours` (S5COR-1): `1.e+05` for `1.0000e+05`.
fn hash_zeros_only(ours: &str, c: &str) -> bool {
    let (o, c) = (ours.trim(), c.trim());
    let Some((c_head, c_exp)) = c.split_once(".e") else { return false };
    let Some((o_head, o_rest)) = o.split_once('.') else { return false };
    let Some((zeros, o_exp)) = o_rest.split_once('e') else { return false };
    c_head == o_head && c_exp == o_exp && !zeros.is_empty() && zeros.bytes().all(|b| b == b'0')
}

/// [`same_as_c`] for programs whose results are `string.format` outputs joined by `,`:
/// every item must match C Lua, except the `%#g` items the oracle's C library gets wrong
/// (only when it is shown to have that defect), and those must be the conforming form.
fn same_as_c_printf(program: &str) {
    let ours = common::remedy(program, 100_000_000);
    assert!(ours.as_ref().is_ok_and(|s| !s.starts_with("error")), "{ours:?}");
    let c = common::oracle(program, 100_000_000);
    let (Ok(o), Ok(t)) = (&ours, &c) else { panic!("{ours:?} / {c:?}") };
    if o == t {
        return;
    }
    let defect = oracle_libc_drops_hash_zeros();
    let (oi, ti): (Vec<&str>, Vec<&str>) = (o.split(',').collect(), t.split(',').collect());
    assert_eq!(oi.len(), ti.len(), "{program}");
    for (a, b) in oi.iter().zip(&ti) {
        assert!(a == b || (defect && hash_zeros_only(a, b)), "ours {a:?}, C Lua {b:?} in {program}");
    }
}

/// S5COR-1: `%#g` keeps its trailing zeros when rounding carries into a new exponent, as
/// C11 requires, whatever the oracle's C library does.
#[test]
fn s5cor_1_hash_g_keeps_zeros_after_carry() {
    let r = common::eval(
        "return string.format('%#.5g|%#g|%#.5g|%#.3g|%#.1g', 99999.95, 999999.5, 1e5, 9.995, 9.5)",
    )
    .unwrap();
    assert_eq!(r, ["1.0000e+05|1.00000e+06|1.0000e+05|9.99|1.e+01"]);
}

/// Equal long strings made apart are different objects: as table keys (get, set, rawget,
/// rawset, next, `__index` and `__newindex` checks) and in `==`/`rawequal` they still act
/// as one value, across the 40/41-byte boundary and after collections.
#[test]
fn long_strings_equal_by_content() {
    same_as_c(
        r#"
        local r = {}
        for _, n in ipairs({39, 40, 41, 42, 100, 5000}) do
            local a = string.rep("x", n - 1) .. "y"
            local b = string.rep("x", n - 1) .. "y"
            local c = string.rep("x", n - 1) .. "z"
            local t = {}
            t[a] = 1
            r[#r + 1] = {t[b], a == b, a ~= c, rawequal(a, b), rawget(t, b), t[c]}
            t[b] = 2
            rawset(t, b, 3)
            local count = 0
            for k, v in pairs(t) do count = count + 1 end
            r[#r + 1] = {count, t[a], next(t, b) == nil}
            local seen = 0
            local w = setmetatable({}, {__newindex = function() seen = seen + 1 end})
            rawset(w, a, 1)
            w[b] = 5
            local p = setmetatable({}, {__index = t})
            r[#r + 1] = {seen, w[a], p[b], ({[a] = 7})[b]}
            collectgarbage()
            local d = string.rep("x", n - 1) .. "y"
            r[#r + 1] = {t[d], d == a, #(a .. b)}
        end
        return r
        "#,
    );
}

/// `..` over numbers, short and long parts: exact text and lengths, one run of operands
/// joined at once, `__concat` still reached for tables.
#[test]
fn concat_formats_and_joins() {
    same_as_c(
        r#"
        local x = string.rep("q", 60)
        local mt = {__concat = function(a, b) return "M" end}
        local t = setmetatable({}, mt)
        return math.mininteger .. "|" .. math.maxinteger .. "|" .. 0 .. "|" .. -7 .. "|"
                 .. 1e300 .. "|" .. -0.0 .. "|" .. 1 / 0 .. "|" .. 3.5,
            string.rep("a", 39) .. 1, string.rep("a", 39) .. 12, 9 .. "",
            (x .. 1 .. x) == (x .. "1" .. x), #(x .. x .. 12345 .. -1.5),
            "a" .. t, t .. x, x .. 1 .. t
        "#,
    );
}

/// Building long strings over and over: memory is charged before each one is made and
/// returned in full when they are collected (the accounting stays exact), and a result past
/// the memory cap is a memory error before it is built.
#[test]
fn long_concat_memory_is_exact() {
    let mut lua = Lua::new(Limits::default().fuel(1_000_000_000).memory(64 << 20), Libs::ALL);
    lua.exec("big = string.rep('x', 1000)", "=t").unwrap();
    lua.collect_garbage().unwrap();
    let base = lua.memory_used();
    lua.exec("local s = '' for i = 1, 2000 do s = s .. big .. i end", "=t").unwrap();
    lua.exec(
        "local s for i = 1, 200 do local t = {big, i, 1.5, big} s = table.concat(t, ',') .. big:gsub('x', 'yy', i)
           .. string.format('%s%d', big, i) .. big:sub(2) .. big:upper() .. big:rep(2, '-') end",
        "=t",
    )
    .unwrap();
    lua.collect_garbage().unwrap();
    assert_eq!(lua.memory_used(), base);

    let mut small = Lua::new(Limits::default().fuel(1_000_000_000).memory(4 << 20), Libs::ALL);
    let r = small.exec("local s = string.rep('x', 3 << 20) return #(s .. s)", "=t");
    assert!(matches!(r, Err(Error::Limit(LimitKind::Memory))), "{r:?}");
    assert_eq!(small.exec("return #('a' .. 'b')", "=t").map(|v| v.len()).ok(), Some(1));
}

/// `%g`, `%e` and `%f` in every form (and `tostring` of floats) over many values, rounding edges
/// included: the fixed form is built from the same rounded digits as the exponent form.
#[test]
fn g_format_matches_c() {
    same_as_c_printf(
        r#"
        local seed = 12345
        local function rnd() seed = (seed * 1103515245 + 12345) % 2147483648 return seed end
        local fmts = {"%.14g", "%g", "%.3g", "%#g", "%.17g", "%#.5g", "%.1g", "%.0g", "%#.1g", "%10.4g", "%-12.6G", "%+.8g",
            "%e", "%.3e", "%#.0e", "%E", "%.20e", "%f", "%.0f", "%#.0f", "%.3f", "%10.2f", "%-+12.4e", "%.99f"}
        local vals = {0.5, 0.05, 0.0001, 0.00001, 0.000123456, 9.9999995, 99999.95, 999999.5, 1e15, 1e14,
            123456789012345, 0.1, 1/3, 2/3, -1.5, -0.0001234, 100, 1e-5, 9.5, 0.95, 0.995, 99.5, 1e100, 2^53, -2^63}
        for i = 1, 1500 do
            vals[#vals + 1] = (rnd() % 2 == 0 and -1 or 1) * (rnd() / 2147483648) * 10.0 ^ (rnd() % 40 - 20)
            vals[#vals + 1] = (rnd() % 100000) / (10 ^ (rnd() % 8))
        end
        local out = {}
        for _, v in ipairs(vals) do
            for _, f in ipairs(fmts) do out[#out + 1] = string.format(f, v) end
            out[#out + 1] = tostring(v) .. tostring(math.tointeger(v) or 0)
        end
        return table.concat(out, ",")
        "#,
    );
}

/// The library paths that build results in place: `sub`/`upper`/`lower`/`reverse` short
/// and long, single and ranged `byte`, and `gsub` with every kind of replacement (one
/// matcher reused across positions; captures handed to functions and tables).
#[test]
fn derived_strings_and_gsub() {
    same_as_c(
        r##"
        local s = "The quick brown fox jumps over the lazy dog, then again and again"
        local r = {s:sub(5, 9), s:sub(-3), s:sub(0), s:sub(10, 5), s:sub(3, 100), s:sub(1, 41) == s:sub(1, 41),
            s:upper(), s:lower():sub(1, 12), s:reverse(), ("ab"):reverse(), (""):upper(), s:byte(1), s:byte(-1),
            s:byte(100), select("#", s:byte(1, 3)), tostring(12345), tostring(-7), tostring(true)}
        local t = "hello world from Lua, key=val; a=b"
        r[#r + 1] = {t:gsub("(%w+)=(%w+)", "%2=%1")}
        r[#r + 1] = {t:gsub("%w+", {hello = "HI", Lua = false})}
        r[#r + 1] = {t:gsub("()(o)", function(p, c) return p .. c:upper() end)}
        r[#r + 1] = {t:gsub("", "-")}
        r[#r + 1] = {t:gsub("^h", "H")}
        r[#r + 1] = {t:gsub("o", "0", 2)}
        r[#r + 1] = {t:gsub("x*", "!")}
        r[#r + 1] = {t:gsub("%a", string.upper)}
        r[#r + 1] = {pcall(string.gsub, t, "o", "%2")}
        r[#r + 1] = {t:gsub("(l)(l)", function() return nil end)}
        r[#r + 1] = {(string.rep("ab", 5000)):gsub("b", "%0%0")}
        local m = "key=val, a=b, num 123 end"
        r[#r + 1] = {m:find("(%w+)=(%w+)")}
        r[#r + 1] = {m:match("(%w+)=(%w+)", 9)}
        r[#r + 1] = {m:find("()%d+()")}
        r[#r + 1] = {m:find("xyz"), m:find("=", 1, true), string.char(72, 105), ("ab"):rep(3, ","), #("x"):rep(100, "--")}
        r[#r + 1] = {string.char(table.unpack({65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81,82,83,84,85,86,
            87,88,89,90,65,66,67,68,69,70,71,72,73,74,75,76,77,78,79,80,81}))}
        for k, v in m:gmatch("(%w+)=(%w+)") do r[#r + 1] = k .. v end
        for w in m:gmatch("%a+") do r[#r + 1] = w end
        return r
        "##,
    );
}

/// Hosts compare script values with `Lua::raw_equal`: equal long strings made apart are
/// equal (while `Value`'s own `==` is identity for them), short strings, numbers across
/// subtypes and tables by identity as `rawequal` does.
#[test]
fn host_raw_equal_compares_strings_by_content() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let long = "x".repeat(100);
    let a = lua.create_string(&long).unwrap();
    let b = lua.create_string(&long).unwrap();
    let r = lua.exec("return string.rep('x', 100), 1, 1.0, {}, {}", "=t").unwrap();
    assert!(lua.raw_equal(&a, &b) && lua.raw_equal(&a, &r[0]) && lua.raw_equal(&r[1], &r[2]));
    assert!(!lua.raw_equal(&r[3], &r[4]) && lua.raw_equal(&r[3], &r[3]));
    let c = lua.create_string("x".repeat(99) + "y").unwrap();
    let (s1, s2) = (lua.create_string("short").unwrap(), lua.create_string("short").unwrap());
    assert!(!lua.raw_equal(&a, &c) && lua.raw_equal(&s1, &s2) && !lua.raw_equal(&s1, &a));
}

/// Library functions called from Lua: results placed for every wanted count (none, one,
/// several, all, more than 16), errors named and positioned as C Lua does, directly and
/// under `pcall`/`xpcall`, and a `__call` object reaching a library function.
#[test]
fn native_calls_place_results_and_errors() {
    same_as_c(
        r##"
        local s = "abcdefghijklmnopqrstuvwxyz"
        local r = {}
        local a, b, c = s:byte(1, 2)
        r[#r + 1] = {a, b, c}
        r[#r + 1] = {s:byte(1, 20)}
        r[#r + 1] = {select("#", s:byte(1, 20)), select("#", string.byte(s, 30))}
        local x = string.len(s)
        string.len(s)
        r[#r + 1] = {x, (s:byte(1, 3))}
        r[#r + 1] = {pcall(string.rep)}
        r[#r + 1] = {pcall(string.sub, s, "x")}
        r[#r + 1] = {pcall(function() return s:sub({}) end)}
        r[#r + 1] = {pcall(function() local t = setmetatable({}, {__index = function(t, k) return string.upper(k) end}) return t.abc, t[1] end)}
        r[#r + 1] = {xpcall(string.rep, function(m) return "handled: " .. m end)}
        r[#r + 1] = {pcall(error, "plain", 0)}
        r[#r + 1] = {pcall(function() error("lvl2", 2) end)}
        local callable = setmetatable({}, {__call = string.upper})
        r[#r + 1] = {pcall(callable, "x")}
        r[#r + 1] = {pcall(math.floor, "z")}
        return r
        "##,
    );
}

/// `type` returns names interned once (kept across collections): every kind, and the
/// results equal to strings made otherwise.
#[test]
fn type_names_are_shared_strings() {
    same_as_c(
        r#"
        local r = {}
        for _, v in ipairs({1, 1.5, "s", {}, print, function() end, coroutine.create(print), true}) do
            collectgarbage()
            r[#r + 1] = type(v)
        end
        r[#r + 1] = type(nil)
        r[#r + 1] = type(nil) == ("ni" .. "l")
        local t = {} t[type(t)] = 1
        r[#r + 1] = t.table
        return r
        "#,
    );
}

const ITERATION: &str = r#"
    local r = {}
    local t = {10, 20, 30, nil, 50, x = 1, y = 2, z = 3}
    local n = 0
    for k, v in pairs(t) do n = n + v end
    r[#r + 1] = n
    for i, v in ipairs(t) do r[#r + 1] = i * v end
    for k in next, {a = 1} do r[#r + 1] = k end
    r[#r + 1] = pairs({}) == next
    -- Clearing and changing fields while traversing (allowed by the manual).
    local u = {} for i = 1, 50 do u["k" .. i] = i end
    local sum, seen = 0, 0
    for k, v in pairs(u) do seen = seen + 1 if v % 2 == 0 then u[k] = nil else u[k] = v * 100 end sum = sum + v end
    local left = 0 for k, v in pairs(u) do left = left + v end
    r[#r + 1] = seen .. ":" .. sum .. ":" .. left
    -- Many dead slots to skip (charged as work).
    local big = {} for i = 1, 5000 do big["b" .. i] = i end
    for i = 1, 4990 do big["b" .. i] = nil end
    local m = 0 for k, v in pairs(big) do m = m + v end
    r[#r + 1] = m
    -- __pairs, ipairs through __index, ipairs on a proxy, one and three loop variables.
    local p = setmetatable({}, {__pairs = function(t) return function(_, k) if not k then return 1, "one" end end, t, nil end})
    for k, v in pairs(p) do r[#r + 1] = k .. v end
    local proxy = setmetatable({}, {__index = function(_, i) if i <= 3 then return i * 7 end end})
    for i, v in ipairs(proxy) do r[#r + 1] = v end
    for i in ipairs({5, 6}) do r[#r + 1] = i end
    for a, b, c in pairs({7}) do r[#r + 1] = tostring(c) end
    for i, v in ipairs({1.5, "s", true}) do r[#r + 1] = tostring(v) end
    r[#r + 1] = select(2, pcall(next, {}, "nokey"))
    r[#r + 1] = select(2, pcall(function() for k in pairs(nil) do end end))
    local s = string.rep("L", 60) local keyed = {[s] = 1}
    for k, v in next, keyed, nil do r[#r + 1] = #k end
    r[#r + 1] = next(keyed, string.rep("L", 60))
    return r
"#;

/// The generic `for` over `next`, `pairs` and `ipairs` (run in place by the fast loop):
/// results as C Lua, `__pairs` and `__index` honored, fields cleared and changed while
/// traversing, dead slots charged; and with the fuel cut at many points, the fast loop and
/// the full path stop at the same place with the same fuel and memory.
#[test]
fn generic_for_over_library_iterators() {
    same_as_c(ITERATION);
    let used = common::remedy_measured(ITERATION, 100_000_000, true).1;
    for cut in (1..60).map(|i| used * i / 60) {
        assert_eq!(
            common::remedy_measured(ITERATION, cut, true),
            common::remedy_measured(ITERATION, cut, false),
            "cut at {cut}"
        );
    }
}

/// Functions called back from the library and from metamethods use their first result
/// only (none: nil): comparators, gsub replacements, `__index`, `__lt`, `__le`, `__eq`,
/// `__concat`, `__len`, `__unm` and `__tostring` returning none, one or several values.
#[test]
fn callbacks_take_their_first_result() {
    same_as_c(
        r#"
        local r = {}
        local t = {5, 2, 8, 1}
        table.sort(t, function(a, b) return a < b, "extra" end)
        r[#r + 1] = table.concat(t, ",")
        r[#r + 1] = ("abc"):gsub("%w", function(c) return c:upper(), "ignored" end)
        r[#r + 1] = ("abc"):gsub("%w", function() end)
        local mt = {
            __index = function(_, k) return k .. "!", 2 end,
            __lt = function() return 1, false end, __le = function() end,
            __eq = function() return nil, true end, __concat = function() return "cc", "dd" end,
            __len = function() return 42, 43 end, __unm = function() end,
            __tostring = function() return "T", "U" end,
        }
        local a, b = setmetatable({}, mt), setmetatable({}, mt)
        r[#r + 1] = {a.x, a < b, a <= b, a == b, a .. "s", #a, -a == nil, tostring(a)}
        r[#r + 1] = {pcall(table.sort, {3, 2, 1, 4, 5}, function() error("cmp") end)}
        return r
        "#,
    );
}

const GSUB_IN_PLACE: &str = r#"
    local text = {}
    for i = 1, 300 do text[#text + 1] = "w" .. i .. " k" .. (i % 7) .. "=v" .. i end
    text = table.concat(text, "; ")
    local made = 0
    -- Callbacks that allocate and collect while gsub reads the subject in place.
    local s1, n1 = text:gsub("(%w+)=(%w+)", function(k, v)
        made = made + #string.rep(k, 50)
        if made % 7 == 0 then collectgarbage() end
        return v .. "=" .. k
    end)
    local s2, n2 = text:gsub("%d+", {["1"] = "one", ["22"] = false})
    local s3, n3 = text:gsub("(w)(%d)(%d?)", "%3%2%1%0", 50)
    local s4, n4 = text:gsub("[^;]-;", "")
    return #s1, n1, s2:sub(1, 80), n2, s3:sub(1, 80), n3, s4, n4, made
"#;

/// gsub reads its subject, pattern and replacement in place (no copies): callbacks that
/// allocate and collect, table and string replacements, long subjects whose matching is
/// charged in chunks; the fast loop and the full path agree with the fuel cut anywhere.
#[test]
fn gsub_reads_in_place() {
    same_as_c(GSUB_IN_PLACE);
    let used = common::remedy_measured(GSUB_IN_PLACE, 100_000_000, true).1;
    for cut in (1..40).map(|i| used * i / 40) {
        assert_eq!(
            common::remedy_measured(GSUB_IN_PLACE, cut, true),
            common::remedy_measured(GSUB_IN_PLACE, cut, false),
            "cut at {cut}"
        );
    }
}

const SHORT_CONCATS: &str = r#"
    local n, made = 0, {}
    for i = 1, 3000 do
        local q = '"' .. "user" .. '"'
        local k = "k" .. (i % 50) .. ":" .. 1.5
        local m = -7 .. "|" .. i % 3
        made[k] = (made[k] or 0) + #q + #m
        if i % 500 == 0 then collectgarbage() end
        n = n + #(q .. k .. m)
    end
    local c = 0 for _ in pairs(made) do c = c + 1 end
    return n, c, made["k7:1.5"], ("x"):rep(39) .. "y", ("x"):rep(40) .. "y"
"#;

/// `..` whose result is a short string is done in the fast loop (interned there): same
/// strings, fuel and memory as the full path, with the fuel cut anywhere, collections in
/// between (interned results kept from the sweep), and the memory running out.
#[test]
fn short_concat_in_fast_loop() {
    same_as_c(SHORT_CONCATS);
    let used = common::remedy_measured(SHORT_CONCATS, 100_000_000, true).1;
    for cut in (1..50).map(|i| used * i / 50) {
        assert_eq!(
            common::remedy_measured(SHORT_CONCATS, cut, true),
            common::remedy_measured(SHORT_CONCATS, cut, false),
            "cut at {cut}"
        );
    }
    let run = |fast: bool| {
        let mut lua = Lua::new(
            Limits::default().fuel(100_000_000).memory(3 << 20).hash_seed(common::TEST_SEED),
            Libs::ALL,
        );
        lua.set_fast_loop(fast);
        let r = lua.exec("local t = {} for i = 1, 1000000 do t[i] = 'k' .. i end return #t", "=t");
        (r.map(|v| v.len()), lua.fuel_left(), lua.memory_used())
    };
    let fast = run(true);
    assert!(matches!(fast.0, Err(Error::Limit(LimitKind::Memory))), "{fast:?}");
    assert_eq!(fast, run(false));
}

const DEEP_PATTERNS: &str = r#"
    local s = string.rep("a", 400)
    local deep = string.rep("a?", 198)
    local mixed = string.rep("a*", 60) .. string.rep("a-", 60) .. string.rep("(a?)", 30) .. string.rep("a?", 40)
    return {string.find(s, deep)}, {string.match(s, mixed .. "$")}, {pcall(string.find, s, string.rep("a?", 199))},
        {pcall(string.match, s, string.rep("(", 32) .. string.rep("a-", 160) .. "a" .. string.rep(")", 32))},
        {s:gsub(deep, "x")}, {s:gsub(string.rep("a-", 150) .. "a", "")},
        {pcall(string.gsub, s, string.rep("a*", 200) .. "b", "")}
"#;

/// The pattern matcher keeps its alternatives on the heap, not the native stack: patterns
/// nested to C Lua's limit (200 levels of `?`, `*`, `-` and captures) give C Lua's results
/// on a thread with a 64 KiB stack, and one level past it is "pattern too complex" there too.
#[test]
fn deep_patterns_on_a_small_stack() {
    let small = std::thread::Builder::new()
        .stack_size(64 * 1024)
        .spawn(|| common::remedy_measured(DEEP_PATTERNS, 100_000_000, true))
        .unwrap()
        .join()
        .unwrap();
    assert!(small.0.as_ref().is_ok_and(|s| !s.starts_with("error")), "{small:?}");
    assert_eq!(small, common::remedy_measured(DEEP_PATTERNS, 100_000_000, false));
    let c = common::oracle(DEEP_PATTERNS, 100_000_000);
    {
        assert_eq!(small.0, c);
    }
}

/// Random patterns (classes, sets, ranges, quantifiers, captures, anchors, `%b`, `%f`,
/// back references, malformed ones) against random subjects with find, match, gmatch and
/// gsub: the same results and errors as C Lua.
#[test]
fn random_patterns_match_c() {
    same_as_c(
        r#"
        local seed = 7
        local function rnd(n) seed = (seed * 1103515245 + 12345) % 2147483648 return seed % n + 1 end
        local atoms = {"a", "b", ".", "%a", "%d", "%s", "%w", "%A", "%D", "[ab]", "[^a]", "[a-c1]", "[%d_]", "%%",
            "%b()", "%f[%w]", "1", " ", "(", ")", "()", "%1", "^", "$", "[", "%", "[]", "%z"}
        local quants = {"", "", "", "*", "+", "-", "?"}
        local chars = {"a", "b", "c", "1", "2", " ", "(", ")", "_", "x"}
        local out = {}
        for i = 1, 400 do
            local p = {}
            for j = 1, rnd(6) do p[#p + 1] = atoms[rnd(#atoms)] .. quants[rnd(#quants)] end
            p = table.concat(p)
            local s = {}
            for j = 1, rnd(14) - 1 do s[#s + 1] = chars[rnd(#chars)] end
            s = table.concat(s)
            local r = {pcall(string.find, s, p)}
            local m = {pcall(string.match, s, p, rnd(3))}
            local g = {pcall(string.gsub, s, p, "<%0>")}
            local n = 0
            local ok = pcall(function() for a, b in s:gmatch(p) do n = n + #tostring(a) + #tostring(b) end end)
            out[#out + 1] = string.format("%q|%q", p, s) .. "|" .. table.concat({tostring(r[1]), tostring(r[2]), tostring(r[3]),
                tostring(m[1]), tostring(m[2]), tostring(m[3]), tostring(g[1]), tostring(g[2]), tostring(ok), n}, ",")
        end
        return table.concat(out, "\n")
        "#,
    );
}

/// The matcher's step counts (charged as fuel) are those of the recursive matcher it
/// replaced, first-item shortcut included: a fixed program, collector stopped, uses
/// exactly the fuel it used before (7412, measured on the recursive matcher; 29689 since
/// sweep 2 charges a unit per 4 steps instead of 64, SEC2-2; 31890 since sweep 3 charges
/// `gsub` a step per replacement-string byte, SEC3-11, and a `gmatch` step 2 units; 32093
/// since each `__index` link is 2 units, `vm::LINK_COST`: the 203 string-method calls here,
/// 40 times five and three more, each go through the string metatable's `__index` once;
/// 32279 since every object made pays for its traversal, the collector stopped or not,
/// COV-5; 32280 with that on an earlier base, whose calls make the same objects a unit apart).
#[test]
fn matcher_fuel_is_unchanged() {
    let prog = r#"
collectgarbage("stop")
local text = string.rep("word12 key=value; (nested (parens)) end\n", 200)
local n = 0
for i = 1, 40 do
  n = n + (text:find("%(", i * 37) or 0) + (text:find("[;=]", i * 11) or 0) + #(text:match("^%a+", i) or "")
  n = n + (text:find("key=(%w+)", i * 13) or 0) + (text:find("%b()", i * 5) or 0)
end
for k, v in text:gmatch("(%w+)=(%w+)") do n = n + #k + #v end
local s, c = text:gsub("[aeiou]", "%0%0")
local s2, c2 = text:gsub("%s+", function(w) return "" end)
return n + #s + c + #s2 + c2
"#;
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().fuel(1_000_000).hash_seed(common::TEST_SEED), Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec(prog, "=t").unwrap();
        assert_eq!(1_000_000 - lua.fuel_left(), 32280);
    }
}

/// With a fixed `hash_seed` a run replays exactly: the JSON benchmark and a program whose
/// output is its `pairs` order and random numbers give the same output and fuel five
/// times over, with the fast loop on and off.
#[test]
fn fixed_hash_seed_replays_exactly() {
    const ORDER: &str = r#"
        local t, out = {}, {}
        for i = 1, 200 do t["k" .. i] = i t[i * 1.5] = i t[{}] = i end
        for k, v in pairs(t) do out[#out + 1] = type(k) == "table" and "t" or tostring(k) end
        math.randomseed()
        for i = 1, 20 do out[#out + 1] = math.random(1000) end
        return table.concat(out, ",")
    "#;
    for prog in [include_str!("../benches/lua/json.lua"), ORDER] {
        let run = |fast: bool| {
            let mut lua =
                Lua::new(Limits::default().fuel(1_000_000_000).memory(1 << 30).hash_seed(42), Libs::ALL);
            lua.set_fast_loop(fast);
            let v = lua.exec(prog, "=t").unwrap();
            (v.first().and_then(|x| lua.to_string_lossy(*x)), lua.fuel_left(), lua.memory_used())
        };
        let first = run(true);
        for i in 0..5 {
            assert_eq!(run(i % 2 == 0), first, "run {i}");
        }
    }
}
