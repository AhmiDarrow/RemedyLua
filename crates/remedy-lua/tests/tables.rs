//! Tables and objects on the fast paths: method lookups through cached
//! `__index` hints, writes that skip a metatable known to lack `__newindex`, new string
//! keys inserted in place, and tables made in the fast loop. Every program runs with the
//! fast loop on and off (`common::remedy` checks result, fuel and memory agree) and
//! against C Lua when the oracle is built; GC-stress runs check the collector's view.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Error, LimitKind, Libs, Limits, Lua};

use super::common;

/// The program's outcome in RemedyLua (both loops) equals `want` and C Lua's.
fn check(program: &str, want: &str) {
    let ours = common::remedy(program, 50_000_000);
    assert_eq!(ours.as_deref(), Ok(want), "{program}");
    let theirs = common::oracle(program, 50_000_000);
    {
        assert_eq!(ours, theirs, "C Lua differs:\n{program}");
    }
    // The collector at every allocation sees the same. (Fuel unbounded: a whole
    // collection at every allocation charges its work like any other.)
    let mut lua = Lua::new(Limits::default().fuel(u64::MAX >> 2), Libs::ALL);
    lua.set_gc_stress(true);
    let v = lua.exec(common::wrap(program), "=stress").unwrap();
    assert_eq!(lua.to_string_lossy(v[0]).as_deref(), Some(want), "under GC stress:\n{program}");
}

