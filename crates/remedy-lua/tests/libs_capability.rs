//! The standard library's opt-in capabilities (`docs/reference.md`).
//!
//! - `Libs::DEBUG_INFO`: `debug.traceback` and a read-only `debug.getinfo`.
//! - `Libs::REQUIRE`: `require` and `package` with modules only from the host.
//!
//! Where behaviour is shared with C Lua 5.4.7 the program runs in both (RemedyLua with the
//! fast loop on and off: results, fuel and memory identical) and the outcomes must be equal.
//! The rest checks what RemedyLua does on its own: the flags are off unless a host opts in,
//! nothing that could break the sandbox is there, the host's modules, and the metering.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Error, Libs, LimitKind, Limits, Lua, ModuleSource};

use super::common;

const FUEL: u64 = 100_000_000;

/// Runs `program` here (every library, the opt-in ones included) and in C Lua: same outcome.
fn same_as_c(program: &str) {
    let ours = common::remedy_libs(program, FUEL, common::ALL_OPT_IN);
    let c = common::oracle(program, FUEL);
    assert_eq!(ours, c, "{program}");
}

/// Cuts a traceback at the test wrapper's `pcall` (`common::wrap`): what is below it is the
/// wrapper's own chunk, named differently in the two runs.
const CUT: &str = r##"
local function cut(s)
  if type(s) ~= "string" then return s end
  local i = 0
  while true do
    local j = s:find("\n\t[C]: in function 'pcall'", i + 1, true)
    if not j then break end
    i = j
  end
  if i > 0 then s = s:sub(1, i - 1) end
  return s
end
"##;

/// A state with the opt-in libraries.
fn state() -> Lua {
    Lua::new(Limits::default().fuel(FUEL).hash_seed(common::TEST_SEED), common::ALL_OPT_IN)
}

fn run(lua: &mut Lua, src: &str) -> Result<Vec<String>, String> {
    let v = lua.exec(src, "=t").map_err(|e| e.to_string())?;
    Ok(v.iter().map(|x| common::show(lua, *x)).collect())
}

// ---- debug.traceback against C Lua

