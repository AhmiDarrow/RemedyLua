//! Lua 5.4 semantics: the last differences from C Lua 5.4.7. Every program
//! runs against C Lua and, through `common::remedy`, with the fast loop on and off (results,
//! fuel and memory identical).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common::{oracle, remedy};

fn same_as_c(program: &str) {
    let ours = remedy(program, 2_000_000_000);
    assert_eq!(ours, oracle(program, 2_000_000_000), "{program}");
    assert!(ours.is_ok(), "{ours:?}");
}

/// Resumes `f` until it ends, collecting what each resume gave (as text).
const DRIVE: &str = r##"
    local function pack(...) return {n = select("#", ...), ...} end
    local function show(r) local t = {} for i = 1, r.n do t[i] = (tostring(r[i]):gsub("0x%x+", "")) end return table.concat(t, ",") end
    local function drive(f, ...)
        local co = coroutine.create(f)
        local out, args = {}, pack(...)
        for _ = 1, 50 do
            out[#out + 1] = show(pack(coroutine.resume(co, table.unpack(args, 1, args.n))))
            if coroutine.status(co) == "dead" then break end
            args = pack("resumed", #out)
        end
        return table.concat(out, " | ")
    end
"##;

/// S5COR-6c (item 1): a `pcall` used as a metamethod whose callee is a protected call
/// (`pcall + t` with `__add = pcall`), `pcall(pcall, pairs, t)`, `pcall(pairs, t)` with
/// `pcall` as a metamethod, and `pairs` itself as a metamethod, with a Lua `__pairs` or
/// callee that yields: RemedyLua ran them as nested calls, so the yield was "attempt to
/// yield across a C-call boundary"; C Lua allows it. Errors after the yield, handlers and
/// `__close` too.
#[test]
fn parity_1_yields_inside_protected_calls_used_as_metamethods() {
    same_as_c(&format!(
        r##"{DRIVE}
        local r = {{}}
        local function yields(...) local a, b = coroutine.yield(...) return a, b, "done" end
        local callee = setmetatable({{}}, {{__call = function(self, x) return yields("call", x) end}})
        local fails = setmetatable({{}}, {{__call = function() coroutine.yield("before error") error("boom", 0) end}})
        local P = setmetatable({{}}, {{__add = pcall, __lt = pcall, __le = pcall, __concat = pcall, __eq = pcall, __index = pcall, __newindex = pcall, __unm = pcall, __len = pcall}})
        r[#r + 1] = drive(function() return pcall + P end)
        r[#r + 1] = drive(function() return callee + P end)
        r[#r + 1] = drive(function() return (pcall + P), "after" end)
        r[#r + 1] = drive(function() if pcall < P then return "lt" end return "not lt" end)
        r[#r + 1] = drive(function() if callee <= P then return "le" end return "not le" end)
        r[#r + 1] = drive(function() return "x" .. tostring(pcall .. P) end)
        r[#r + 1] = drive(function() return callee[P] end)
        r[#r + 1] = drive(function() return fails + P end)
        r[#r + 1] = drive(function() return pcall + setmetatable({{}}, {{__add = pcall, __call = function() return pcall + P end}}) end)
        -- `pcall(pcall, pairs, t)` and friends with a yielding `__pairs`.
        local yp = setmetatable({{}}, {{__pairs = function(t) local v = coroutine.yield("in pairs") return next, t, v, "dropped" end}})
        local ep = setmetatable({{}}, {{__pairs = function(t) coroutine.yield("in pairs") error("pairs failed", 0) end}})
        r[#r + 1] = drive(function() return pcall(pcall, pairs, yp) end)
        r[#r + 1] = drive(function() return pcall(xpcall, pairs, print, yp) end)
        r[#r + 1] = drive(function() return pcall(pcall, pcall, pairs, ep) end)
        r[#r + 1] = drive(function() return xpcall(pcall, print, pairs, ep) end)
        r[#r + 1] = drive(function() return pcall(xpcall, pairs, function(m) return "handled " .. m end, ep) end)
        r[#r + 1] = drive(function() return select("#", pcall(pcall, pairs, yp)) end)
        -- `pcall(pairs, t)` as a metamethod, and `pairs` as one.
        local both = setmetatable({{}}, {{__add = pcall, __pairs = function(t) coroutine.yield("pairs mm") return next, t, nil end}})
        r[#r + 1] = drive(function() return pairs + both end)
        r[#r + 1] = drive(function() local q = setmetatable({{}}, {{__index = pairs}}) local f = q[yp] return type(f) end)
        r[#r + 1] = drive(function() local q = setmetatable({{}}, {{__index = pairs}}) return pcall(function() return q[ep] end) end)
        -- A `__close` that yields inside such a call, on success and on error.
        local function closing(tag) return setmetatable({{}}, {{__close = function(_, e) coroutine.yield("close " .. tag .. " " .. tostring(e)) end}}) end
        local cl = setmetatable({{}}, {{__call = function() local c <close> = closing("ok") return "body" end}})
        local ce = setmetatable({{}}, {{__call = function() local c <close> = closing("err") error("e", 0) end}})
        r[#r + 1] = drive(function() return cl + P end)
        r[#r + 1] = drive(function() return ce + P end)
        return table.concat(r, "\n")
        "##
    ));
}

/// S5COR-6c (item 2): C Lua counts `pairs`'s call of a Lua `__pairs` (`lua_callk`) and the
/// metamethod call around a `pcall` used as one (`luaT_callTMres`) as C levels; RemedyLua
/// did not, so a message handler that kept failing ran once more before "C stack
/// overflow" under `pcall(pairs, t)` (and every shape containing it) or `pcall` as a
/// metamethod. Error levels under `pcall(pairs, t)` were one short too (`pairs` is an
/// activation). Checked over generated call shapes (`lua/parity/c_levels_shapes.lua`:
/// tail and plain calls, `pcall`, `xpcall`, nested protected calls, yields before and
/// after, `__index`, `__close`, `__call`, `__pairs`, `wrap` and `resume`, metamethods).
#[test]
fn parity_2_c_levels_and_error_levels_match_c_lua() {
    let shapes = include_str!("lua/parity/c_levels_shapes.lua");
    for seed in [7, 31] {
        same_as_c(&shapes.replace("SEED", &seed.to_string()).replace("ITERS", "260"));
    }
    same_as_c(
        r##"
        local r = {}
        for n = 1, 5 do
            local p = setmetatable({}, {__pairs = function() error("x", n) end})
            r[#r + 1] = {n, pcall(pairs, p)}
            r[#r + 1] = {n, pcall(pcall, pairs, p)}
            r[#r + 1] = {n, pcall(function() return pairs(p) end)}
            local o = setmetatable({}, {__call = function() error("y", n) end})
            local t = setmetatable({}, {__add = pcall})
            r[#r + 1] = {n, pcall(function() return select(2, o + t) end)}
            local q = setmetatable({}, {__index = pairs})
            r[#r + 1] = {n, pcall(function() return q[p] end)}
        end
        return r
        "##,
    );
}

/// S5COR-8 (pre-existing): an error raised by the continuation a protected call's outcome
/// goes to, while an error unwinds to that call (`pcall` as `__concat`, and the
/// concatenation then fails on its `false`), left every enclosing `pcall`, coroutine and
/// `xpcall` handler: it reached the host as an uncaught error. It unwinds again now, from
/// the frame below.
#[test]
fn parity_8_errors_from_a_protected_metamethods_continuation_are_caught() {
    same_as_c(&format!(
        r##"{DRIVE}
        local t = setmetatable({{}}, {{__concat = pcall, __lt = pcall, __add = pcall}})
        local function d(inner) return function()
            local o = setmetatable({{}}, {{__call = function() return inner() end}})
            return "x" .. (o .. t)
        end end
        local f = d(d(function() return "r" end))
        local r = {{}}
        r[#r + 1] = {{pcall(f)}}
        r[#r + 1] = {{coroutine.resume(coroutine.create(f))}}
        r[#r + 1] = {{xpcall(f, function(m) return "handled: " .. m end)}}
        r[#r + 1] = {{pcall(d(d(d(function() coroutine.yield() return 1 end))))}}
        r[#r + 1] = drive(d(d(d(function() coroutine.yield("deep") return 1 end))))
        r[#r + 1] = drive(function() return pcall(coroutine.wrap(d(d(function() coroutine.yield("w") end)))) end)
        -- A continuation that errors after a coroutine died under `wrap`.
        local w = coroutine.wrap(function() error("died", 0) end)
        r[#r + 1] = {{pcall(function() return "x" .. (w .. t) end)}}
        r[#r + 1] = {{pcall(function() return 1 + (w + t) end)}}
        return r
        "##
    ));
}

/// Item 4: the default call depth was 2,000 frames, so recursion C Lua runs (about a
/// million frames for the smallest functions) failed with "stack overflow". Frames are
/// memory now, like the value stack: deep recursion runs, and a call whose frames would
/// fill the memory budget is C Lua's catchable "stack overflow" (the frames given back
/// afterwards). On the 64 KiB stack, with both loops: Lua-to-Lua calls, error unwinding
/// from the bottom, a message handler there, `__close` all the way up, a coroutine parked
/// deep and collected, and collections in the middle.
#[test]
fn parity_4_deep_recursion_runs_as_in_c_lua() {
    let deep = r##"
        local n = 0
        local function f(k) if k == 0 then return 0 end n = n + 1 return 1 + f(k - 1) end
        local r = {f(300000), n}
        local function g(k) if k == 0 then collectgarbage() error({depth = n}) end local x <close> = setmetatable({}, {__close = function() n = n - 1 end}) return g(k - 1) + 1 end
        n = 200000
        local ok, e = pcall(g, 200000)
        r[#r + 1] = tostring(ok) .. " " .. e.depth .. " " .. n
        r[#r + 1] = select(2, xpcall(function() return f(250000) + nil end, function(m) return "handled " .. m end))
        local co = coroutine.create(function() local function h(k) if k == 0 then coroutine.yield("bottom") return 0 end return 1 + h(k - 1) end return h(150000) end)
        r[#r + 1] = select(2, coroutine.resume(co))
        r[#r + 1] = select(2, coroutine.resume(co))
        co = coroutine.create(function() local function h(k) if k == 0 then coroutine.yield() end return 1 + h(k - 1) end return h(150000) end)
        coroutine.resume(co)
        co = nil
        collectgarbage()
        -- Unbounded recursion: the ordinary error, caught, and the state goes on.
        local function inf(k) return 1 + inf(k + 1) end
        local ok2, e2 = pcall(inf, 1)
        r[#r + 1] = tostring(ok2) .. " " .. tostring(e2)
        r[#r + 1] = f(1000)
        return r
    "##;
    let theirs = oracle(deep, 4_000_000_000);
    assert!(theirs.is_ok(), "{theirs:?}");
    let ours = super::common::small_stack(move || {
        let mut out = Vec::new();
        for fast in [true, false] {
            out.push(super::common::remedy_measured(deep, 4_000_000_000, fast));
        }
        out
    });
    assert_eq!(ours[0], ours[1], "fast loop and full path differ");
    assert_eq!(ours[0].0, theirs);
    // Under the default budget (64 MiB), unbounded recursion is the catchable error too,
    // and the memory comes back.
    let r = super::common::small_stack(|| {
        let mut out = Vec::new();
        for fast in [true, false] {
            let mut lua =
                remedy_lua::Lua::new(remedy_lua::Limits::default().fuel(1 << 32), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            let v = lua
                .exec(
                    "local n = 0 local function inf() n = n + 1 return 1 + inf() end \
                     local ok, e = pcall(inf) local deep = n > 100000 n = 0 local ok2 = pcall(inf) \
                     collectgarbage() return ok, e, ok2, deep, collectgarbage('count') < 1024",
                    "=t",
                )
                .unwrap();
            out.push(v.iter().map(|x| super::common::show(&lua, *x)).collect::<Vec<_>>().join(" "));
        }
        out
    });
    assert_eq!(r[0], "false t:1: stack overflow false true true");
    assert_eq!(r[0], r[1]);
}

/// Item 3 (decided: keep the superset): protected calls nested deeper than C Lua's 200 C
/// levels run here, where C Lua stops them with "C stack overflow" (an implementation
/// limit, which `docs/reference.md` says RemedyLua does not have). Up to that depth the
/// handler counts are C Lua's (`parity_2`, `s5cor_5`). Past it, a message handler was never
/// called ("error in error handling" at once, from 220 levels); it now runs as at C Lua's
/// last level: once with the error, then with "C stack overflow", then "error in error
/// handling". Fixed expectations (C Lua refuses these programs), both loops, and the
/// shallow cases against C Lua.
#[test]
fn parity_3_protected_calls_past_c_luas_levels() {
    let program = |depths: &str| {
        format!(
            r##"
            local function nest(n, f) local fs = {{}} for i = 1, n do fs[i] = pcall end fs[n + 1] = f return pcall(table.unpack(fs)) end
            local out = {{}}
            for _, n in ipairs({{{depths}}}) do
                local seen = 0
                local r = table.pack(nest(n, function() return xpcall(error, function(m) seen = seen + 1 if seen < 3 then error("again " .. seen, 0) end return "handled " .. tostring(m) end, "e") end))
                local runaway = 0
                local q = table.pack(nest(n, function() return xpcall(error, function(m) runaway = runaway + 1 error(m, 0) end, "e") end))
                out[#out + 1] = n .. ": " .. tostring(r[r.n - 1]) .. " " .. tostring(r[r.n]) .. " " .. seen .. " / " .. tostring(q[q.n]) .. " " .. runaway
            end
            return table.concat(out, "; ")
            "##
        )
    };
    same_as_c(&program("0, 10, 150"));
    let deep = program("250, 1000, 20000");
    assert_eq!(
        remedy(&deep, 2_000_000_000).unwrap(),
        "\"250: false handled again 2 3 / error in error handling 20; \
         1000: false handled again 2 3 / error in error handling 20; \
         20000: false handled again 2 3 / error in error handling 20\""
    );
}

/// Item 5: the per-function register cap is gone. One call, return, `local` list,
/// parameter list, `...` adjustment, generic `for` or `local` list without values could
/// span at most 32,767 registers ("too many registers in one instruction"), counts at most
/// 65,534 ("too many values"), and a jump out of a block closed upvalues from at most
/// register 65,534 (from there, past it: a `break` with captured locals above that closed
/// too many). Now calls, tail calls, returns, `...` and the iterator call need only their
/// first registers in a window (the machine reads their spans from the stack), counts past
/// 16 bits go through `SetTop`/`PadNil`, nil runs are split, `MoveAbs` reaches any
/// register, parameters past a window are set, and far jumps close through a `Close`.
/// C Lua refuses all of these (200 locals, 255 registers): fixed expectations, both loops,
/// and the same shapes small enough for C Lua against it.
#[test]
fn parity_5_functions_past_any_register_count() {
    let program = |sizes: &str| {
        format!(
            r##"
            local function gen(n, f) local t = {{}} for i = 1, n do t[i] = f(i) end return table.concat(t, ", ") end
            local out = {{}}
            local function try(name, src)
                local f, e = load(src)
                if not f then out[#out + 1] = name .. ": compile " .. e return end
                local ok, r = pcall(f)
                out[#out + 1] = name .. ": " .. tostring(ok) .. " " .. tostring(r)
            end
            local function names(n, p) return gen(n, function(i) return p .. i end) end
            local function nums(n) return gen(n, function(i) return i end) end
            for _, n in ipairs({{{sizes}}}) do
                try("args" .. n, "return select('#', " .. nums(n) .. ")")
                try("ret" .. n, "local function f() return " .. nums(n) .. " end local t = table.pack(f()) return t.n .. ' ' .. t[t.n]")
                try("tailret" .. n, "local function g(...) return select('#', ...) end local function f() return g(" .. nums(n) .. ") end return f()")
                try("method" .. n, "local o = {{m = function(self, ...) return select('#', ...) + #self.x end, x = 'ab'}} return o:m(" .. nums(n) .. ")")
                try("locals" .. n, "local " .. names(n, "v") .. " v1 = 1 return tostring(v1) .. tostring(v" .. n .. ")")
                try("localvals" .. n, "local " .. names(n, "v") .. " = " .. nums(n) .. " return v1 + v" .. n)
                try("params" .. n, "local function f(" .. names(n, "p") .. ") return tostring(p1) .. tostring(p" .. n .. ") end return f(1) .. f(" .. nums(n) .. ") .. f()")
                try("varargs" .. n, "local function f(...) local " .. names(n, "v") .. " = ... return tostring(v" .. n .. ") .. tostring(v" .. (n - 1) .. ") end return f(" .. nums(n - 1) .. ")")
                try("callres" .. n, "local function g() return 1, 2, 3, 4, 5 end local " .. names(n, "v") .. " = g() return tostring(v5) .. tostring(v6) .. tostring(v" .. n .. ")")
                try("tfor" .. n, "local s = 0 for " .. names(n, "k") .. " in function(_, c) if c then return nil end return 1, 2, 3 end do s = s + k1 + k3 + (k" .. n .. " or 10) end return s")
                try("brk" .. n, "local fs = {{}} local " .. names(n, "v") .. " local i = 0 while true do i = i + 1 local x = i fs[i] = function() x = x + 100 return x end if i == 3 then break end end return fs[1]() + fs[1]() + fs[2]() + fs[3]()")
                try("goto" .. n, "local fs = {{}} local " .. names(n, "v") .. " local i = 1 ::top:: do local x = i fs[i] = function() x = x * 2 return x end i = i + 1 if i <= 3 then goto top end end return fs[1]() + fs[1]() + fs[2]() * 10 + fs[3]() * 100")
                try("tbc" .. n, "local log = {{}} local " .. names(n, "v") .. " do local c <close> = setmetatable({{}}, {{__close = function() log[#log + 1] = 'closed' end}}) log[#log + 1] = 'body' end return table.concat(log, ',')")
                try("pack" .. n, "return #{{" .. nums(n) .. ", (function() return 1, 2, 3 end)()}}")
                try("upv" .. n, "local " .. names(n, "v") .. " = " .. nums(n) .. " return (function() return v" .. n .. " + v1 end)()")
                try("brkcap" .. n, "local " .. names(n, "v") .. " = " .. nums(n) .. " local get = function() return v" .. n .. " end while true do local x = 1 local h = function() return x end break end v" .. n .. " = 5 return get()")
            end
            return table.concat(out, "; ")
            "##
        )
    };
    same_as_c(&program("3, 100"));
    let mut want = Vec::new();
    for n in [40000, 70000] {
        want.extend([
            format!("args{n}: true {n}"),
            format!("ret{n}: true {n} {n}"),
            format!("tailret{n}: true {n}"),
            format!("method{n}: true {}", n + 2),
            format!("locals{n}: true 1nil"),
            format!("localvals{n}: true {}", n + 1),
            format!("params{n}: true 1nil1{n}nilnil"),
            format!("varargs{n}: true nil{}", n - 1),
            format!("callres{n}: true 5nilnil"),
            format!("tfor{n}: true 14"),
            format!("brk{n}: true 507"),
            format!("goto{n}: true 646"),
            format!("tbc{n}: true body,closed"),
            format!("pack{n}: true {}", n + 3),
            format!("upv{n}: true {}", n + 1),
            format!("brkcap{n}: true 5"),
        ]);
    }
    assert_eq!(remedy(&program("40000, 70000"), 4_000_000_000).unwrap(), format!("\"{}\"", want.join("; ")));
}

/// S5COR-9 (found by fuzzing; pre-existing): two places where an
/// assignment's targets were evaluated in another order than C Lua's.
/// - `t.name = v` where `t` is an upvalue and `name` a short string constant is C Lua's
///   `SETTABUP`: the upvalue is read when the value is stored, after the values are
///   evaluated. RemedyLua read it into a register first, so a value that reassigned the
///   upvalue (a call) stored into the old table.
/// - C Lua's `check_conflict` copies a local (or that upvalue) an earlier target's table or
///   key is when the later target naming it is read, before the targets after it; RemedyLua
///   copied once every target had been read, after their keys ran.
/// Errors name the upvalue as before. Both compilers (`front-oracle` checks they agree).
#[test]
fn parity_9_assignment_targets_evaluate_in_c_luas_order() {
    same_as_c(
        r##"
        local g = {}
        local function f() g = {} return 7 end
        local function show(t) local ks = {} for k, v in pairs(t) do ks[#ks + 1] = tostring(k) .. ":" .. (type(v) == "table" and "tab" or tostring(v)) end table.sort(ks) return "{" .. table.concat(ks, ",") .. "}" end
        local long = "a long constant string key over forty characters ............"
        local tests = {
            function() g.n = f() end,
            function() g.m, g.k = f(), 9 end,
            function() g.x, g.y = 1, f() end,
            function() g[1] = f() end,
            function() g[1], g[2] = 1, f() end,
            function() local k = "z" g[k] = f() end,
            function() local k = "z" g[k], g.w = f(), 2 end,
            function() g.a = f() + 1 end,
            function() g.lst = {f()} end,
            function() g["a long constant string key over forty characters ............"] = f() end,
            function() g.p, g[1], g.q = f(), f(), f() end,
            function() g.x, g = 1, {} end,
            function() g, g.x = {}, 1 end,
            function() g.x, g, g.y = 1, {}, 2 end,
            function() local x = {} g.x, g, x[f()] = "a", "b", "c" end,
        }
        local out = {}
        for i, t in ipairs(tests) do g = {} local old = g t() out[#out + 1] = i .. ": " .. show(type(g) == "table" and g or {}) .. " / " .. show(old) end
        -- Locals: the copy is made when the target naming the local is read.
        local t = {} local old = t local x = {}
        local function h() t = {"new"} return 1 end
        t[1], t, x[h()] = "a", "b", "c"
        out[#out + 1] = tostring(old[1]) .. " " .. tostring(t)
        local a, i = {}, 1
        local function j() i = 2 return 3 end
        a[i], i, a[j()] = "x", "y", "z"
        out[#out + 1] = show(a) .. " " .. tostring(i)
        -- `_ENV` as the upvalue.
        local function e() local r = {} local env = setmetatable({}, {__index = _G}) local function k() _ENV = env return 1 end x1, _ENV = k(), env r[1] = rawget(env, "x1") return r end
        out[#out + 1] = tostring(e()[1]) .. " " .. tostring(rawget(_G, "x1"))
        return table.concat(out, "\n")
        "##,
    );
}

/// S5COR-10 (found by fuzzing; pre-existing): `string.format`'s `%s`
/// with modifiers checks the string for zeros and the specification (`checkformat`) before
/// the shortcut that keeps a string of 100 bytes or more whole; RemedyLua took the shortcut
/// first, so `%028s` (or `%+s`, or a string with a zero) of a long string was accepted.
#[test]
fn parity_10_format_s_checks_long_strings_too() {
    same_as_c(
        r##"
        local long, z = string.rep("x", 120), string.rep("y", 50) .. "\0" .. string.rep("y", 60)
        local r = {}
        for _, f in ipairs({"%028s", "%-130s", "%5s", "%.3s", "%s", "%+s", "% s", "%#s", "%0s", "%123s", "%1.2s", "%99s"}) do
            for _, s in ipairs({long, "short", z}) do
                local ok, v = pcall(string.format, f, s)
                r[#r + 1] = f .. ":" .. tostring(ok) .. ":" .. (ok and #v or v)
            end
        end
        return table.concat(r, "\n")
        "##,
    );
}

/// S5COR-11 (found by fuzzing; pre-existing): an error raised as a
/// metamethod's Lua function returns (the continuation of a `..` after `__concat`, a
/// comparison after `__lt`, ...) named the line of the metamethod's `return`, not the
/// caller's: the interpreter saved the returning function's position into its caller's
/// frame once the frame was gone. The same for a tail call that fails after its frame left.
#[test]
fn parity_11_errors_after_a_metamethod_returns_name_the_callers_line() {
    same_as_c(
        r##"
        local O3 = setmetatable({}, {__concat = function(a, b) return 1 end, __lt = function() return true end})
        local Y = setmetatable({}, {__concat = function(a, b) return (coroutine.yield("y")) end})
        local r = {}
        r[#r + 1] = select(2, pcall(function() local x = true return x ..
            O3 .. {} end))
        r[#r + 1] = select(2, pcall(function() local x = true return (x ..
            (O3 ..
            {})) end))
        r[#r + 1] = select(2, pcall(function() local x = "s" return x ..
            O3 .. {} .. "z" end))
        local co = coroutine.wrap(function() local x = true return (x ..
            (Y .. {})) end)
        co()
        r[#r + 1] = select(2, pcall(co, 1))
        r[#r + 1] = select(2, pcall(function()
            local t = setmetatable({}, {__index = function(t, k) return k end})
            local v = t.x
            return v ..
                {}
        end))
        return table.concat(r, " | ")
        "##,
    );
}

/// S5COR-12 (found by fuzzing; pre-existing): a library function the
/// call site gives no name to is named as C Lua's `pushglobalfuncname` finds it in the
/// loaded libraries (`string.rep`), and `?` when no library table holds it: `ipairs`'s and
/// `utf8.codes`'s iterators were named `ipairs_aux` and `for iterator`.
#[test]
fn parity_12_anonymous_library_functions_are_question_marks() {
    same_as_c(
        r##"
        local r = {}
        r[#r + 1] = select(2, pcall(function() return ipairs({})() end))
        r[#r + 1] = select(2, pcall(function() return utf8.codes("x")() end))
        r[#r + 1] = select(2, pcall(function() for p in utf8.codes("ab")() do end end))
        r[#r + 1] = select(2, pcall(function() return (pairs({}))() end))
        r[#r + 1] = select(2, pcall(function() return (load("return string.rep"))()() end))
        r[#r + 1] = select(2, pcall(function() for _ in ipairs({}) do end return ipairs({})({}, nil) end))
        r[#r + 1] = select(2, pcall(function() local f = utf8.codes("x") return f() end))
        return table.concat(r, " | ")
        "##,
    );
}

/// S5COR-13 (found by fuzzing; pre-existing): compile-time constants
/// fold as C Lua's code generator folds them (`constfolding`, `codenot`). A `<const>` local
/// whose value is an operator on constants (`local b <const> = a * 255` with `a`
/// constant, `-a`, `a & 3`, `not nil`, `(5)`) is a constant with no register, so a message
/// names no local; a `<const>` local used as a key is the constant key (`t[b]` with `b`
/// 5 is `t[5]`, "field 'integer index'"); and literals fold by C Lua's rules: no division
/// by zero (`1/0` stays an operation), bitwise operations on floats with integer values,
/// `//` and `%` on floats, `~` on a float, `not` of any constant, no NaN or zero results.
/// Both parsers and both compilers (`front-oracle` checks they agree). S5COR-16 (the
/// confirmation fuzz): as C Lua's `localstat`, the last name of a `local` statement with as
/// many values as names is such a constant too, not only a lone name.
#[test]
fn parity_13_constants_fold_as_in_c_lua() {
    same_as_c(include_str!("lua/parity/const_folding.lua"));
}

/// S5COR-14 (found by the suites after S5COR-9; pre-existing for globals): an upvalue's
/// field written by `SETTABUP` (every global assignment, and since S5COR-9 every
/// `upvalue.name = v`) lost the fuel its table write charged (a rehash, probe steps): the
/// interpreter's local fuel count was not synced around it. The same writes through a
/// register cost the same now, and the fast loop and full path agree.
#[test]
fn parity_14_upvalue_field_writes_are_charged() {
    let names: Vec<String> = (1..=3000).map(|i| format!("k{i}")).collect();
    let sets = |prefix: &str| names.iter().map(|n| format!("{prefix}{n} = 1")).collect::<Vec<_>>().join(" ");
    let via_upvalue = format!("local t = {{}} local function f() {} end f() return 1", sets("t."));
    let via_register = format!("local function f() local t = {{}} {} end f() return 1", sets("t."));
    let via_globals = format!("local function f() {} end f() return 1", sets(""));
    let fuel = |src: &str, fast: bool| super::sweep5b_open::measured(src, super::common::TEST_SEED, fast).1;
    for fast in [true, false] {
        let (u, r, g) = (fuel(&via_upvalue, fast), fuel(&via_register, fast), fuel(&via_globals, fast));
        // (Within 5%: the globals table starts with the libraries in it, so it grows at
        // other sizes. Before the fix the globals cost 42% of it.)
        assert!(u * 100 >= r * 95, "fast {fast}: upvalue {u}, register {r}");
        assert!(g * 100 >= r * 95, "fast {fast}: globals {g}, register {r}");
    }
    assert_eq!(fuel(&via_upvalue, true), fuel(&via_upvalue, false));
    assert_eq!(fuel(&via_globals, true), fuel(&via_globals, false));
}

/// S5COR-15 (found by the confirmation fuzz run; pre-existing): C Lua numbers a `return`
/// where its values end (the last token its code generator read), so a `__close` that fails
/// as the function returns names that line; RemedyLua numbered it where `return` stands.
/// Both compilers.
#[test]
fn parity_15_returns_are_numbered_where_their_values_end() {
    same_as_c(include_str!("lua/parity/return_lines.lua"));
}

/// S5COR-17 (found by the confirmation fuzz; pre-existing): `select` counts when its first
/// argument is a string that starts with `#`, as C Lua checks only the first byte; RemedyLua
/// wanted exactly `"#"`.
#[test]
fn parity_17_select_counts_for_any_string_starting_with_hash() {
    same_as_c(
        r###"
        local r = {}
        for _, k in ipairs({"#", "#3", "#x", "##", "x#", "3", "-1"}) do
            local ok, v = pcall(select, k, "a", "b", "c")
            r[#r + 1] = k .. "=" .. tostring(ok) .. ":" .. tostring(v)
        end
        return table.concat(r, " ")
        "###,
    );
}

/// S5COR-18 (found by the confirmation fuzz; pre-existing): `string.format`'s `%q` with a
/// value that has no literal form named argument #2 whatever its position.
#[test]
fn parity_18_format_q_names_its_own_argument() {
    same_as_c(
        r###"
        local r = {}
        for _, c in ipairs({
            {"%q", {}}, {"%s %q", 1, {}}, {"%s|%10s|%-10s|%.2s|%q|%q", "abc", "right", "left", 1, math},
            {"%d %5.1f %q", 1, 2, print}, {"%q %q %q", 1, "x", coroutine.create(print)},
        }) do
            r[#r + 1] = select(2, pcall(string.format, table.unpack(c)))
        end
        return table.concat(r, "\n")
        "###,
    );
}

/// Each program's error, caught, against C Lua's (fast loop on and off, via `remedy`);
/// `want` must be in it, so the test cannot pass on a message both engines lost.
fn same_error_as_c(cases: &[(&str, &str)]) {
    for &(p, want) in cases {
        let prog = format!("return select(2, pcall(function() {p} end))");
        let ours = remedy(&prog, 2_000_000_000);
        assert_eq!(ours, oracle(&prog, 2_000_000_000), "{p}");
        let text = ours.unwrap_or_else(|e| panic!("{p}: {e}"));
        assert!(text.contains(want), "{p}: {text}");
    }
}

/// LR-4 (long-run fuzz): "table index is nil" (and NaN) in a constructor was put on the
/// key's line. C Lua emits the store once the value is read, on the line the value's last
/// token ends on (`ls->lastline`).
#[test]
fn parity_19_constructor_store_is_on_the_values_last_line() {
    same_error_as_c(&[
        ("local k; local t = {[k] = function()\n\nend}", "t:3: table index is nil"),
        ("local k; local t = {[0/0] = function()\n\nend}", "t:3: table index is NaN"),
        ("local k; local t = {[k] = 1 +\n2\n}", "t:2: table index is nil"),
        ("local k; local t = {[k] = [[a\n\nb]]}", "t:3: table index is nil"),
        ("local k; local t = {[k] = (\n1\n)}", "t:3: table index is nil"),
        ("local k; local t = {[k] = print(\n1)\n}", "t:2: table index is nil"),
        ("local k; local t = {[k] = {\n}\n}", "t:2: table index is nil"),
        ("local k; local t = {[\nk\n] = 1}", "t:3: table index is nil"),
        ("local k; local t = {1, 2, [k] = 'x' .. \n 'y'}", "t:2: table index is nil"),
        ("local k; local t = {x = 1, [k] = -\n1}", "t:2: table index is nil"),
        ("local k; local t = {[k] = 1 and\n2 or\n3}", "t:3: table index is nil"),
    ]);
}

/// LR-5 (long-run fuzz): `table.concat` named a bad value by its `__name`; C Lua's
/// `addfield` uses `luaL_typename`, the plain type. The other side of the same split:
/// `luaL_typeerror` reads `__name` from any value's metatable, the strings' one included,
/// where RemedyLua skipped strings; the machine's own messages (`luaT_objtypename`) look
/// only at tables and userdata.
#[test]
fn parity_20_type_names_follow_typename_or_name_as_c_does() {
    let s = r#"getmetatable("").__name = "Foo" "#;
    let t = r#"local T = setmetatable({}, {__name = "T"}) "#;
    same_error_as_c(&[
        (
            r#"return table.concat({1, setmetatable({}, {__name = "MyType"})})"#,
            "invalid value (table) at index 2",
        ),
        (&format!("{s}return math.floor('x')"), "number expected, got Foo"),
        (&format!("{s}return string.char('x')"), "number expected, got Foo"),
        (&format!("{s}return table.insert('x', 1)"), "table expected, got Foo"),
        (&format!("{s}return setmetatable('x', {{}})"), "table expected, got Foo"),
        (&format!("{s}return next('x')"), "table expected, got Foo"),
        (&format!("{s}return string.format('%d', 'x')"), "number expected, got Foo"),
        (&format!("{s}return coroutine.resume('x')"), "thread expected, got Foo"),
        (&format!("{s}return utf8.char('x')"), "number expected, got Foo"),
        (&format!("{s}return ('x')()"), "attempt to call a string value"),
        (&format!("{s}return ('x') < 1"), "attempt to compare string with number"),
        (&format!("{s}return ('x') .. {{}}"), "attempt to concatenate a table value"),
        (&format!("{t}return string.gsub('a', 'a', {{a = T}})"), "invalid replacement value (a table)"),
        (&format!("{t}return math.floor(T)"), "number expected, got T"),
        (&format!("{t}return T < 1"), "attempt to compare T with number"),
        (&format!("{t}return T .. 'x'"), "attempt to concatenate a T value"),
        (&format!("{t}return error((tostring(T):gsub('0x%x+', '')))"), "T: "),
    ]);
    // The host's message for an uncaught non-string error, as `lua.c`'s handler words it.
    let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
    let e = lua.exec("error(setmetatable({}, {__name = 'T'}))", "=t").map(|_| ()).unwrap_err().to_string();
    assert!(e.contains("(error object is a table value)"), "{e}");
}

/// LR-9 (long-run fuzz): `utf8.codes`'s iterator checked its control value as an integer.
/// C's `iter_aux` reads it with an unchecked `lua_tointeger` (not an integer: 0) as an
/// unsigned index (negative: past the end, no values), and skips continuation bytes from
/// any index, 0 included.
#[test]
fn parity_21_utf8_codes_iterator_reads_its_control_as_c_does() {
    same_error_as_c(&[
        (r#"for p in utf8.codes "abc"("\u{D800}", true) do end"#, "invalid UTF-8 code"),
        (r#"return utf8.codes("abc")("\u{D800}", {})"#, "invalid UTF-8 code"),
    ]);
    let cases = [
        r#"return utf8.codes("abc")("abc", true)"#,
        r#"return utf8.codes("abc")("abc", nil)"#,
        r#"return utf8.codes("abc")("abc")"#,
        r#"return utf8.codes("abc")("abc", 1.5)"#,
        r#"return utf8.codes("abc")("abc", 1.0)"#,
        r#"return utf8.codes("abc")("abc", "1")"#,
        r##"return select("#", utf8.codes("abc")("abc", -1))"##,
        r##"return select("#", utf8.codes("abc")("abc", math.mininteger))"##,
        r#"return utf8.codes("abc")("\x80\x80a", 0)"#,
        r#"return utf8.codes("abc")(12, 1)"#,
        r#"return utf8.codes("a")(("\x80"):rep(100000) .. "z", 0)"#,
    ];
    for p in cases {
        let prog = format!("return pcall(function() {p} end)");
        let ours = remedy(&prog, 2_000_000_000);
        assert_eq!(ours, oracle(&prog, 2_000_000_000), "{p}");
    }
}

/// LR-3 (routed from the long-run fuzz triage; pre-existing): nested resumes stop where C
/// Lua's do. C Lua's `lua_resume` starts a thread's C levels (`nCcalls`) from its resumer's
/// and refuses at 200 before the coroutine runs, with "C stack overflow" and no position:
/// `coroutine.resume` returns it, `wrap` raises it (positioned at its caller when that is a
/// Lua function). So a `pcall` around each resume halves the depth (99) and a bare resume
/// chain reaches 198. At the 199th level the resume runs and a coroutine not yet started
/// fails as its body is called (dead afterwards; a suspended one runs on). RemedyLua
/// stopped at 200 nested resumes in every shape, raised from `coroutine.resume` instead of
/// returning, and positioned the message. Runs on the 64 KiB stack.
#[test]
fn parity_22_nested_resumes_count_c_levels() {
    let program = r###"
        local out = {}
        local function rec(name, mk)
          local n, last = 0, nil
          local function f()
            n = n + 1
            local ok, e = mk(f)
            if not ok then last = last or (n .. " " .. tostring(e)) error(e, 0) end
          end
          local ok, e = pcall(f)
          out[#out + 1] = name .. ": " .. tostring(last) .. " | " .. tostring(ok) .. " " .. #tostring(e) .. " " .. tostring(e):sub(1, 40)
        end
        rec("wrap+pcall", function(f) return pcall(coroutine.wrap(f)) end)
        rec("resume", function(f) local ok, e = coroutine.resume(coroutine.create(f)) return ok, e end)
        rec("resume+pcall", function(f) local ok, a, b = pcall(coroutine.resume, coroutine.create(f)) if not ok then return ok, a end return a, b end)
        -- (One protected call more, so that C Lua's 200th level is reached by the resume:
        -- with the other parity it is reached by C Lua's `pcall`, which refuses there, and
        -- ours (protected calls past C Lua's limit run: item 3) stops at the next resume.)
        pcall(rec, "wrap-from-lua", function(f) local co = coroutine.wrap(f) return pcall(function() return co() end) end)
        rec("xpcall+wrap", function(f) return xpcall(coroutine.wrap(f), function(m) return m end) end)
        rec("pcall-pcall-resume", function(f) return pcall(pcall, coroutine.resume, coroutine.create(f)) end)
        -- The LR-3 program itself.
        local n = 0
        local function g() n = n + 1 local co = coroutine.wrap(g) local ok, e = pcall(co) if not ok then out[#out + 1] = n .. " " .. e error(e, 0) end end
        out[#out + 1] = tostring(select(2, pcall(g)))
        -- One coroutine resumed from different depths: its count follows its resumer's.
        local gen = coroutine.wrap(function() while true do coroutine.yield(1) end end)
        local function at(d) if d == 0 then return pcall(gen) end return coroutine.wrap(at)(d - 1) end
        for _, d in ipairs({0, 50, 150, 196, 197, 198, 199, 20}) do out[#out + 1] = d .. ": " .. tostring(select(2, pcall(at, d))) end
        -- A message handler inside the chain counts the resumers' levels too.
        local function h(d)
          if d == 0 then
            local k = 0
            local _, e = xpcall(error, function(m) k = k + 1 error(m, 0) end, "x")
            return k .. " " .. tostring(e)
          end
          local ok, v = coroutine.resume(coroutine.create(h), d - 1)
          return v
        end
        for _, d in ipairs({0, 10, 100, 190}) do out[#out + 1] = "handler@" .. d .. ": " .. tostring(h(d)) end
        -- At C Lua's 199th level a resume runs, takes the 200th, and a coroutine not yet
        -- started fails in the call of its body: it is dead after (a suspended one goes on).
        -- At the 200th the resume is refused and the coroutine stays as it was.
        for _, depth in ipairs({194, 195, 196, 197, 198}) do
          local fresh = coroutine.create(function(a) return "ran " .. tostring(a) end)
          local started = coroutine.create(function() while true do coroutine.yield("again") end end)
          coroutine.resume(started)
          local wrapped = coroutine.wrap(function() return "wran" end)
          local seen = {}
          local function deep(d)
            if d == 0 then
              seen[#seen + 1] = table.concat({tostring(coroutine.resume(fresh, 1))}, " ")
              seen[#seen + 1] = table.concat({tostring(coroutine.resume(started))}, " ")
              seen[#seen + 1] = tostring(select(2, pcall(wrapped)))
              return
            end
            local ok, e = coroutine.resume(coroutine.create(deep), d - 1)
            if not ok then seen[#seen + 1] = "chain " .. d .. " " .. tostring(e) end
          end
          deep(depth)
          seen[#seen + 1] = coroutine.status(fresh) .. " " .. coroutine.status(started)
          seen[#seen + 1] = table.concat({tostring(coroutine.resume(fresh, 2))}, " ")
          seen[#seen + 1] = table.concat({tostring(coroutine.close(fresh))}, " ")
          seen[#seen + 1] = tostring(select(2, pcall(wrapped)))
          out[#out + 1] = depth .. ": " .. table.concat(seen, " / ")
        end
        -- Resumes from the main thread above 190 locals: the call slot puts the quick bound
        -- past C Lua's limit, so those resumes count exactly, and they run.
        local wide = load("local " .. string.rep("a, ", 189) .. "b = 1 " ..
          "local function r(d) if d == 0 then return 'deep' end local ok, v = coroutine.resume(coroutine.create(r), d - 1) return v end " ..
          "return r(150), select(2, coroutine.resume(coroutine.create(function() return 'one' end)))")
        out[#out + 1] = table.concat({wide()}, " ")
        local wide2 = load("local " .. string.rep("a, ", 189) .. "b = 1 " ..
          "local n = 0 local function r() n = n + 1 local ok, e = pcall(coroutine.wrap(r)) if not ok then error(e, 0) end end " ..
          "local ok, e = pcall(r) return n, e")
        out[#out + 1] = table.concat({wide2()}, " ")
        return table.concat(out, "\n")
    "###;
    // C Lua on the test's own thread (its resumes are real C levels); ours, both loops
    // (`remedy` asserts they agree), on the 64 KiB stack.
    let theirs = oracle(program, 2_000_000_000);
    let ours = super::common::small_stack(move || remedy(program, 2_000_000_000));
    assert_eq!(ours, theirs);
    assert!(ours.is_ok(), "{ours:?}");
}

/// S5COR-19 (found running the LR-3 repro; pre-existing): `gsub` with a table replacement
/// reads only the first capture (C Lua's `add_value` pushes one), so an unfinished later
/// capture is no error there; RemedyLua built every capture and raised "unfinished
/// capture". A function replacement, `%2`, `find`, `match` and `gmatch` read them all.
#[test]
fn parity_23_gsub_table_reads_only_the_first_capture() {
    same_as_c(
        r###"
        local r = {}
        local function t(...) local ok, a, b = pcall(...) r[#r + 1] = tostring(ok) .. " " .. tostring(a) .. " " .. tostring(b) end
        t(string.gsub, "abc", "(b)(", {b = "B"})
        t(string.gsub, "abcb", "(b)()(", {b = "B"})
        t(string.gsub, "abc", "(b)(", function(x) return x:upper() end)
        t(string.gsub, "abc", "(b)(", "%1")
        t(string.gsub, "abc", "(b)(", "%2")
        t(string.gsub, "abc", "b(", {b = 1})
        t(string.gsub, "abc", "b", {b = 1})
        t(string.gsub, "abc", "()b(", {[2] = "two"})
        t(string.find, "abc", "(b)(")
        t(string.match, "abc", "(b)(")
        t(function() local n = 0 for a in string.gmatch("abc", "(b)(") do n = n + 1 end return n end)
        return table.concat(r, "\n")
        "###,
    );
}

/// LR-12 (routed from the long-run fuzz triage; item 4's promise): recursion that passes its
/// varargs on grows the value stack quadratically with depth (each level copies them), and
/// that growth failing against the budget was the uncatchable memory limit. It is C Lua's
/// catchable "stack overflow" now, as for frames: the thread's own stack fills the budget.
/// The memory comes back as the error unwinds. 64 KiB stack, both loops, against C Lua.
#[test]
fn parity_24_vararg_recursion_overflows_catchably() {
    let program = r###"
        function f(...) return f("#", ...), ... end
        local r = {}
        for i = 1, 2 do r[#r + 1] = table.concat({tostring(select(1, pcall(f))), select(2, pcall(f))}, " ") end
        -- (Not a tail call: a tail call reuses its frame, and the copies only take time.)
        local function g(n, ...) local v = g(n + 1, n, ...) return v end
        r[#r + 1] = tostring(select(2, pcall(g, 1)))
        return table.concat(r, "\n")
    "###;
    let theirs = oracle(program, 4_000_000_000);
    let ours = super::common::small_stack(move || remedy(program, 4_000_000_000));
    assert_eq!(ours, theirs);
    assert!(ours.as_ref().is_ok_and(|s| s.contains("stack overflow")), "{ours:?}");
    // The budget's room comes back: the same program runs again in the same state.
    let again = super::common::small_stack(move || {
        let mut lua =
            remedy_lua::Lua::new(remedy_lua::Limits::default().fuel(1 << 34), remedy_lua::Libs::ALL);
        let mut out = Vec::new();
        for _ in 0..3 {
            let v = lua
                .exec("function f(...) return f('#', ...), ... end return select(2, pcall(f))", "=t")
                .map(|v| lua.to_string_lossy(v[0]));
            out.push(v.map_err(|e| e.to_string()));
        }
        lua.exec("collectgarbage()", "=t").unwrap();
        (out, lua.memory_used())
    });
    assert!(
        again.0.iter().all(|v| v.as_ref().is_ok_and(|s| s.as_deref() == Some("t:1: stack overflow"))),
        "{again:?}"
    );
    assert!(again.1 < 4 << 20, "{again:?}");
}

/// S6-7 (sweep 6's open item; the C-level model LR-3 uses): a message handler that keeps
/// failing is called again where its error was raised (`luaG_errormsg` runs at the raise),
/// so an error raised inside a metamethod, a library function's callback or a `__close`
/// costs C Lua more levels per round: 109 calls (an `__index` error), 73 (two levels), 55
/// (three) where a plain error makes 216. RemedyLua counted one level a round. And a call
/// falling on C Lua's 200th level raises "C stack overflow" there, positioned at the Lua
/// function making it when that is a metamethod call from Lua code. Counts and messages
/// against C Lua, both loops. (Not here: an `xpcall` inside the handler, whose own call
/// falls on the 200th in C Lua one round earlier; protected calls past C Lua's limit run
/// here, item 3.)
#[test]
fn parity_25_handler_counts_follow_the_raise_level() {
    same_as_c(
        r####"
local out = {}
local function run(name, mk)
  local k = 0
  local h = mk(function() k = k + 1 end)
  local ok, e = xpcall(error, h, "x")
  out[#out + 1] = name .. ": " .. k .. " " .. tostring(e)
end
run("plain", function(c) return function(m) c() error(m, 0) end end)
run("index-fn", function(c) return function(m) c() local t = setmetatable({}, {__index = function() error(m, 0) end}) return t.x end end)
run("index-fn-direct", function(c) local t = setmetatable({}, {__index = function(_, m) error(m, 0) end}) return function(m) c() return t[m] end end)
run("newindex", function(c) local t = setmetatable({}, {__newindex = function(_, k) error(k, 0) end}) return function(m) c() t[m] = 1 end end)
run("call-mm", function(c) local t = setmetatable({}, {__call = function(_, m) error(m, 0) end}) return function(m) c() return t(m) end end)
run("add-mm", function(c) local t = setmetatable({}, {__add = function(a, b) error(b, 0) end}) return function(m) c() return t + m end end)
run("concat-mm", function(c) local t = setmetatable({}, {__concat = function(a, b) error(b, 0) end}) return function(m) c() return t .. m end end)
run("eq-mm", function(c) local mt = {__eq = function(a, b) error("e", 0) end} local a, b = setmetatable({}, mt), setmetatable({}, mt) return function(m) c() return a == b end end)
run("tostring", function(c) local t = setmetatable({}, {__tostring = function() error("ts", 0) end}) return function(m) c() return tostring(t) end end)
run("pcall-in", function(c) return function(m) c() local ok, e = pcall(error, m, 0) error(e, 0) end end)
run("sort", function(c) return function(m) c() table.sort({3, 2, 1}, function() error(m, 0) end) end end)
run("gsub", function(c) return function(m) c() string.gsub("a", "a", function() error(m, 0) end) end end)
run("close", function(c) return function(m) c() do local x <close> = setmetatable({}, {__close = function() error(m, 0) end}) end end end)
run("index-chain", function(c) local t = setmetatable({}, {__index = setmetatable({}, {__index = function(_, m) error(m, 0) end})}) return function(m) c() return t[m] end end)
run("len-mm", function(c) local t = setmetatable({}, {__len = function() error("l", 0) end}) return function(m) c() return #t end end)
run("wrap-in", function(c) return function(m) c() coroutine.wrap(function() error(m, 0) end)() end end)

local function run(name, mk)
  local k, first, seen = 0, nil, {}
  local h = mk(function(m) k = k + 1 if tostring(m):find("overflow") then first = first or (k .. "=" .. tostring(m)) seen[#seen+1] = k end end)
  local ok, e = xpcall(error, h, "x")
  out[#out + 1] = name .. ": " .. k .. " overflow-at " .. tostring(first) .. " [" .. table.concat(seen, ",") .. "] " .. tostring(e)
end
run("plain", function(c) return function(m) c(m) error(m, 0) end end)
run("index-fn", function(c) local t = setmetatable({}, {__index = function(_, m) error(m, 0) end}) return function(m) c(m) return t[m] end end)
run("sort", function(c) return function(m) c(m) table.sort({3, 2, 1}, function() error(m, 0) end) end end)
run("two", function(c) local t = setmetatable({}, {__index = function(_, m) return setmetatable({}, {__index = function(_, m2) error(m2, 0) end})[m] end}) return function(m) c(m) return t[m] end end)

local function run(name, mk, pre)
  local k, first = 0, nil
  local h = mk(function(m) k = k + 1 if type(m) == "string" and m:find("overflow") then first = first or (k .. "=" .. m) end end)
  local ok, e = (pre or function(f) return f() end)(function() return xpcall(error, h, "x") end)
  out[#out + 1] = name .. ": " .. k .. " " .. tostring(first) .. " " .. tostring(e)
end
local idx = setmetatable({}, {__index = function(_, m) error(m, 0) end})
run("sort-index", function(c) return function(m) c(m) table.sort({3, 2, 1}, function() return idx[m] end) end end)
run("tostring-index", function(c) local t = setmetatable({}, {__tostring = function() return idx.zz end}) return function(m) c(m) return tostring(t) end end)
run("index-sort", function(c) local t = setmetatable({}, {__index = function(_, m) table.sort({2, 1}, function() error(m, 0) end) end}) return function(m) c(m) return t[m] end end)
run("close-index", function(c) return function(m) c(m) do local x <close> = setmetatable({}, {__close = function() return idx[m] end}) end end end)
run("helper", function(c) local function help(m) return idx[m] end return function(m) c(m) return help(m) end end)
run("index3", function(c) local a = setmetatable({}, {__index = function(_, m) return setmetatable({}, {__index = function(_, m2) return setmetatable({}, {__index = function(_, m3) error(m3, 0) end})[m2] end})[m] end}) return function(m) c(m) return a[m] end end)
run("in-coroutine", function(c) return function(m) c(m) return idx[m] end end, function(f) return coroutine.wrap(f)() end)
run("in-pcall", function(c) return function(m) c(m) return idx[m] end end, function(f) return select(2, pcall(f)) end)
run("table-error", function(c) local t = setmetatable({}, {__index = function(_, m) error({m}, 0) end}) return function(m) c(type(m) == "table" and m[1] or m) return t[type(m) == "table" and m[1] or m] end end)
run("gsub-index", function(c) return function(m) c(m) string.gsub("a", "a", function() return idx[m] end) end end)
run("concat-index", function(c) local t = setmetatable({}, {__concat = function(a, b) return idx[b] end}) return function(m) c(m) return t .. m end end)

return table.concat(out, "\n")
        "####,
    );
}

/// LR-19 (routed from the long-run triage): a `goto` out of a block holding a to-be-closed
/// variable, to a label past a local with a run-time value, is compiled wrong by C Lua
/// 5.4.7: it runs the `__close` handlers once instead of each time, ends the loop early,
/// fails with "attempt to call a nil value (metamethod 'close')", or crashes (this program
/// makes it segfault). RemedyLua runs every handler as the language says, deliberately
/// (`docs/reference.md`, Beyond Lua 5.4). RemedyLua only: the output is pinned, both loops.
#[test]
fn parity_26_goto_out_of_a_to_be_closed_block_closes_it() {
    let program = r####"
local ev = {}
local function closer(name) return setmetatable({}, {__close = function(_, e) ev[#ev + 1] = name .. (e and ":err" or "") end}) end
-- The routed shape: a goto out of a block with a to-be-closed variable, to a label past a
-- local with a run-time value.
for i = 0, 1 do
  do local c <close> = closer("c" .. i) if true then goto cont end end
  local v = {}
  ::cont::
end
ev[#ev + 1] = "done1"
-- Several iterations, the goto taken only sometimes.
for i = 1, 4 do
  do local c <close> = closer("d" .. i) if i % 2 == 0 then goto cont end ev[#ev + 1] = "fell" .. i end
  local v = {i}
  ev[#ev + 1] = "after" .. #v
  ::cont::
end
ev[#ev + 1] = "done2"
-- A while loop, two to-be-closed variables, the label's block holding another one.
local n = 0
while n < 2 do
  n = n + 1
  do
    local a <close> = closer("a" .. n)
    local b <close> = closer("b" .. n)
    if n > 0 then goto next end
  end
  local w = setmetatable({}, {})
  ::next::
end
ev[#ev + 1] = "done3"
return table.concat(ev, " ")
    "####;
    assert_eq!(
        remedy(program, 2_000_000_000),
        Ok("\"c0 c1 done1 fell1 d1 after1 d2 fell3 d3 after1 d4 done2 b1 a1 b2 a2 done3\"".to_string())
    );
}

/// LR-12's class, routed from triage (fuzz_mutated seeds 10023 and 10025): memory filling
/// where C Lua would already have stopped with its own catchable error is that error.
/// Recursion keeping a table per frame fills the budget before the thread's stack reaches
/// its share, but past C Lua's 1,000,000 stack slots ("stack overflow"); a metamethod that
/// re-enters itself allocating is past C Lua's 200 C levels ("C stack overflow"). Both
/// were the uncatchable memory limit. 64 KiB stack, both loops, against C Lua; the memory
/// comes back, and the program runs again in the same state.
#[test]
fn parity_27_memory_past_c_luas_limits_is_its_error() {
    let programs = [
        "function r() return {} + r() end ok = pcall(r) return ok",
        "local function rc() local f = function() return 1 end return f() + rc() end return tostring(select(2, pcall(rc)))",
        "V = {} __add = function() setmetatable({x}, V) end \
         V.__unm = function(a) return setmetatable({x = -a>>x}) end \
         local a = setmetatable({1}, V), setmetatable({}, V) \
         return (a).x, (-a)",
        "V = {} V.__unm = function(a) return setmetatable({x = -a>>x}) end \
         local a = setmetatable({1}, V) return tostring(select(2, pcall(function() return -a end)))",
    ];
    for p in programs {
        let theirs = oracle(p, 4_000_000_000);
        let ours = super::common::small_stack(move || remedy(p, 4_000_000_000));
        assert_eq!(ours, theirs, "{p}");
    }
    let again = super::common::small_stack(|| {
        // (256 MiB: with less, memory runs out before C Lua's 1,000,000 slots, and that
        // is the memory limit, as C Lua would not have overflowed yet.)
        let mut lua = remedy_lua::Lua::new(
            remedy_lua::Limits::default().fuel(1 << 34).memory(256 << 20),
            remedy_lua::Libs::ALL,
        );
        let mut out = Vec::new();
        for _ in 0..3 {
            let v = lua
                .exec("function r() return {} + r() end return pcall(r)", "=t")
                .map(|v| v.iter().map(|x| lua.to_string_lossy(*x)).collect::<Vec<_>>());
            out.push(v.map_err(|e| e.to_string()));
        }
        lua.exec("collectgarbage()", "=t").unwrap();
        (out, lua.memory_used())
    });
    assert!(
        again
            .0
            .iter()
            .all(|v| v.as_ref().is_ok_and(|s| s.get(1) == Some(&Some("t:1: stack overflow".to_string())))),
        "{:?}",
        again.0
    );
    assert!(again.1 < 4 << 20, "{}", again.1);
}

/// LR-25 (routed; a C Lua 5.4.7 code-generation bug): a comparison as the key into an
/// upvalue table. C Lua fails with "attempt to index a number value"; RemedyLua indexes
/// with the boolean, as the manual says. RemedyLua only: pinned, both loops.
#[test]
fn parity_28_comparison_keys_into_upvalue_tables() {
    let program = "local t = {} \
        local function w() t[1 < 2] = 1 t[1 > 2] = 'no' end \
        local function r() return t[1 == 1], t[2 <= 1], t[not nil] end \
        w() return table.concat({tostring(t[true]), tostring(t[false]), tostring((r()))}, ' ')";
    assert_eq!(remedy(program, 1_000_000), Ok("\"1 no 1\"".to_string()));
}

/// LR-27 (routed, fuzz_mutated seed 10032): a syntax error inside a `repeat` body after a
/// label. C Lua checks for `until` before leaving the body (where its gotos are resolved),
/// and a label just before a block-ending token is the last one, so `repeat goto l local x
/// = 1 ::l:: end` is "'until' expected"; RemedyLua reported the goto. Against C Lua's
/// messages through `load`, both loops, with the `do` and complete-`repeat` shapes.
#[test]
fn parity_29_repeat_checks_until_before_its_gotos() {
    same_as_c(
        r###"
        local r = {}
        for _, s in ipairs({
            "repeat goto l local x = 1 ::l:: end",
            "do goto l local x = 1 ::l:: end",
            "repeat goto l local x = 1 ::l:: until true",
            "repeat goto l local x = 1 ::l:: ::m:: end",
            "repeat goto l local x = 1 ::l:: else",
            "repeat goto l local x = 1 ::l:: )",
            "while true do repeat goto l local x = 1 ::l:: end",
            "repeat do goto l end local x = 1 ::l:: end",
            "repeat goto l local x = 1 ::l:: x = 2 end",
        }) do
            local f, e = load(s, "=t")
            r[#r + 1] = tostring(e)
        end
        return table.concat(r, "\n")
        "###,
    );
}