/// The cached `__index` hints never outlive a change: a new method in the class, a
/// shadowing field on the instance, a new `__index` table or function, `__index`
/// removed, a new metatable, a two-level chain changed in the middle.
#[test]
fn method_cache_follows_every_change() {
    check(
        r#"
        local A = {} A.__index = A
        function A:f() return "A" end
        local B = {} B.__index = B
        function B:f() return "B" end
        local o = setmetatable({}, A)
        local out = {}
        for i = 1, 12 do
          local ok, r = pcall(function() return o:f() end)
          out[#out + 1] = ok and r or "err"
          if i == 2 then function A:f() return "A2" end end
          if i == 3 then o.f = function() return "own" end end
          if i == 4 then o.f = nil end
          if i == 5 then A.__index = B end
          if i == 6 then A.__index = function(t, k) return function() return "fn" .. k end end end
          if i == 7 then A.__index = nil end
          if i == 8 then A.__index = A setmetatable(o, B) end
          if i == 9 then rawset(B, "__index", A) end
          if i == 10 then setmetatable(o, nil) o.f = function() return "bare" end end
          if i == 11 then setmetatable(o, A) o.f = nil end
        end
        return table.concat(out, ",")
        "#,
        r#""A,A,A2,own,A2,B,fnf,err,B,A2,bare,A2""#,
    );
}

/// `obj.field` (GETFIELD) through a chain of classes, with the middle class changing.
#[test]
fn field_cache_follows_chain_changes() {
    check(
        r#"
        local Base = {x = 1} Base.__index = Base
        local Mid = setmetatable({}, Base) Mid.__index = Mid
        local o = setmetatable({}, Mid)
        local s = {}
        for i = 1, 8 do
          s[#s + 1] = tostring(o.x)
          if i == 2 then Mid.x = 2 end
          if i == 3 then Mid.x = nil end
          if i == 4 then Base.x = 4 end
          if i == 5 then getmetatable(Mid).__index = {x = 5} end
          if i == 6 then o.x = 6 end
          if i == 7 then o.x = nil Base.__index = nil end
        end
        return table.concat(s, ",")
        "#,
        r#""1,1,2,1,4,5,6,nil""#,
    );
}

/// A write to a field the object lacks skips `__newindex` only while the metatable has
/// none: one added later (by a plain write, `rawset` or a constructor-made metatable
/// swapped in) is called.
#[test]
fn newindex_added_later_is_called() {
    check(
        r#"
        local mt = {}
        local o = setmetatable({a = 1}, mt)
        local log = {}
        for i = 1, 8 do
          o.b = i      -- absent field (after the clear below)
          o.b = nil
          if i == 2 then mt.__newindex = function(t, k, v) log[#log + 1] = k .. tostring(v) end end
          if i == 3 then mt.__newindex = nil end
          if i == 4 then rawset(mt, "__newindex", function(t, k, v) log[#log + 1] = "r" .. k end) end
          if i == 5 then rawset(mt, "__newindex", nil) end
          if i == 6 then setmetatable(o, {__newindex = function(t, k, v) log[#log + 1] = "s" .. k end}) end
          if i == 7 then setmetatable(o, mt) end
        end
        o.a = 9 -- existing field: never __newindex
        return table.concat(log, ",") .. "|" .. tostring(rawget(o, "b")) .. o.a
        "#,
        r#""b3,bnil,rb,rb,sb,sb|nil9""#,
    );
}

/// `__newindex` as a table: writes of absent keys go there, present keys stay.
#[test]
fn newindex_table_receives_absent_keys() {
    check(
        r#"
        local store = {}
        local o = setmetatable({x = 1}, {__newindex = store})
        for i = 1, 5 do o.x = i o.y = i end
        return o.x .. tostring(rawget(o, "y")) .. store.y
        "#,
        r#""5nil5""#,
    );
}

/// New string keys go in place while the hash part has room and through `set` (rehash)
/// when it has not: same table, same `next` order as C Lua sees the same contents, same
/// memory both ways.
#[test]
fn new_keys_in_place_and_by_rehash() {
    check(
        r#"
        local t = {a = 1, b = 2}
        for i = 1, 200 do t["k" .. i] = i end
        for i = 1, 200, 3 do t["k" .. i] = nil end
        for i = 1, 200, 3 do t["k" .. i] = -i end
        t.never = nil
        local n, s = 0, 0
        for k, v in pairs(t) do n = n + 1 s = s + v end
        local objs = {}
        for i = 1, 1000 do objs[i] = {x = i, y = -i} objs[i].z = i end
        local z = 0
        for i = 1, 1000 do z = z + objs[i].x + objs[i].y + objs[i].z end
        return n .. " " .. s .. " " .. z .. " " .. tostring(t.k1) .. tostring(t.never)
        "#,
        r#""202 6703 500500 -1nil""#,
    );
}

/// Tables made in the fast loop count toward the cap like any other: a loop making them
/// stops with the memory error, and the state still works after.
#[test]
fn tables_in_the_fast_loop_hit_the_memory_cap() {
    let mut lua = Lua::new(Limits::default().memory(1 << 20), Libs::ALL);
    let r = lua.exec("local keep = {} for i = 1, 1e7 do keep[i] = {x = i, y = i} end", "=t");
    assert_eq!(r.err(), Some(Error::Limit(LimitKind::Memory)));
    let v = lua.exec("local t = {x = 1} return t.x + 1", "=t").unwrap();
    assert_eq!(common::show(&lua, v[0]), "2");
    // Garbage tables are collected, so a long loop of them fits.
    check("local s = 0 for i = 1, 200000 do local t = {i, x = i} s = s + t[1] + t.x end return s", "40000200000");
}

/// String methods go through the string metatable's cached `__index` hint: a new
/// `__index` (a table, then a function) is seen at once.
#[test]
fn string_method_cache_follows_index_changes() {
    check(
        r#"
        local mt = getmetatable("")
        local s, out = "ab", {}
        for i = 1, 6 do
          out[#out + 1] = s:upper()
          if i == 2 then mt.__index = {upper = function() return "X" end} end
          if i == 4 then mt.__index = function(_, k) return function() return k end end end
        end
        mt.__index = string
        return table.concat(out) .. s:upper()
        "#,
        r#""ABABXXupperupperAB""#,
    );
}

/// Field writes at the hinted node mark the new value for the collector: old objects
/// swapped between the fields of black and not-yet-marked tables in mid-cycle survive.
/// (Without the barrier this returns a wrong sum: the swapped tables are freed.)
#[test]
fn hinted_field_writes_keep_values_alive() {
    check(
        r#"
        local objs, src = {}, {}
        for i = 1, 64 do objs[i] = {f = {i}} end
        for i = 1, 64 do src[i] = {v = {64 + i}} end
        for r = 1, 4000 do
          for i = 1, 64 do
            local j = (i * 7 + r) % 64 + 1
            local a, b = objs[i], src[j]
            a.f, b.v = b.v, a.f
          end
          collectgarbage("step", 0)
          local junk = {} for k = 1, 30 do junk[k] = {k} end
        end
        collectgarbage() collectgarbage()
        local junk = {} for k = 1, 5000 do junk[k] = {-k} end
        local s = 0
        for i = 1, 64 do s = s + objs[i].f[1] + src[i].v[1] end
        return s
        "#,
        "8256",
    );
}

/// Resizing is charged as fuel by the slots it copies and the passes it makes (1 per 16)
/// and the keys it places (`PLACE_COST`, S5B-COST-2): filling a table to 64k keys costs
/// more than the same instructions writing one slot, by about a unit per key (the
/// geometric sum of its sizes), the same with the fast loop on and off.
#[test]
fn resizes_are_charged_as_fuel() {
    let grow = "local t = {} for i = 1, 65536 do t[i * 2] = i end return 1";
    let flat = "local t = {0} for i = 1, 65536 do t[i // i] = i end return 1";
    let fuel = |p: &str, fast: bool| common::remedy_measured(p, 100_000_000, fast).1;
    let (g, f) = (fuel(grow, true), fuel(flat, true));
    assert_eq!((g, f), (fuel(grow, false), fuel(flat, false)));
    let extra = g - f;
    assert!((4096..=2 * 65536).contains(&extra), "resize fuel {extra}");
}

/// Integer keys outside the array part go in place too; the next array index still
/// appends (and pulls following keys over from the hash part), so lengths, reads and
/// traversals agree with C Lua.
#[test]
fn integer_keys_in_place_and_appends() {
    check(
        r#"
        local t = {}
        for i = 200, 1, -1 do t[i] = i end          -- reversed: hash part, then migration
        local u = {}
        for i = 1, 300 do u[i * 3] = i u[-i] = -i end
        for i = 1, 300, 4 do u[i * 3] = nil end
        u[0] = 0
        local n, s = 0, 0
        for k, v in pairs(u) do n = n + 1 s = s + v end
        local a = {}
        for i = 1, 50 do a[i] = i end
        a[52] = 52 a[51] = 51                        -- 52 waits in the hash part
        return #t .. " " .. t[150] .. " " .. n .. " " .. s .. " " .. #a .. " " .. a[52]
        "#,
        r#""200 150 526 -11175 52 52""#,
    );
}

/// Integer keys that all start in one slot of a 64k-slot integer part (multiples of
/// 65535, once scattered fillers have grown it to that size). The cluster's probe steps
/// are fuel: under a 5M budget the script stops cleanly after a few thousand inserts,
/// where unmetered it would do ~10^9 steps.
const COLLIDE: &str = r#"
    local t = {}
    for k = 1, 30000 do t[-k * 2654435761] = k end
    progress = 0
    for j = 1, 40000 do t[j * 65535] = j progress = j end
    local s = 0
    for r = 1, 1000 do s = s + t[progress * 65535] end
    return s
"#;

#[test]
fn colliding_integer_keys_run_out_of_fuel() {
    for fast in [true, false] {
        let mut lua = Lua::new(Limits::default().fuel(5_000_000).memory(64 << 20), Libs::ALL);
        lua.set_fast_loop(fast);
        let t = std::time::Instant::now();
        assert_eq!(lua.exec(COLLIDE, "=t").err(), Some(Error::Limit(LimitKind::Fuel)));
        let v = lua.exec("return progress", "=t").unwrap();
        let done: i64 = common::show(&lua, v[0]).parse().unwrap();
        // Quadratic probing paid for: about sqrt(2 * budget) inserts, not 40000.
        assert!((1000..10_000).contains(&done), "fast {fast}: {done} inserts");
        assert!(t.elapsed().as_secs() < 5, "fast {fast}: took {:?}", t.elapsed());
    }
}

/// A smaller cluster runs to the end: same result as C Lua, the same fuel with the fast
/// loop on and off (checked by `check`), and fuel well above the instructions alone.
#[test]
fn colliding_integer_keys_charge_their_probes() {
    // (2000 colliding keys and the fillers fit a 32768-node array: multiples of 32767.)
    let small = COLLIDE.replace("40000", "2000").replace("1000 do", "50 do").replace("65535", "32767");
    check(&small, "100000");
    let fuel = common::remedy_measured(&small, 1_000_000_000, true).1;
    assert!(fuel > 2_000 * 2_000 / 2, "fuel {fuel}");
}

/// Long strings (over 40 bytes) are interned only when they become table keys: keys built
/// at run time with `..`, and long constant keys, stored and read through every path (field
/// and indexed reads and writes, new keys in place, methods, `__index` chains,
/// `__newindex`, rawget/rawset, next) find the same entries.
#[test]
fn long_string_keys_on_every_path() {
    check(
        r#"
        local pre = string.rep("k", 40)
        local function key(i) return pre .. i end
        local t = {}
        for i = 1, 20 do t[key(i)] = i end                 -- new keys (SetTable)
        for i = 1, 20, 2 do t[key(i)] = t[key(i)] * 10 end  -- existing keys
        t.kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk1 = -1     -- long constant (SetTableK)
        local s = t.kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk1 + t["kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkk2"]
        for i = 1, 20 do s = s + t[key(i)] + rawget(t, key(i)) end
        rawset(t, key(21), 21) s = s + t[key(21)]
        local C = {} C.__index = C
        C[pre .. "method"] = function(self, x) return x + 1 end
        local o = setmetatable({}, C)
        local m = pre .. "method"
        s = s + o[m](o, 1) + o:kkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkkmethod(2)
        local log = {}
        local w = setmetatable({[key(1)] = 1}, {__newindex = function(_, k, v) log[#log + 1] = #k end})
        w[key(1)] = 5 w[key(2)] = 6
        local n = 0
        for k, v in next, t do if t[k] ~= v then n = n + 1000 end n = n + 1 end
        return s .. " " .. w[key(1)] .. " " .. table.concat(log, ",") .. " " .. n
        "#,
        r#""2225 5 41 21""#,
    );
}

/// Array slots read and written in place: live slots, holes (nil) with and without a
/// metatable (`__index` and `__newindex` still asked for holes), integer keys past the
/// end, float keys with integer values.
#[test]
fn array_slots_in_place() {
    check(
        r#"
        local a = {1, 2, 3, 4, 5}
        a[3] = nil
        local log = {}
        local m = setmetatable({10, 20, nil, 40}, {
          __index = function(_, k) log[#log + 1] = "i" .. k return -k end,
          __newindex = function(t, k, v) log[#log + 1] = "n" .. k rawset(t, k, v) end,
        })
        local s = 0
        for r = 1, 3 do
          for i = 1, 6 do s = s + (a[i] or 0) + m[i] end
          a[2.0] = a[2] + 1
          m[1] = m[1] + 1
          m[3] = r          -- a hole: __newindex the first time, then a live slot
          a[3] = r
        end
        return s .. " " .. a[2] .. " " .. m[1] .. " " .. table.concat(log, ",")
        "#,
        r#""222 5 13 i3,i5,i6,n3,i5,i6,i5,i6""#,
    );
}

/// `math.sqrt`, `floor`, `ceil`, `abs`, `min` and `max` computed in the fast loop when the
/// callee is the library's own function: same results (integer and float subtypes,
/// -0.0, nan, huge, mininteger), errors and string arguments through the full path, extra
/// arguments ignored, results adjusted to the call; a rebound `math.sqrt` is called.
#[test]
fn math_calls_in_place() {
    check(
        r#"
        local m = math
        local sqrt, floor, ceil, abs, min, max = m.sqrt, m.floor, m.ceil, m.abs, m.min, m.max
        local out = {}
        local function put(...) local t = table.pack(...) for i = 1, t.n do out[#out + 1] = tostring(t[i]) .. (math.type(t[i]) or type(t[i])) end end
        for _, x in ipairs({4, 2.25, -0.0, 0/0, math.huge, -7, -7.5, 7.5, math.mininteger, 1e300}) do
          put(sqrt(x), floor(x), ceil(x), abs(x), min(x, 3, -1.5), max(x, 3, 1e9))
        end
        put(min(5), max(2.0, 2), min(2, 2.0), sqrt(16, "ignored"), sqrt("9"), floor("2.5"), abs("-3"))
        local a, b, c = sqrt(9)
        put(a, b, c)
        put(pcall(sqrt, {}))
        put(pcall(min))
        put(pcall(function() return sqrt({}) end))
        put(pcall(function() return max(1, nil) end))
        put(pcall(max, 1, "x"))
        local s = 0
        for i = 1, 2000 do s = s + floor(sqrt(i)) + abs(-i) % 3 + max(i, 1000) - min(i, 1000) end
        math.sqrt = function(x) return "mine" .. x end
        put(math.sqrt(4))
        math.sqrt = sqrt
        return table.concat(out, " ") .. " " .. s
        "#,
        r#""2.0float 4integer 4integer 4integer -1.5float 1000000000.0float 1.5float 2integer 3integer 2.25float -1.5float 1000000000.0float -0.0float 0integer 0integer 0.0float -1.5float 1000000000.0float -nanfloat -nanfloat -nanfloat nanfloat -nanfloat -nanfloat inffloat inffloat inffloat inffloat -1.5float inffloat -nanfloat -7integer -7integer 7integer -7integer 1000000000.0float -nanfloat -8integer -7integer 7.5float -7.5float 1000000000.0float 2.7386127875258float 7integer 8integer 7.5float -1.5float 1000000000.0float -nanfloat -9223372036854775808integer -9223372036854775808integer -9223372036854775808integer -9223372036854775808integer 1000000000.0float 1e+150float 1e+300float 1e+300float 1e+300float -1.5float 1e+300float 5integer 2.0float 2integer 4.0float 3.0float 2integer 3.0float 3.0float nilnil nilnil falseboolean bad argument #1 to 'math.sqrt' (number expected, got table)string falseboolean bad argument #1 to 'math.min' (value expected)string falseboolean t:13: bad argument #1 to 'sqrt' (number expected, got table)string falseboolean attempt to compare number with nilstring falseboolean attempt to compare number with stringstring mine4string 1060675""#,
    );
}

/// `#t` against C Lua after random writes (EQ-1): integer keys in and past the array part,
/// holes, nils, appends, `table.insert`/`remove` at random positions, float keys, and
/// constructors with holes and multiple results. The border C Lua picks depends on its exact array sizing and
/// border hint (`ltable.c`), which `table.rs` reproduces; every program also runs with the
/// fast loop on and off. `REMEDYLUA_LEN_ITERS` programs (default 150).
#[test]
fn length_matches_c_lua_after_random_writes() {
    let iters: u64 = std::env::var("REMEDYLUA_LEN_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(150);
    // String keys: C Lua places them by a hash seeded at random per state, so where a
    // table with deleted string keys rehashes (and so its border) can differ from run to
    // run in C Lua itself; only with `REMEDYLUA_LEN_STR=1` (expect a few differences).
    let with_str = std::env::var_os("REMEDYLUA_LEN_STR").is_some();
    let mut fails = Vec::new();
    for seed in 0..iters {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut rnd = |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n
        };
        let ctor = ["{}", "{1, 2, 3}", "{1, nil, 3, nil, 5}", "{nil, nil, 3}", "{1, 2, 3, 4, 5, 6, 7, 8}", "{n = 1, 2, 3}", "{(f())}", "{f()}", "{f(), f()}", "{x = 1, y = 2}", "{[1] = 1, [2] = 2, [4] = 4}", "{[3] = 3, 1, 2, f()}", "{1, 2, [7] = 7, [-1] = 0}"];
        let mut p = String::from("local function f() return 1, nil, 3, nil end\nlocal r = {}\n");
        let c = ctor[rnd(ctor.len() as u64) as usize];
        let c = if !with_str && c.contains(" = ") && !c.contains('[') { "{}" } else { c };
        p.push_str(&format!("local t = {c}\n"));
        for _ in 0..40 + rnd(60) {
            let k = match rnd(10) {
                0..=4 => (1 + rnd(20)).to_string(),
                5 | 6 => (1 + rnd(70)).to_string(),
                7 => format!("{}.0", 1 + rnd(20)),
                8 => format!("-{}", rnd(5)),
                _ => format!("{}", 100 + rnd(200)),
            };
            let line = match rnd(12) {
                0..=3 => format!("t[{k}] = {}", rnd(100)),
                4 | 5 => format!("t[{k}] = nil"),
                6 => format!("t[#t + 1] = {}", rnd(9)),
                7 => format!("table.insert(t, {})", rnd(9)),
                8 => format!("table.insert(t, math.min(#t + 1, {}), 7)", 1 + rnd(30)),
                9 => "if #t > 0 then table.remove(t) end".to_owned(),
                10 => format!("if #t > 0 then table.remove(t, math.min(#t, {})) end", 1 + rnd(30)),
                _ if !with_str => "t[1] = 1".to_owned(),
                _ => format!("t.s{} = {}", rnd(6), if rnd(3) == 0 { "nil".to_owned() } else { rnd(9).to_string() }),
            };
            p.push_str(&line);
            p.push_str(" r[#r + 1] = #t\n");
        }
        p.push_str("return table.concat(r, ',')\n");
        let ours = common::remedy(&p, 50_000_000);
        let theirs = common::oracle(&p, 50_000_000);
        if theirs != ours {
            // The shortest prefix that still differs, for the report.
            let lines: Vec<&str> = p.lines().collect();
            let body = &lines[..lines.len() - 1];
            let prog = |n: usize| format!("{}\nreturn table.concat(r, ',')\n", body[..n].join("\n"));
            let differs = |q: &str| common::remedy(q, 50_000_000) != common::oracle(q, 50_000_000);
            let n = (3..=body.len()).find(|&n| differs(&prog(n))).unwrap_or(body.len());
            // Then drop single operations while it still differs.
            let mut keep: Vec<&str> = body[..n].to_vec();
            let mut i = 3;
            while i < keep.len() {
                let mut t = keep.clone();
                t.remove(i);
                if differs(&format!("{}\nreturn table.concat(r, ',')\n", t.join("\n"))) {
                    keep = t;
                } else {
                    i += 1;
                }
            }
            let q = format!("{}\nreturn table.concat(r, ',')\n", keep.join("\n"));
            fails.push(format!("seed {seed}:\n{q}ours:  {:?}\nC Lua: {:?}", common::remedy(&q, 50_000_000), common::oracle(&q, 50_000_000)));
        }
    }
    assert!(fails.is_empty(), "{} of {iters} differ from C Lua; first:\n{}", fails.len(), fails[0]);
}