#[test]
fn traceback_names_levels_as_c_lua() {
    same_as_c(&format!(
        r##"{CUT}
        local out = {{}}
        local function loc() return cut(debug.traceback("m")) end
        out[#out + 1] = loc()
        function GLOB() return cut(debug.traceback()) end
        out[#out + 1] = GLOB()
        local t = {{f = function() return cut(debug.traceback(nil, 1)) end}}
        out[#out + 1] = t.f()
        local o = {{}} function o:m() return cut(debug.traceback("x", 2)) end
        out[#out + 1] = o:m()
        out[#out + 1] = cut(debug.traceback("lvl0", 0))
        out[#out + 1] = cut(debug.traceback(12))
        out[#out + 1] = cut(debug.traceback(1.5, 1))
        out[#out + 1] = cut(debug.traceback("a\0b"))
        out[#out + 1] = type(debug.traceback({{}}))
        out[#out + 1] = debug.traceback(true)
        out[#out + 1] = cut(debug.traceback("neg", -1))
        out[#out + 1] = cut(debug.traceback("far", 100))
        out[#out + 1] = cut(debug.traceback(nil, 2 ^ 32 + 1))
        local up = function() return cut(debug.traceback()) end
        local function viaup() return (up()) end
        out[#out + 1] = viaup()
        out[#out + 1] = select(2, pcall(debug.traceback, "x", "y"))
        return out
        "##
    ));
}

#[test]
fn traceback_tail_calls_metamethods_and_iterators() {
    same_as_c(&format!(
        r##"{CUT}
        local out = {{}}
        local function tc() return cut(debug.traceback()) end
        local function caller() return tc() end
        out[#out + 1] = caller()
        local function tail2() return caller() end
        out[#out + 1] = tail2()
        local mt = {{}}
        mt.__index = function(t, k) out[#out + 1] = cut(debug.traceback(k)) return 1 end
        mt.__newindex = function(t, k, v) out[#out + 1] = cut(debug.traceback(k)) end
        mt.__add = function(a, b) out[#out + 1] = cut(debug.traceback("add")) return 0 end
        mt.__concat = function(a, b) out[#out + 1] = cut(debug.traceback("concat")) return "" end
        mt.__len = function(a) out[#out + 1] = cut(debug.traceback("len")) return 0 end
        mt.__unm = function(a) out[#out + 1] = cut(debug.traceback("unm")) return 0 end
        mt.__eq = function(a, b) out[#out + 1] = cut(debug.traceback("eq")) return true end
        mt.__lt = function(a, b) out[#out + 1] = cut(debug.traceback("lt")) return true end
        mt.__le = function(a, b) out[#out + 1] = cut(debug.traceback("le")) return true end
        mt.__call = function(self, x) out[#out + 1] = cut(debug.traceback("call")) return x end
        local a, b = setmetatable({{}}, mt), setmetatable({{}}, mt)
        local _ = a.x
        a.y = 1
        _ = a + 1
        _ = a .. "s"
        _ = #a
        _ = -a
        _ = a == b
        _ = a < b
        _ = a <= b
        _ = a(1)
        local function iter(s, i) if i < 1 then out[#out + 1] = cut(debug.traceback("iter")) return i + 1 end end
        for i in iter, nil, 0 do end
        return out
        "##
    ));
}

#[test]
fn traceback_skips_the_middle_of_a_deep_stack() {
    same_as_c(&format!(
        r##"{CUT}
        local out = {{}}
        local function rec(n) if n == 0 then return cut(debug.traceback("deep")) end local r = rec(n - 1) return r end
        for _, n in ipairs({{5, 18, 19, 20, 21, 22, 23, 30, 100}}) do out[#out + 1] = rec(n) end
        local function rec2(n) if n == 0 then return cut(debug.traceback("lvl", 5)) end local r = rec2(n - 1) return r end
        out[#out + 1] = rec2(40)
        return out
        "##
    ));
}

/// The message handler runs where the error was raised: the function that raised it is a
/// level of the traceback (`[C]: in function 'error'`), as in C Lua. (Regression: the
/// native was gone from the levels by the time the handler ran.)
#[test]
fn traceback_as_message_handler_sees_the_raising_function() {
    same_as_c(&format!(
        r##"{CUT}
        local out = {{}}
        out[#out + 1] = cut(select(2, xpcall(function() error("boom") end, debug.traceback)))
        out[#out + 1] = cut(select(2, xpcall(error, debug.traceback, "e")))
        out[#out + 1] = cut(select(2, xpcall(function() local x = nil + 1 end, debug.traceback)))
        out[#out + 1] = cut(select(2, xpcall(function() pcall(error, "z") local x = {{}} .. 1 end, debug.traceback)))
        out[#out + 1] = cut(select(2, xpcall(string.rep, debug.traceback)))
        out[#out + 1] = cut(select(2, xpcall(function() string.rep() end, debug.traceback)))
        out[#out + 1] = cut(select(2, xpcall(function() error({{}}) end, debug.traceback)) and "table")
        out[#out + 1] = cut(select(2, xpcall(function() error("lvl2", 2) end, function(m) return debug.traceback(m, 2) end)))
        return out
        "##
    ));
}

/// LIBS-2 on its own (the oracle test above compares the whole text): the raising
/// function is the handler's first level.
#[test]
fn message_handler_traceback_starts_at_the_raising_function() {
    let mut lua = state();
    let r = run(
        &mut lua,
        "return select(2, xpcall(function() error('boom') end, debug.traceback)), \
         select(2, xpcall(error, debug.traceback, 'e'))",
    )
    .unwrap();
    assert_eq!(
        r[0],
        "t:1: boom\nstack traceback:\n\t[C]: in function 'error'\n\tt:1: in function <t:1>\n\t[C]: in function 'xpcall'\n\tt:1: in main chunk"
    );
    assert_eq!(r[1], "e\nstack traceback:\n\t[C]: in function 'error'\n\t[C]: in function 'xpcall'\n\tt:1: in main chunk");
}

#[test]
fn traceback_of_coroutines() {
    same_as_c(&format!(
        r##"{CUT}
        local out = {{}}
        local co = coroutine.create(function(a) local function inner() coroutine.yield(1) end inner() return 2 end)
        out[#out + 1] = debug.traceback(co)
        coroutine.resume(co)
        out[#out + 1] = debug.traceback(co)
        out[#out + 1] = debug.traceback(co, "hi", 1)
        out[#out + 1] = debug.traceback(co, nil, 2)
        out[#out + 1] = debug.traceback(co, "neg", -1)
        coroutine.resume(co)
        out[#out + 1] = coroutine.status(co) .. " " .. debug.traceback(co, "done")
        local inner = coroutine.wrap(function() out[#out + 1] = debug.traceback("inside") out[#out + 1] = debug.traceback(coroutine.running(), "self") end)
        inner()
        local outer
        outer = coroutine.create(function()
          local c2 = coroutine.create(function() out[#out + 1] = debug.traceback(outer, "outer is normal") end)
          coroutine.resume(c2)
        end)
        coroutine.resume(outer)
        return out
        "##
    ));
}

// ---- debug.getinfo against C Lua

/// A function that shows a getinfo table with sorted keys (the serializer would do it too;
/// this keeps `nil` results visible).
const SHOW: &str = r##"
local function show(t)
  if t == nil then return "nil" end
  local k = {}
  for n in pairs(t) do k[#k + 1] = n end
  table.sort(k)
  local o = {}
  for _, n in ipairs(k) do o[#o + 1] = n .. "=" .. tostring(t[n]) end
  return table.concat(o, " ")
end
"##;

#[test]
fn getinfo_levels_and_functions_as_c_lua() {
    same_as_c(&format!(
        r##"{SHOW}
        local out = {{}}
        out[#out + 1] = show(debug.getinfo(0, "Slnrtu"))
        out[#out + 1] = show(debug.getinfo(1, "Slnrtu"))
        local function f(a, b, ...) return debug.getinfo(1, "Slnrtu") end
        out[#out + 1] = show(f())
        out[#out + 1] = show((f()))
        local function g() local r = debug.getinfo(2, "Sln") return r end
        local function h() local r = g() return r end
        out[#out + 1] = show(h())
        out[#out + 1] = show(debug.getinfo(print, "Slnrtu"))
        out[#out + 1] = show(debug.getinfo(f, "Slnrtu"))
        out[#out + 1] = show(debug.getinfo(function()
          return 1
        end, "S"))
        out[#out + 1] = show(debug.getinfo(load("return 1"), "S"))
        out[#out + 1] = show(load("return debug.getinfo(1, 'S')")())
        out[#out + 1] = show(load("return debug.getinfo(1, 'S')", "=named")())
        out[#out + 1] = show(load("return debug.getinfo(1, 'S')", "@file.lua")())
        out[#out + 1] = show(load("\n\nreturn debug.getinfo(1, 'Sl')", "multi\nline")())
        local parts = {{"return ", "debug.getinfo(1, 'S')"}}
        local i = 0
        out[#out + 1] = show(load(function() i = i + 1 return parts[i] end)())
        out[#out + 1] = show(debug.getinfo(100, "S")) .. show(debug.getinfo(-1, "S"))
        out[#out + 1] = show(debug.getinfo(100, "q"))
        out[#out + 1] = show(debug.getinfo(1, ""))
        out[#out + 1] = show(debug.getinfo(1, "S\0q"))
        local function tail() return debug.getinfo(1, "nt") end
        local function viatail() return tail() end
        out[#out + 1] = show(viatail())
        local o = {{}} function o:m() return debug.getinfo(1, "n") end
        out[#out + 1] = show(o:m())
        out[#out + 1] = show(o.m(o))
        function GF() return debug.getinfo(1, "n") end
        out[#out + 1] = show(GF())
        out[#out + 1] = show(setmetatable({{}}, {{__index = function() return debug.getinfo(1, "n") end}}).x)
        for _ in function(s, c) if c then return nil end out[#out + 1] = show(debug.getinfo(1, "n")) return 1 end do end
        return out
        "##
    ));
}

#[test]
fn getinfo_errors_and_threads_as_c_lua() {
    same_as_c(&format!(
        r##"{SHOW}
        local out = {{}}
        out[#out + 1] = select(2, pcall(function() return debug.getinfo("x") end))
        out[#out + 1] = select(2, pcall(function() return debug.getinfo({{}}) end))
        out[#out + 1] = select(2, pcall(function() return debug.getinfo() end))
        out[#out + 1] = select(2, pcall(function() return debug.getinfo(1, "q") end))
        out[#out + 1] = select(2, pcall(function() return debug.getinfo(1, ">S") end))
        out[#out + 1] = select(2, pcall(function() return debug.getinfo(1, {{}}) end))
        out[#out + 1] = select(2, pcall(debug.getinfo, 1.5))
        local co = coroutine.create(function(x) local y = coroutine.yield(x) return y end)
        out[#out + 1] = show(debug.getinfo(co, 0, "Sl"))
        coroutine.resume(co, 1)
        out[#out + 1] = show(debug.getinfo(co, 0, "Sln"))
        out[#out + 1] = show(debug.getinfo(co, 1, "Sln"))
        out[#out + 1] = show(debug.getinfo(co, 2, "Sln"))
        out[#out + 1] = show(debug.getinfo(co, print, "S"))
        out[#out + 1] = select(2, pcall(function() return debug.getinfo(co, 1, "q") end))
        coroutine.resume(co, 2)
        out[#out + 1] = show(debug.getinfo(co, 0, "S"))
        out[#out + 1] = show(debug.getinfo(coroutine.running(), 1, "Sl"))
        return out
        "##
    ));
}

// ---- require and package against C Lua

#[test]
fn require_with_preload_and_loaded_as_c_lua() {
    same_as_c(
        r##"
        local out = {}
        local calls = {}
        package.preload.p = function(...) calls[#calls + 1] = table.concat({...}, ",") return {p = true} end
        local m, d = require("p")
        out[#out + 1] = tostring(m.p) .. " " .. tostring(d) .. " " .. tostring(package.loaded.p == m)
        out[#out + 1] = select("#", require("p")) .. " " .. table.concat(calls, ";")
        package.preload.q = function() end
        out[#out + 1] = tostring(require("q")) .. " " .. tostring(package.loaded.q)
        package.preload.r = function() package.loaded.r = "set" end
        out[#out + 1] = table.concat({require("r")}, " ")
        package.preload.f = function() return false end
        out[#out + 1] = tostring(require("f")) .. " " .. tostring(package.loaded.f)
        package.loaded.f = nil
        out[#out + 1] = tostring(require("f"))
        package.preload["12"] = function(n) return n end
        out[#out + 1] = require(12)
        package.preload.e = function() error("loader fails") end
        out[#out + 1] = select(2, pcall(require, "e"))
        out[#out + 1] = tostring(require("string") == string) .. tostring(package.loaded._G == _G)
            .. tostring(package.loaded.table == table) .. tostring(package.loaded.coroutine == coroutine)
            .. tostring(package.loaded.math == math) .. tostring(package.loaded.utf8 == utf8)
            .. tostring(package.loaded.debug == debug) .. tostring(package.loaded.package == package)
        out[#out + 1] = type(package.loaded) .. type(package.preload) .. type(package.searchers) .. type(require)
        setmetatable(package.loaded, {__index = function(t, k) return "from __index " .. k end})
        out[#out + 1] = require("virtual")
        setmetatable(package.loaded, nil)
        local seen
        package.loaded.w = nil
        package.preload.w = function() return "w" end
        setmetatable(package.loaded, {__newindex = function(t, k, v) seen = k rawset(t, k, v .. "!") end})
        out[#out + 1] = require("w") .. " " .. tostring(seen)
        setmetatable(package.loaded, nil)
        return out
        "##,
    );
}

#[test]
fn require_messages_and_searchers_as_c_lua() {
    same_as_c(
        r##"
        local out = {}
        local first = package.searchers[1]
        package.searchers = {first}
        out[#out + 1] = select(2, pcall(require, "nope"))
        out[#out + 1] = select(2, pcall(function() require("nope") end))
        out[#out + 1] = select(2, pcall(function() return require("a\0b") end))
        package.searchers = {first, function(n) return "\n\tcustom " .. n end, function() return nil end, function() return 42 end}
        out[#out + 1] = select(2, pcall(require, "zz"))
        package.searchers = {function(n) return function(a, b) return a .. "|" .. tostring(b) end, "data" end}
        out[#out + 1] = table.concat({require("custom")}, " ")
        package.searchers = {function(n) return function(a, b) return select("#", a, b) end end}
        out[#out + 1] = table.concat({tostring(require("nodata"))}, " ")
        package.searchers = {}
        out[#out + 1] = select(2, pcall(require, "none"))
        package.searchers = 1
        out[#out + 1] = select(2, pcall(require, "x"))
        package.searchers = {first}
        out[#out + 1] = select(2, pcall(require))
        out[#out + 1] = select(2, pcall(require, {}))
        package.preload.y = function() error({code = 7}) end
        local ok, e = pcall(require, "y")
        out[#out + 1] = tostring(ok) .. " " .. type(e) .. " " .. tostring(e.code)
        return out
        "##,
    );
}

/// A cycle recurses (C Lua has no cycle detection either) until C Lua's limit of C levels:
/// "C stack overflow", without a position, as C Lua raises it inside `require`. (LIBS-5:
/// it was "t:2: stack overflow (too many nested calls from library functions)".) Deep
/// recursion through other library callbacks says the same.
#[test]
fn require_cycle_stops_with_c_luas_message() {
    let program = r##"
        package.preload.c1 = function() return require("c2") end
        package.preload.c2 = function() return require("c1") end
        local ok, e = pcall(require, "c1")
        local function r() return string.gsub("a", "a", r) end
        local ok2, e2 = pcall(r)
        return ok, e, ok2, e2
    "##;
    let ours = common::remedy_libs(program, FUEL, common::ALL_OPT_IN);
    assert_eq!(ours, Ok("false \"C stack overflow\" false \"C stack overflow\"".into()), "ours");
    same_as_c(program);
}

/// A coroutine that died by an error keeps its stack for `debug.traceback` and
/// `debug.getinfo`, the function that raised the error included, until it is closed: as in
/// C Lua. (LIBS-4: it had no levels.)
#[test]
fn dead_coroutine_keeps_its_stack_for_tracebacks() {
    let program = format!(
        r##"{SHOW}
        local out = {{}}
        local co = coroutine.create(function() local function f() error("oops") end f() end)
        local ok, e = coroutine.resume(co)
        out[#out + 1] = debug.traceback(co, e)
        out[#out + 1] = show(debug.getinfo(co, 0, "Sln"))
        out[#out + 1] = show(debug.getinfo(co, 1, "Sln"))
        out[#out + 1] = show(debug.getinfo(co, 2, "Sln"))
        out[#out + 1] = show(debug.getinfo(co, 3, "Sln"))
        local c2 = coroutine.create(function(x) local y = x + 1 return y end)
        out[#out + 1] = debug.traceback(c2, select(2, coroutine.resume(c2)))
        local c3 = coroutine.create(function() local t = setmetatable({{}}, {{__index = function() error({{}}) end}}) return t.x end)
        coroutine.resume(c3)
        out[#out + 1] = debug.traceback(c3, "table error")
        out[#out + 1] = tostring(coroutine.close(co)) .. " " .. debug.traceback(co, "closed")
        local c4 = coroutine.create(function() return 1 end)
        coroutine.resume(c4)
        out[#out + 1] = debug.traceback(c4, "finished")
        return out
        "##
    );
    let ours = common::remedy_libs(&program, FUEL, common::ALL_OPT_IN).unwrap();
    // (Serialized: `\9` is a tab, a backslash and a newline a newline.)
    assert!(ours.contains("\\9[C]: in function 'error'\\\n\\9t:12: in local 'f'"), "{ours}");
    assert!(ours.contains("name=error namewhat=global short_src=[C]"), "{ours}");
    same_as_c(&program);
}

/// `nups` of library functions is C Lua's: 3 for `gmatch`'s iterator, 1 for a wrap
/// function, `require` and the searchers, none for the rest. (LIBS-6: `pairs` and `ipairs`
/// counted RemedyLua's own upvalue.)
#[test]
fn library_function_upvalues_as_c_lua() {
    let program = r##"
        local out = {}
        for _, f in ipairs({print, pairs, ipairs, next, string.gmatch("a", "a"), coroutine.wrap(print), require,
            package.searchers[1], utf8.codes("a"), string.format, table.sort}) do
          out[#out + 1] = debug.getinfo(f, "u").nups
        end
        return table.concat(out, " ")
    "##;
    assert_eq!(common::remedy_libs(program, FUEL, common::ALL_OPT_IN), Ok("\"0 0 0 0 3 1 1 1 0 0 0\"".into()));
    same_as_c(program);
}

// ---- off unless the host opts in; nothing that breaks the sandbox

#[test]
fn opt_in_flags_are_off_in_every_preset() {
    for libs in [Libs::SAFE, Libs::STANDARD, Libs::ALL] {
        assert!(!libs.contains(Libs::DEBUG_INFO) && !libs.contains(Libs::REQUIRE));
        let mut lua = Lua::new(Limits::default(), libs);
        let r = run(&mut lua, "return type(debug), type(require), type(package)").unwrap();
        assert_eq!(r, ["nil", "nil", "nil"], "{libs:?}");
    }
    let mut lua = Lua::new(Limits::default(), Libs::SAFE);
    lua.open_libs(Libs::DEBUG_INFO);
    assert_eq!(run(&mut lua, "return type(debug.traceback), type(require)").unwrap(), ["function", "nil"]);
    lua.open_libs(Libs::REQUIRE);
    assert_eq!(run(&mut lua, "return type(require), package.loaded.debug == debug").unwrap(), ["function", "true"]);
    // Each flag alone.
    let mut lua = Lua::new(Limits::default(), Libs::SAFE | Libs::REQUIRE);
    assert_eq!(run(&mut lua, "return type(debug), type(require)").unwrap(), ["nil", "function"]);
}

#[test]
fn nothing_that_breaks_the_sandbox_is_there() {
    let mut lua = state();
    let r = run(
        &mut lua,
        r##"
        local keys = {}
        for k in pairs(debug) do keys[#keys + 1] = k end
        table.sort(keys)
        local pk = {}
        for k in pairs(package) do pk[#pk + 1] = k end
        table.sort(pk)
        return table.concat(keys, " "), table.concat(pk, " "), #package.searchers,
          tostring(io), tostring(dofile), tostring(loadfile), tostring(os.getenv), tostring(os.execute)
        "##,
    )
    .unwrap();
    assert_eq!(r, ["getinfo traceback", "loaded preload searchers", "2", "nil", "nil", "nil", "nil", "nil"]);
    // `f` and `L` would hand out functions: refused, by name.
    for opt in ["f", "L", "Sf", "flnSrtu"] {
        let e = run(&mut lua, &format!("return debug.getinfo(1, '{opt}')")).unwrap_err();
        assert!(e.contains("is not available"), "{opt}: {e}");
    }
    // The default options are C Lua's without `f`: no value in the result is a function,
    // at any level, of any thread.
    let r = run(
        &mut lua,
        r##"
        local bad = 0
        local function scan(t) if t then for k, v in pairs(t) do if type(v) == "function" or type(v) == "thread" then bad = bad + 1 end end end end
        local co = coroutine.create(function() coroutine.yield() end) coroutine.resume(co)
        local function deep(n) if n == 0 then for i = 0, 10 do scan(debug.getinfo(i)) scan(debug.getinfo(co, i)) end return end deep(n - 1) end
        deep(5)
        scan(debug.getinfo(print)) scan(debug.getinfo(deep))
        local t = debug.getinfo(1)
        return bad, t.func == nil, t.activelines == nil, t.source, t.what
        "##,
    )
    .unwrap();
    assert_eq!(r, ["0", "true", "true", "=t", "main"]);
}

// ---- the host's modules

#[test]
fn host_modules_load_through_require() {
    let mut lua = state();
    lua.add_module("greet", "local M = {} function M.hi(n) return 'hi ' .. n end function M.where() return debug.traceback('w') end return M");
    lua.add_module_source("cfg", ModuleSource::new("return {name = ..., origin = select(2, ...)}", "mods/cfg.lua"));
    lua.add_module("broken", "return = 1");
    lua.add_module("loops", "while true do end");
    lua.set_module_loader(Some(|n: &str| {
        n.strip_prefix("gen.").map(|rest| ModuleSource::new(format!("return '{rest}'"), format!("gen/{rest}.lua")))
    }));
    let r = run(
        &mut lua,
        r##"
        local g, gd = require("greet")
        local c, cd = require("cfg")
        return g.hi("you"), gd, c.name, c.origin, cd, require("gen.x"), select(2, require("gen.y")),
          g.where(), select(2, pcall(require, "broken")), select(2, pcall(require, "absent")),
          package.loaded.greet == g
        "##,
    )
    .unwrap();
    assert_eq!(r[0], "hi you");
    assert_eq!(r[1], "greet");
    assert_eq!((r[2].as_str(), r[3].as_str(), r[4].as_str()), ("cfg", "mods/cfg.lua", "mods/cfg.lua"));
    assert_eq!((r[5].as_str(), r[6].as_str()), ("x", "gen/y.lua"));
    assert_eq!(r[7], "w\nstack traceback:\n\tgreet:1: in function 'greet.where'\n\tt:5: in main chunk");
    assert_eq!(r[8], "error loading module 'broken' from host module 'broken':\n\tbroken:1: unexpected symbol near '='");
    assert_eq!(r[9], "module 'absent' not found:\n\tno field package.preload['absent']\n\tno host module 'absent'");
    assert_eq!(r[10], "true");
    // A module's code runs on the script's budget: an endless one stops at the fuel limit,
    // which no `pcall` catches.
    lua.set_limits(lua.limits().fuel(200_000));
    let e = lua.exec("return pcall(require, 'loops')", "=t").unwrap_err();
    assert_eq!(e, Error::Limit(LimitKind::Fuel));
    // No loader: an unknown name is not found; `None` removes the loader.
    lua.set_module_loader(None::<fn(&str) -> Option<ModuleSource>>);
    lua.set_limits(lua.limits().fuel(1_000_000));
    assert!(run(&mut lua, "return select(2, pcall(require, 'gen.z'))").unwrap()[0].contains("no host module 'gen.z'"));
}

/// Compiling a host module is charged as `load` charges the same source.
#[test]
fn host_module_compile_is_charged_like_load() {
    let src: String = (0..2000).map(|i| format!("local a{i} = {i}\n")).collect::<String>() + "return 1";
    let fuel_of = |code: &str| {
        let mut lua = state();
        lua.add_module("big", &src);
        let s = lua.create_string(&src).unwrap();
        lua.set_global("SRC", s).unwrap();
        let f = lua.load(code, "=t").unwrap();
        lua.call(f.value(), &[]).unwrap();
        FUEL - lua.fuel_left()
    };
    let by_require = fuel_of("return require('big')");
    let by_load = fuel_of("return load(SRC, '@big')()");
    // (`load` also reads its argument; `require` its lookups and searchers: within 1%.)
    let diff = by_require.abs_diff(by_load);
    assert!(diff * 100 < by_load, "require {by_require} against load {by_load}");
}

// ---- metering

/// Fuel is identical with the fast loop on and off, and does not depend on the hash seed,
/// in the shapes LIBS-1 was found in. (Regression: the loaded-modules table, traversed by
/// the collector before the globals, reached the libraries in its own hash order, a cycle's
/// split into steps followed it, and fuel differed by 1 to 13 units between seeds. The
/// property can still fail in rare shapes, as it could before: LIBS-8.)
#[test]
fn fuel_is_independent_of_the_seed_and_the_fast_loop() {
    let fuel = |src: &str, seed: u64, fast: bool| {
        let mut lua = Lua::new(Limits::default().fuel(FUEL).hash_seed(seed), common::ALL_OPT_IN);
        lua.set_fast_loop(fast);
        lua.exec(src, "=t").unwrap();
        FUEL - lua.fuel_left()
    };
    let tail = "local function f() return ('x'):rep(3) end local s = 0 for i = 1, 20 do s = s + #f() + #tostring(i) end return s";
    let with_libs = "local function tb() return debug.traceback('x') end local s = 0 \
                     for i = 1, 20 do s = s + #tb() + #debug.traceback() + debug.getinfo(1, 'Sl').currentline end \
                     package.preload.z = function() return tb() end return s + #require('z')";
    let mut varied = Vec::new();
    for n in (1..=150).step_by(3) {
        // Tables the script keeps in its globals, and modules only `package.loaded` holds.
        for store in ["T = {} for i = 1, N do T['m' .. i] = {a = i, b = {}} end", "for i = 1, N do package.loaded['m' .. i] = {a = i, b = {}} end"] {
            for body in [tail, with_libs] {
                let src = format!("for i = 1, 3000 do _G['g' .. i] = i end {} {body}", store.replace('N', &n.to_string()));
                let mut fuels = std::collections::BTreeSet::new();
                for seed in [1u64, 2, 3, 4] {
                    fuels.insert(fuel(&src, seed, true));
                }
                fuels.insert(fuel(&src, 1, false));
                if fuels.len() > 1 {
                    varied.push(format!("n = {n}, {store}: {fuels:?}"));
                }
            }
        }
    }
    assert!(varied.is_empty(), "fuel varies with the seed or the fast loop:\n{}", varied.join("\n"));
}

/// A traceback's name search reads every loaded module: charged by the slots it reads,
/// churned (mostly empty) tables included, so the work stays bounded by fuel.
#[test]
fn traceback_name_search_is_charged_by_slots_read() {
    let used = |setup: &str| {
        let mut lua = state();
        lua.exec(setup, "=s").unwrap();
        lua.exec("local s = debug.traceback('m')", "=t").unwrap();
        FUEL - lua.fuel_left()
    };
    let small = used("");
    let wide = used("for i = 1, 65536 do package.loaded['c' .. i] = i end for i = 1, 65535 do package.loaded['c' .. i] = nil end");
    assert!(wide > small + 60_000, "a churned 64k-slot loaded table cost {wide} against {small}");
    let nested = used("local t = {} for i = 1, 65536 do t['k' .. i] = i end package.loaded.big = t");
    assert!(nested > small + 60_000, "a 64k-entry loaded module cost {nested} against {small}");
}

/// Deep stacks: each call walks them, and is charged for it.
#[test]
fn deep_stacks_are_charged() {
    let used = |depth: usize, body: &str| {
        let mut lua = state();
        let src = format!(
            "local function D(n) if n == 0 then {body} return end D(n - 1) end \
             local before = 0 D({depth})"
        );
        let f = lua.load(src, "=t").unwrap();
        lua.call(f.value(), &[]).unwrap();
        FUEL - lua.fuel_left()
    };
    for body in ["for i = 1, 100 do local t = debug.getinfo(1, 'l') end", "for i = 1, 100 do local s = debug.traceback() end"] {
        let shallow = used(10, body);
        let deep = used(1500, body);
        // 100 walks of 1,500 more levels: at least a unit per two levels.
        assert!(deep > shallow + 100 * 1490 / 2, "{body}: {deep} against {shallow}");
    }
}
