//! Speed, native calls: the library functions the fast loop runs in place,
//! checked against C Lua with the fast loop on and off (same results, fuel and memory:
//! `common::remedy`), at fuel limits that run out inside them, and under several hash
//! seeds.

use remedy_lua::{Libs, Limits, Lua};

use crate::common::{TEST_SEED, oracle, remedy};

fn same_as_c(program: &str) {
    let ours = remedy(program, 50_000_000);
    let theirs = oracle(program, 50_000_000);
    assert_eq!(ours, theirs, "differs from C Lua:\n{program}");
}

/// The fuel `src` uses with the fast loop on and off, under hash seeds 1 to 8: all equal.
fn same_fuel_every_seed(src: &str) {
    let mut fuel = Vec::new();
    for seed in 1..=8u64 {
        for fast in [true, false] {
            let mut lua = Lua::new(Limits::default().hash_seed(seed), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(src, "=t").unwrap();
            fuel.push(lua.fuel_left());
        }
    }
    assert!(fuel.iter().all(|f| *f == fuel[0]), "fuel differs between seeds or paths: {fuel:?}");
}

/// Calls made through the general native call when `src` runs with the fast loop on (on
/// even under `REMEDYLUA_TEST_NO_FAST_LOOP`: the in-place calls are on with it).
fn native_calls(src: &str) -> u64 {
    let mut lua = Lua::new(Limits::default().hash_seed(TEST_SEED), Libs::ALL);
    lua.set_fast_loop(true);
    lua.exec(src, "=t").unwrap();
    lua.native_calls()
}

/// `s:byte(i)`, `s:sub(i, j)` of a string interned already, plain `s:find`,
/// and `find`/`match` with a short pattern run in the fast loop: a thousand iterations
/// of each make (almost) no general native calls.
#[test]
fn string_natives_run_in_place() {
    let src = r#"
        local s = "hello, world: 123 and more"
        local n = 0
        for i = 1, 1000 do
          n = n + s:byte(3) + #s:sub(1, 5) + s:find("world", 1, true)
          n = n + s:find("[%d]") + #s:match("%d+") + (s:find("o", 6) or 0)
        end
        return n
    "#;
    let calls = native_calls(src);
    assert!(calls < 10, "{calls} general native calls (the in-place path did not run)");
    same_as_c(src);
}

/// Every argument shape of `len` and the in-place `byte` and `sub`, the cases they run and
/// the ones they leave to the full path (errors, conversions, several bytes, strings not
/// interned yet), with every number of results wanted: as C Lua.
#[test]
fn len_byte_sub_every_shape() {
    same_as_c(
        r#"
        local s = "abcdefghij"
        local log = {}
        local function add(...) log[#log + 1] = select('#', ...) .. ':' .. table.concat(table.pack(...), ',', 1, select('#', ...)) end
        local pos = { -12, -11, -10, -1, 0, 1, 2, 9, 10, 11, 100, math.mininteger, math.maxinteger }
        for _, i in ipairs(pos) do
          add(s:byte(i))
          add((s:byte(i)))
          local a, b = s:byte(i); add(a, b)
          add({ s:byte(i) } [1])
          for _, j in ipairs(pos) do
            add(s:sub(i, j))
            add(s:byte(i, j))
          end
          add(s:sub(i))
          add(s:byte(i, nil))
        end
        add(s:byte())
        add(s:len(), ("").len(""), string.len(s), #s:sub(3, 2))
        add(s:sub(2.0, 3.0), s:byte(2.0), s:sub("2", "4"), s:byte("3"))
        add(string.byte(12345, 2), string.sub(12345, 2, 3), string.len(12.5))
        -- New strings (not interned yet), long ones, the empty string.
        for i = 1, 30 do add(s:rep(5):sub(i, i + 7)) end
        add(s:rep(10):sub(1, 60), s:sub(5, 4), ("x"):sub(2))
        for _, bad in ipairs({ "s:sub()", "s:sub(nil, 2)", "s:byte({})", "s:sub(1.5)", "string.len()", "string.byte(nil)", "s:sub(1, {})", "string.sub({}, 1)" }) do
          add(pcall(load("local s = ... return " .. bad), s))
        end
        return table.concat(log, ' | ')
    "#,
    );
}

/// `find` and `match` in place: plain and patterned, anchored, from every start
/// position, with position and string captures (one, two, more), no match, the empty
/// pattern, malformed patterns, and the plain flag in every form: as C Lua.
#[test]
fn find_and_match_every_shape() {
    same_as_c(
        r#"
        local s = "key = value; n=42, x = -3.5e2 [tag]"
        local log = {}
        local function add(...) log[#log + 1] = select('#', ...) .. ':' .. table.concat(table.pack(...), ',', 1, select('#', ...)) end
        local pats = { "=", "value", "%d+", "^key", "^value", "(%a+)%s*=", "()(%d)()", "(%a+)=(%d+)", "(%a)(%a)(%a)",
          "[%[%]]", "x", "", "%s*$", "[^ ]+", "-?%d+%.?%d*[eE]?[-+]?%d*", "(", "%", "[a", "(()", "%g" , ".-;", "a*", "q" }
        for _, p in ipairs(pats) do
          for _, i in ipairs({ -100, -5, -1, 0, 1, 2, 10, 35, 36, 37, 38, 100 }) do
            add(pcall(string.find, s, p, i))
            add(pcall(string.match, s, p, i))
            add(pcall(string.find, s, p, i, true))
            add(pcall(string.find, s, p, i, 1))
            add(pcall(string.find, s, p, i, false))
          end
          add(pcall(string.find, s, p))
          add(pcall(string.match, s, p))
          local ok, a = pcall(function() return (s:find(p)) end); add(ok, a)
          local ok2, b, c, d = pcall(function() local b, c, d = s:match(p); return b, c, d end); add(ok2, b, c, d)
        end
        add(pcall(string.find, s, 5)); add(pcall(string.find, 12345, 3)); add(pcall(string.find, s))
        add(pcall(string.find, s, "a", 1.0)); add(pcall(string.find, s, "a", "2")); add(pcall(string.find, s, "a", 1.5))
        return table.concat(log, ' | ')
    "#,
    );
}

/// A rebound or wrapped library function is not run in place (the check is identity),
/// and the in-place functions keep working after `string` is changed: as C Lua.
#[test]
fn rebound_string_functions_are_called_as_usual() {
    same_as_c(
        r#"
        local s, log = "abcdef", {}
        local sub, byte = string.sub, string.byte
        for i = 1, 4 do
          log[#log + 1] = tostring(s:sub(2, 3)) .. tostring(s:byte(i))
          if i == 1 then string.sub = function(x, a, b) return "<" .. sub(x, a, b) .. ">" end end
          if i == 2 then string.byte = function() return -1 end end
          if i == 3 then string.sub, string.byte = sub, byte end
        end
        local mt = getmetatable("")
        mt.__index = { sub = function() return "own" end, byte = byte }
        log[#log + 1] = s:sub(1, 2) .. s:byte(1)
        mt.__index = string
        log[#log + 1] = s:sub(1, 2) .. s:byte(1)
        return table.concat(log, ' ')
    "#,
    );
}

/// The fuel limit running out at every step of a loop of in-place calls (inside a
/// long plain search, a long pattern match, a run of interner walks): the fast loop and
/// the full path stop at the same step with the same fuel and memory.
#[test]
fn fuel_runs_out_inside_in_place_calls() {
    let src = r#"
        local s = string.rep("ab", 300) .. "needle" .. string.rep("cd", 300)
        local words = {}
        for i = 1, 40 do words[i] = "w" .. i end
        local line = table.concat(words, " ")
        local n = 0
        for i = 1, 30 do
          n = n + s:find("needle", 1, true) + (s:find("%d") or 0) + #s:match("n%a+")
          n = n + #line:sub(i * 3, i * 3 + 2) + line:byte(i) + #(line:match("w%d+", i) or "")
        end
        return n
    "#;
    same_as_c(src);
    for fuel in (1..2500u64).step_by(2) {
        let _ = remedy(src, fuel);
    }
}

/// The interner's walk steps an in-place `sub` or `match` charges, and its search work,
/// do not depend on the hash seed (nor on the path).
#[test]
fn in_place_fuel_is_the_same_under_every_seed() {
    same_fuel_every_seed(
        r#"
        local parts = {}
        for i = 1, 500 do parts[i] = "k" .. i end
        local s = table.concat(parts, ",")
        local n, at = 0, 1
        while true do
          local i, j = s:find(",", at, true)
          if not i then break end
          local k = s:sub(at, i - 1)
          n = n + #k + (k:match("%d+") and 1 or 0) + k:byte(1)
          at = j + 1
        end
        return n
    "#,
    );
}

/// A string limit below the short-string length: an in-place `sub` or `match` whose
/// result is past it fails as the full path fails.
#[test]
fn string_limit_below_short_strings() {
    let src = r#"local s = string.rep("x", 30) local t = s .. "" return pcall(function() return #s:sub(1, 20) end), pcall(function() return #s:match("x+") end)"#;
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().string_len(10).hash_seed(TEST_SEED), Libs::ALL);
        lua.set_fast_loop(fast);
        let r = lua.exec(src, "=t");
        assert!(
            r.is_err(),
            "fast loop {fast}: a 20-byte result under a 10-byte limit: {:?}",
            r.map(|v| v.len())
        );
    }
}

/// In-place calls while the collector runs (strings found dead but not yet swept are
/// kept, as `intern` keeps them): the same results, fuel and memory on both paths.
#[test]
fn in_place_calls_during_collection() {
    let src = r#"
        local s = "alpha beta gamma delta epsilon"
        local keep, n = {}, 0
        for i = 1, 3000 do
          local junk = { i, tostring(i), s .. i }
          local w = s:sub(1 + (i % 5) * 6, 5 + (i % 5) * 6)
          keep[i % 64] = w
          n = n + #w + (s:find(w, 1, true) or 0) + #(s:match("%a+", i % 30 + 1) or "")
          if i % 500 == 0 then collectgarbage("step", 0) end
        end
        return n
    "#;
    same_as_c(src);
    let _ = remedy(src, 2_000_000);
}

/// `upper`, `lower`, `rep` and `char` run in place when their result
/// is interned already: a thousand iterations make (almost) no general native calls.
#[test]
fn building_string_natives_run_in_place() {
    let src = r#"
        local s = "Hello, World"
        local n = 0
        for i = 1, 1000 do
          n = n + #s:upper() + #s:lower() + #s:rep(2) + #s:rep(3, "-") + #("ab"):rep(i % 4)
          n = n + #string.char(65 + i % 26, 66)
        end
        return n
    "#;
    let calls = native_calls(src);
    assert!(calls < 100, "{calls} general native calls (the in-place path did not run)");
    same_as_c(src);
}

/// Every argument shape of the in-place `upper`, `lower`, `rep` and `char`, and of `reverse`:
/// empty strings, results at and past the short-string length, counts and separators of
/// every kind, bytes out of range, conversions and errors, and strings not interned yet
/// (the full path makes them): as C Lua, with the same fuel on both paths.
#[test]
fn upper_lower_reverse_rep_char_every_shape() {
    let src = r#"
        local log = {}
        local function add(...) log[#log + 1] = select('#', ...) .. ':' .. table.concat(table.pack(...), ',', 1, select('#', ...)) end
        for _, s in ipairs({ "", "a", "aB3 ;x", string.rep("Zy", 20), string.rep("q", 41), "\0\255\128" }) do
          for r = 1, 2 do
            add(s:upper(), s:lower(), s:reverse(), #s:upper())
            for _, n in ipairs({ -1, 0, 1, 2, 3, 40, 41, math.mininteger }) do
              add(pcall(string.rep, s, n))
              add(pcall(string.rep, s, n, ""))
              add(pcall(string.rep, s, n, ", "))
            end
          end
        end
        add(pcall(string.rep, "x", 2.0), pcall(string.rep, "x", "3"), pcall(string.rep, "x", 2, 5), pcall(string.rep, "x", 1.5))
        add(pcall(string.rep, "x", math.maxinteger), pcall(string.rep, "", math.maxinteger), pcall(string.rep, "", math.maxinteger, "y"))
        add(pcall(string.rep, "x"), pcall(string.rep), pcall(string.upper), pcall(string.lower, {}), pcall(string.reverse, 12))
        add(string.upper(12.5), string.lower(10), string.reverse(123))
        for r = 1, 2 do
          add(string.char(), string.char(72), string.char(72, 105), string.char(0, 255, 1), string.char(65, 66, 67, 68))
          add(pcall(string.char, 256), pcall(string.char, -1), pcall(string.char, 65.0), pcall(string.char, "66"), pcall(string.char, 1.5), pcall(string.char, {}))
        end
        return table.concat(log, ' | ')
    "#;
    same_as_c(src);
    for fuel in (1..3000u64).step_by(7) {
        let _ = remedy(src, fuel);
    }
}

/// `tostring` of numbers, booleans and nil and `type` run in place,
/// and so do the string functions whose result is a new string (made there, as
/// `new_str` makes it): a thousand iterations make (almost) no general native calls.
#[test]
fn tostring_type_and_new_strings_run_in_place() {
    let src = r#"
        local s, n = "the quick brown fox", 0
        for i = 1, 1000 do
          n = n + #tostring(i) + #tostring(i / 4) + #tostring(i % 2 == 0) + #tostring(nil)
          n = n + #type(s) + #type(i)
          n = n + #(s:sub(i % 10, i % 10 + 5 + i % 7) .. i) + #(s .. i):upper()
        end
        return n
    "#;
    let calls = native_calls(src);
    assert!(calls < 100, "{calls} general native calls (the in-place path did not run)");
    same_as_c(src);
}

/// `tostring`, `type` and `math.type` on every kind of value, and the cases they leave to
/// the full path (tables, functions, strings with `__tostring` on the string metatable,
/// missing arguments): as C Lua.
#[test]
fn tostring_type_math_type_every_shape() {
    same_as_c(
        r#"
        local log = {}
        local function add(...) log[#log + 1] = select('#', ...) .. ':' .. table.concat(table.pack(...), ',', 1, select('#', ...)) end
        local vals = { 0, -1, 7, math.maxinteger, math.mininteger, 0.5, -0.0, 1e300, 1/0, -1/0, 2^53, 3.0, true, false, "s", "" }
        for _, v in ipairs(vals) do
          add(tostring(v), type(v), math.type(v))
          local a, b = tostring(v); add(a, b)
        end
        add(tostring(nil), type(nil), math.type(nil), type({}), type(print), type(coroutine.create(print)), math.type({}))
        add(pcall(tostring), pcall(type), pcall(math.type))
        add(tostring(setmetatable({}, { __tostring = function() return "T" end })))
        add(tostring(setmetatable({}, { __name = "N" })):match("^N: "))
        getmetatable("").__tostring = function(s) return "S:" .. #s end
        add(tostring("abc"), tostring(5))
        getmetatable("").__tostring = nil
        add(tostring("abc"))
        local n = 0
        for i = 1, 2000 do n = n + #tostring(i * 7919) + #tostring(i + 0.25) end
        add(n)
        return table.concat(log, ' | ')
    "#,
    );
}

/// New strings made in place under a tight memory limit (the full path collects and
/// tries again when memory is refused), in collector stress mode (a collection at every
/// allocation: the collector's step after each call), and with the fuel running out on
/// the interner's walk right after a string is made: the same results, fuel and memory
/// on both paths.
#[test]
fn new_strings_in_place_under_limits() {
    let src = r#"
        local keep, n = {}, 0
        for i = 1, 3000 do
          local s = tostring(i)
          keep[i % 50] = s
          n = n + #s + #("x" .. i):upper() + #s:rep(3)
        end
        return n
    "#;
    same_as_c(src);
    for fuel in (1..6000u64).step_by(5) {
        let _ = remedy(src, fuel);
    }
    for (memory, stress) in [(64 << 10, false), (200 << 10, false), (256 << 20, true)] {
        let run = |fast: bool| {
            let mut lua = Lua::new(Limits::default().memory(memory).hash_seed(TEST_SEED), Libs::ALL);
            lua.set_fast_loop(fast);
            lua.set_gc_stress(stress);
            let r = lua
                .exec(src, "=t")
                .map(|v| v.first().and_then(|x| lua.to_string_lossy(*x)))
                .map_err(|e| e.to_string());
            (r, lua.fuel_left(), lua.memory_used())
        };
        assert_eq!(run(true), run(false), "memory {memory}, stress {stress}");
    }
    same_fuel_every_seed(src);
}

/// With more than 2^18 strings interned, each new one costs the interner's walk steps
/// (`regrow_share`), charged right after it is made: slices of fuel then end on those
/// steps, after an in-place `tostring` made its string, and the next slice is handed out
/// there (or the fuel limit stops the call there) as on the full path.
#[test]
fn fuel_slices_end_on_the_walk_after_a_new_string() {
    let src = r#"
        local keep = {}
        for i = 1, 270000 do keep[i] = tostring(i) end
        local n = 0
        for i = 270001, 330000 do n = n + #tostring(i) end
        return n + #keep
    "#;
    let full = remedy_lua_fuel(src, u64::MAX / 4);
    for fuel in [full.1 - 1, full.1 - 2, full.1 - 3, full.1 - 1000, full.1 - 77_777] {
        let fast = remedy_lua_fuel(src, fuel);
        assert!(fast.0.is_err(), "fuel {fuel} of {}: {:?}", full.1, fast.0);
    }
}

/// The result and fuel used of `src` with `fuel`, the same on both paths (asserted).
fn remedy_lua_fuel(src: &str, fuel: u64) -> (Result<String, String>, u64) {
    let (r, used, mem) = crate::common::remedy_measured(src, fuel, true);
    let slow = crate::common::remedy_measured(src, fuel, false);
    assert_eq!((r.clone(), used, mem), slow, "fast loop and full path differ at fuel {fuel}");
    (r, used)
}

/// `table.insert` and `table.remove` share the in-place call (`fast::lib_call`) with the
/// library functions: on a plain array they still run in the fast loop (a thousand of
/// each make a dozen general native calls, the inserts that grow the array: some two
/// thousand without it), with C Lua's results, and the same fuel and memory on both paths
/// at every third fuel limit.
#[test]
fn table_insert_remove_share_the_in_place_call() {
    let src = r#"
        local t, s = {}, 0
        for i = 1, 1000 do table.insert(t, i * 2) end
        for i = 1, 1000 do s = s + table.remove(t) + #("x"):rep(i % 3) end
        table.insert(t, "a") table.insert(t, "b")
        local r = { table.remove(t), table.remove(t), table.remove(t), #t }
        return s, r[1], r[2], r[3], r[4]
    "#;
    let calls = native_calls(src);
    assert!(calls < 50, "{calls} general native calls (the in-place path did not run)");
    same_as_c(src);
    for fuel in (1..3000u64).step_by(3) {
        let _ = remedy(src, fuel);
    }
}
