//! Sweep 5, area "correctness": differences from C Lua found by the bounded long fuzz.
//! Each program runs in RemedyLua with the fast loop on and off
//! (`common::remedy`: results, fuel and memory identical) and is compared with C Lua.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

fn same_as_c(program: &str) {
    let ours = common::remedy(program, 100_000_000);
    let c = common::oracle(program, 100_000_000);
    assert_eq!(ours, c, "{program}");
}

/// S5COR-2: a back-reference to a position capture (`()%1`) never matches, as in C Lua
/// (its length is `(size_t)CAP_POSITION`); it matched the empty string.
#[test]
fn s5cor_2_backreference_to_a_position_capture_never_matches() {
    same_as_c(
        r#"
        local r = {string.match("0x", "()%1"), string.find("aa", "()%1"), string.find("abab", "(ab)%1")}
        r[#r + 1] = select(2, string.gsub("a b", " .()%1", "[%0]"))
        r[#r + 1] = string.gsub("a b", " .()%1", "[%0]")
        for a, b in string.gmatch("xyxy", "()(%1)") do r[#r + 1] = a end
        r[#r + 1] = string.match("aXbX", "(a)()X.%2")
        return r
        "#,
    );
}

/// S5COR-3: error messages quote a script's bytes as they are (`string.pack`'s option,
/// `string.format`'s conversion, `os.date`'s specifier), not converted to UTF-8.
#[test]
fn s5cor_3_error_messages_keep_raw_bytes() {
    same_as_c(
        r#"
        local function bytes(ok, e) return tostring(ok) .. ": " .. table.concat({string.byte(tostring(e), 1, -1)}, " ") end
        return {
            bytes(pcall(string.unpack, "\xC3", "abcd")),
            bytes(pcall(string.pack, "i\xC3", 4)),
            bytes(pcall(string.packsize, "\xFF")),
            bytes(pcall(function() return string.pack("i\xC3", 4) end)),
            bytes(pcall(string.format, "%\xC3", 4)),
            bytes(pcall(string.format, "%5\xE9", 4)),
            bytes(pcall(function() return ("%\xC3"):format(4) end)),
            bytes(pcall(os.date, "%E\xC3")),
            bytes(pcall(os.date, "%\xC3\xA9x")),
            bytes(pcall(function() return os.date("!%O\x80", 0) end)),
            bytes(pcall(string.pack, "y", 4)),
            bytes(pcall(string.format, "%y", 4)),
        }
        "#,
    );
}

/// S5COR-4: `return f()` inside a generic `for` is not a tail call: the loop's closing value
/// is to-be-closed (C Lua's `marktobeclosed`), so it closes after `f` returns or raises, and
/// `__close` sees the error. It closed first, with nil, then tail-called `f`. Tail calls
/// after the loop, and in functions made inside it, stay tail calls.
#[test]
fn s5cor_4_return_call_inside_generic_for_closes_after_the_call() {
    same_as_c(
        r##"
        local log = {}
        local function closing(tag)
            return setmetatable({}, {__close = function(o, e) log[#log + 1] = tag .. ":" .. tostring(e) end})
        end
        local function boom() log[#log + 1] = "boom" error("x", 0) end
        local function value(...) log[#log + 1] = "value" return ... end
        local r = {pcall(function()
            for _ in next, {1}, nil, closing("a") do return boom() end
        end)}
        r[#r + 1] = select("#", (function()
            for _ in next, {1}, nil, closing("b") do return value(1, nil, 3) end
        end)())
        r[#r + 1] = (function() for _ in next, {1}, nil, closing("c") do return value("v") end end)()
        -- Nested loops, and a method call.
        local obj = {m = function(self, x) log[#log + 1] = "m" return x * 2 end}
        r[#r + 1] = (function()
            for _ in next, {1}, nil, closing("outer") do
                for _ in next, {1}, nil, closing("inner") do return obj:m(21) end
            end
        end)()
        -- After the loop a tail call is a tail call again (deeper than the call depth).
        local function down(n)
            for _ in pairs({}) do end
            if n == 0 then return "bottom" end
            return down(n - 1)
        end
        r[#r + 1] = down(100000)
        -- And in a function made inside the loop.
        local made
        for _ in next, {1}, nil, closing("d") do
            made = function(n) if n == 0 then return "deep" end return made(n - 1) end
        end
        r[#r + 1] = made(100000)
        r[#r + 1] = table.concat(log, " ")
        return r
        "##,
    );
}

/// S5COR-5: an error inside `xpcall`'s message handler calls the handler again with that
/// error (it stays in place while it runs, as C Lua's `luaG_errormsg`); ours gave "error in
/// error handling" at once. A handler that keeps failing sees "C stack overflow" at C Lua's
/// 200th C level and stops at its limit with "error in error handling", after as many calls
/// as in C Lua, however deep in protected calls and coroutines it runs.
#[test]
fn s5cor_5_errors_in_a_message_handler_are_handled_again() {
    same_as_c(
        r##"
        local r = {}
        r[#r + 1] = {xpcall(function() error() end, function(m) return "handled: " .. m end)}
        local n = 0
        r[#r + 1] = {xpcall(error, function(m) n = n + 1 if n < 3 then error("e" .. n, 0) end return "ok:" .. tostring(m) end, "first")}
        r[#r + 1] = n
        local function runaway(recover)
            local seen = {}
            local res = {xpcall(error, function(m)
                seen[#seen + 1] = tostring(m)
                if recover and m == "C stack overflow" then return "recovered" end
                error(m, 0)
            end, "first")}
            local k
            for i, s in ipairs(seen) do if s == "C stack overflow" then k = i break end end
            return {res[1], res[2], k, #seen, seen[#seen]}
        end
        r[#r + 1] = runaway(false)
        r[#r + 1] = runaway(true)
        r[#r + 1] = {pcall(function() return pcall(function() return runaway(false) end) end)}
        r[#r + 1] = {coroutine.wrap(function() return runaway(false) end)()}
        r[#r + 1] = {coroutine.wrap(function() return pcall(runaway, true) end)()}
        r[#r + 1] = {coroutine.resume(coroutine.create(function() return runaway(false) end))}
        r[#r + 1] = {pcall(coroutine.wrap(function() return coroutine.wrap(function() return pcall(runaway, false) end)() end))}
        r[#r + 1] = {coroutine.wrap(function() return pcall(pcall, coroutine.wrap(function() coroutine.yield(1) return runaway(true) end)) end)()}
        -- A handler that errors with a table, and one raising from a nested call.
        r[#r + 1] = {xpcall(error, function(m) if type(m) == "string" then error({m}) end return m end, "s")}
        r[#r + 1] = select(2, xpcall(error, function(m) if m == "a" then string.rep() end return m end, "a"))
        return r
        "##,
    );
}

/// S5COR-6: `pcall` or `xpcall` called by `pcall` or `xpcall` (`pcall(pcall, f)`): the
/// inner protected call's frame took the outer one's place, so the outer never put its
/// `true` first and the result began with the function (`pcall, true, ...`), whatever `f`
/// did. Found while checking S5COR-5; it was in `feat/themes` before this sweep. (Yields
/// inside: S5COR-6b, `sweep5b_open`.)
#[test]
fn s5cor_6_protected_call_of_a_protected_call() {
    same_as_c(
        r##"
        local log = {}
        local function closing(tag)
            return setmetatable({}, {__close = function(o, e) log[#log + 1] = tag .. ":" .. tostring(e) end})
        end
        local function f(...) return 1, ... end
        local function bad() local c <close> = closing("c") error("bad", 0) end
        local r = {}
        r[#r + 1] = {pcall(pcall, f, 2, 3)}
        r[#r + 1] = {pcall(pcall, function() end)}
        r[#r + 1] = select("#", pcall(pcall, function() end))
        r[#r + 1] = {pcall(pcall, error, "x")}
        r[#r + 1] = {pcall(pcall, error)}
        r[#r + 1] = {pcall(xpcall, error, function(m) log[#log + 1] = "h" return "h:" .. m end, "x")}
        r[#r + 1] = {xpcall(pcall, print, error, "y")}
        r[#r + 1] = {pcall(pcall, pcall, error, "z")}
        r[#r + 1] = {pcall(pcall, bad)}
        r[#r + 1] = {pcall(xpcall, bad, function(m) log[#log + 1] = "handler" return m end)}
        r[#r + 1] = {pcall(pcall)}
        r[#r + 1] = {pcall(xpcall, f)}
        r[#r + 1] = {pcall(pcall, string.rep, "ab", 3)}
        r[#r + 1] = {pcall(pcall, setmetatable({}, {__call = function(self, a) return "called", a end}), 7)}
        r[#r + 1] = {coroutine.wrap(function() return pcall(pcall, coroutine.wrap(function() coroutine.yield(1) end)) end)()}
        local t = setmetatable({}, {__index = function(t, k) return select(2, pcall(pcall, f, k)) end})
        r[#r + 1] = {t.key}
        r[#r + 1] = table.concat(log, " ")
        return r
        "##,
    );
}

/// S5B-SOAK-1 (found by the long soak, `gc_stress_over_the_corpus_keeps_results`; in
/// `feat/themes` before sweep 5): a coroutine that dies with an error keeps its pending
/// to-be-closed variables for `coroutine.close`, but `shrink_parked` gave a dead coroutine's
/// whole stack back, so a collection in between freed their values: the close then found
/// no `__close` and returned "attempt to call a nil value" instead of running them and
/// returning the error. Checked with plain collections, against C Lua, and under GC stress.
#[test]
fn s5b_soak_1_dead_coroutine_keeps_pending_closes_across_collections() {
    let program = r##"
        local log = {}
        local function closing(tag, f)
            return setmetatable({}, {__close = function(_, e)
                log[#log + 1] = tag .. " " .. type(e) .. " " .. tostring(type(e) == "table" and e.msg or e) .. (f and f() or "")
            end})
        end
        local cos = {}
        for n = 1, 3 do
            cos[n] = coroutine.create(function()
                local a <close> = closing("a" .. n)
                local up = 0
                local function bump() up = up + 1 return " " .. up end
                for _ in pairs({1}) do
                    local b <close> = closing("b" .. n, bump)
                    if n == 2 then error("boom" .. n) end
                    error({msg = "boom" .. n})
                end
            end)
            local ok, e = coroutine.resume(cos[n])
            log[#log + 1] = "resumed " .. tostring(ok) .. " " .. type(e)
        end
        collectgarbage() collectgarbage()
        local junk = {} for i = 1, 2000 do junk[i] = {i, tostring(i)} end junk = nil
        collectgarbage()
        for n = 3, 1, -1 do
            local c1, c2 = coroutine.close(cos[n])
            log[#log + 1] = "close" .. n .. " " .. tostring(c1) .. " " .. type(c2) .. " " .. tostring(type(c2) == "table" and c2.msg or c2)
            log[#log + 1] = coroutine.status(cos[n])
        end
        return table.concat(log, " | ")
        "##;
    same_as_c(program);
    for fast in [true, false] {
        let run = |stress: bool| {
            let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            lua.set_gc_stress(stress);
            let v = lua.exec(program, "=t").unwrap();
            lua.to_string_lossy(v[0]).unwrap()
        };
        assert_eq!(run(true), run(false), "fast {fast}");
    }
}

/// S5C-1: fuel is the same under every hash seed with the incremental collector running
/// (it was off by a unit or two: collector work depended on where keys sat in hash parts).
#[test]
fn s5c_1_fuel_is_seed_independent_with_the_collector_running() {
    let programs = [
        "local T = {} for j = 1, 6000 do T[j] = {a = j} end return #T",
        "local T = {} for j = 1, 3000 do T['k' .. j] = {j, tostring(j)} end local n = 0 for k, v in pairs(T) do n = n + #v end return n",
        "local W = setmetatable({}, {__mode = 'k'}) local keep = {} for j = 1, 4000 do local k = {} W[k] = {j} if j % 3 == 0 then keep[#keep + 1] = k end end collectgarbage() local n = 0 for _ in pairs(W) do n = n + 1 end return n",
        "local t = {} for j = 1, 5000 do t[j % 97 .. 'x'] = {} t[j * 1.5] = j end return 1",
    ];
    for src in programs {
        for fast in [true, false] {
            let mut seen = Vec::new();
            for seed in [super::common::TEST_SEED, 0, 1, 2, 7, 0xdead_beef, u64::MAX] {
                let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default().hash_seed(seed), remedy_lua::Libs::ALL);
                lua.set_fast_loop(fast);
                let f = lua.load(src, "=t").unwrap();
                let out = format!("{:?}", lua.call(f.value(), &[]).unwrap().first().map(|v| lua.to_string_lossy(*v)));
                seen.push((seed, out, lua.limits().fuel - lua.fuel_left()));
            }
            let first = &seen[0];
            assert!(seen.iter().all(|s| s.1 == first.1 && s.2 == first.2), "fast {fast}: {src}\n{seen:?}");
        }
    }
}

/// S5C-COR-1 (sweep 5 final fuzz): `error(msg, level)` in a `__close` that runs while an
/// error unwinds counted the frames being unwound as levels; C Lua runs that `__close` once
/// they are gone, on `pcall` itself (level 2 has no position there). (Levels past 4 reach
/// the test's wrapper, which the two harnesses name differently.)
#[test]
fn s5c_cor_1_error_levels_in_an_error_closing_close() {
    same_as_c(
        r##"
        local r = {}
        for lv = 0, 4 do
            r[#r + 1] = select(2, pcall(function()
                local a <close> = setmetatable({}, {__close = function() error("lv" .. lv, lv) end})
                local b <close> = setmetatable({}, {__close = function() error("b") end})
            end))
            local function g()
                local a <close> = setmetatable({}, {__close = function() error("g" .. lv, lv) end})
                error("first")
            end
            r[#r + 1] = select(2, pcall(function() g() end))
            r[#r + 1] = select(2, pcall(pcall, g))
            r[#r + 1] = select(2, xpcall(g, function(m) return "h:" .. tostring(m) end))
            r[#r + 1] = select(2, coroutine.resume(coroutine.create(function() return select(2, pcall(g)) end)))
        end
        return r
        "##,
    );
}

/// S5C-COR-2 (sweep 5 final fuzz): a chunk name is a C string in C Lua, so it ends at the
/// first zero byte (`load("\0")` is `[string ""]`), whether it is the source or a name.
#[test]
fn s5c_cor_2_chunk_names_end_at_a_zero_byte() {
    same_as_c(
        r##"
        return {select(2, load("\0")), select(2, load("a\0b c")), select(2, load("x \0 y")),
            select(2, load("x", "=ab\0cd")), select(2, load("x", "@f\0g")), select(2, load("\0\n\0"))}
        "##,
    );
}

/// S5C-COR-3: under `xpcall`, an error a `__close` raises while the first error unwinds goes
/// through the message handler too (C Lua closes with the handler in place); it was passed
/// on raw. Lua, native and missing `__close` handlers, several in a row, and handlers that
/// fail.
#[test]
fn s5c_cor_3_close_errors_go_through_the_message_handler() {
    same_as_c(
        r##"
        local function h(m) return "h(" .. tostring(m) .. ")" end
        local function closing(f) return setmetatable({}, {__close = f}) end
        local r = {}
        r[#r + 1] = {xpcall(function()
            local a <close> = closing(function(_, e) error("a<" .. tostring(e) .. ">", 0) end)
            error("first", 0)
        end, h)}
        r[#r + 1] = {xpcall(function()
            local a <close> = closing(function(_, e) error("a", 0) end)
            local b <close> = closing(function(_, e) error("b<" .. tostring(e) .. ">", 0) end)
            local c <close> = closing(function() end)
            error("first", 0)
        end, h)}
        r[#r + 1] = {xpcall(function()
            local a <close> = closing(error)
            error("first", 0)
        end, h)}
        r[#r + 1] = {xpcall(function()
            local t = closing(function() end)
            local a <close> = t
            getmetatable(t).__close = nil
            error("first", 0)
        end, h)}
        r[#r + 1] = {xpcall(function()
            local a <close> = closing(function() error("a", 0) end)
            error("first", 0)
        end, function(m) if m == "a" then error("in handler") end return "ok:" .. m end)}
        r[#r + 1] = {pcall(function()
            local a <close> = closing(function(_, e) error("a<" .. tostring(e) .. ">", 0) end)
            error("first", 0)
        end)}
        return r
        "##,
    );
}

/// S5C-COR-4 (sweep 5 final fuzz): a field of a register holding `_ENV` (`_ENV.f()`, which
/// loads the upvalue into a register first) is named "global", as C Lua's `isEnv` does; it
/// was "field". S5C-COR-5: `load` checks its mode, then its name, then that a non-string
/// chunk is a function, as C Lua does, before reading a reader.
#[test]
fn s5c_cor_4_5_env_fields_are_globals_and_load_checks_its_arguments_in_order() {
    same_as_c(
        r##"
        local r = {}
        r[#r + 1] = select(2, pcall(function() return _ENV.nope1() end))
        r[#r + 1] = select(2, pcall(function() local e = _ENV return e.nope2() end))
        r[#r + 1] = select(2, pcall(function() return _ENV.nope3 + 1 end))
        r[#r + 1] = select(2, pcall(function() local t = {} return t.nope4() end))
        r[#r + 1] = select(2, pcall(function() local _ENV = {x = {}} return x.y() end))
        r[#r + 1] = select(2, pcall(load, {}, {}))
        r[#r + 1] = select(2, pcall(load, {}, "n", {}))
        r[#r + 1] = select(2, pcall(load, {}))
        r[#r + 1] = select(2, pcall(load, "return 1", {}))
        r[#r + 1] = select(2, pcall(load, function() error("read") end, {}))
        local reads = 0
        r[#r + 1] = select(2, pcall(load, function() reads = reads + 1 return nil end, "n", {}))
        r[#r + 1] = reads
        return r
        "##,
    );
}

/// SEC7-V1: a coroutine whose body is a native that calls back into Lua (`wrap(table.sort)`
/// with a Lua comparator) took the callback's return for the body's own and died there:
/// "internal: coroutine without a resumer". And a native body's error under `pcall` was
/// caught with the coroutine still running (no results, the resumer's stack left alone).
#[test]
fn sec7_v1_a_native_coroutine_body_calls_back_into_lua() {
    same_as_c(
        r##"
        local r, lt = {}, function(a, b) return a < b end
        local t = {3, 1, 2}
        r[#r + 1] = select("#", coroutine.wrap(table.sort)(t, lt))
        r[#r + 1] = table.concat(t)
        r[#r + 1] = select("#", pcall(coroutine.wrap(table.sort), {2, 1}, lt))
        r[#r + 1] = select(2, coroutine.resume(coroutine.create(table.sort), {2, 1}, lt))
        r[#r + 1] = coroutine.wrap(string.gsub)("abc", "%w", string.upper)
        r[#r + 1] = select(2, pcall(coroutine.wrap(error), "x"))
        r[#r + 1] = select(2, pcall(coroutine.wrap(table.sort), {3, 1, 2}, function() error("e") end))
        r[#r + 1] = select(2, pcall(coroutine.wrap(table.sort), {3, 1, 2}, function() coroutine.yield() end))
        local co = coroutine.create(table.sort)
        r[#r + 1] = select(2, coroutine.resume(co, {3, 1, 2}, function() error("f") end))
        r[#r + 1] = coroutine.status(co)
        return r
        "##,
    );
}
