//! Speed, the call path: the paths the speed work changed, checked against
//! C Lua with the fast loop on and off (same results, fuel and memory: `common::remedy`).

use crate::common::{oracle, remedy};

fn same_as_c(program: &str) {
    let ours = remedy(program, 50_000_000);
    let theirs = oracle(program, 50_000_000);
    assert_eq!(ours, theirs, "differs from C Lua:\n{program}");
}

/// `new_closure` copies a prototype's upvalue descriptions into a 16-entry buffer on the
/// stack, and reads the enclosing closure's upvalues in place instead of cloning them:
/// closures with up to and past 16 upvalues, from registers (fresh and already open,
/// shared between sibling closures) and from the enclosing function's upvalues, in every
/// order, keep their values and share them as in C Lua.
#[test]
fn closures_of_every_upvalue_count_and_source() {
    let mut src = String::from("local log = {}\n");
    for n in [0usize, 1, 15, 16, 17, 40] {
        // n locals of an outer function; a middle function captures half of them as its
        // own upvalues and declares the rest; the inner closure captures all n, some from
        // the middle function's registers, some through its upvalues.
        let outer: Vec<String> = (0..n).map(|i| format!("u{i}")).collect();
        let decl = if n == 0 { String::new() } else { format!("local {} = {}", outer.join(", "), (0..n).map(|i| (i * 3).to_string()).collect::<Vec<_>>().join(", ")) };
        let captured: Vec<&String> = outer.iter().step_by(2).collect();
        let fresh: Vec<String> = (0..n).filter(|i| i % 2 == 1).map(|i| format!("u{i}")).collect();
        let touch = captured.iter().map(|u| format!("local _ = {u}")).collect::<Vec<_>>().join("; ");
        let fresh_decl = if fresh.is_empty() { String::new() } else { format!("local {} = {}", fresh.join(", "), fresh.iter().map(|_| "100").collect::<Vec<_>>().join(", ")) };
        let sum = if n == 0 { "0".to_string() } else { outer.join(" + ") };
        let bump = outer.iter().map(|u| format!("{u} = {u} + k")).collect::<Vec<_>>().join("; ");
        src.push_str(&format!(
            "do
  {decl}
  local function middle()
    {touch}
    {fresh_decl}
    local get = function() return {sum} end
    local add = function(k) {bump} end
    return get, add
  end
  local g1, a1 = middle()
  local g2, a2 = middle()
  log[#log + 1] = g1() .. ',' .. g2()
  a1(1); a2(2)
  log[#log + 1] = g1() .. ',' .. g2()
  for i = 1, 50 do local g, a = middle(); a(i); log[#log + 1] = g() end
end
"
        ));
    }
    src.push_str("return table.concat(log, ' ')");
    same_as_c(&src);
}


/// A call between a function that needs more than one narrow register window and one
/// that does not is made on the full path (the fast loop only calls functions of its own
/// width): results, fuel and memory are the same either way.
#[test]
fn calls_between_wide_and_narrow_functions() {
    let ones = vec!["1"; 300].join(", ");
    let src = format!(
        "local function narrow(x) return x + 1 end
         local function wide(k) return narrow(k) + select('#', {ones}) end
         local s = 0
         for i = 1, 20 do s = s + wide(i) + narrow(i) end
         return s"
    );
    // (C Lua refuses a call with 300 arguments: too many registers.)
    let r = remedy(&src, 50_000_000);
    assert_eq!(r, Ok("6460".to_string()), "{r:?}");
}


/// An open upvalue is read and written in place only by its own thread: a coroutine using
/// a local open on the main thread's stack, and the main thread using one open on a
/// suspended coroutine's stack, go through the full path.
#[test]
fn upvalues_open_on_another_thread() {
    same_as_c(
        r"
        local x = 1
        local co = coroutine.wrap(function() for i = 1, 3 do x = x + i; coroutine.yield(x) end end)
        local s = 0
        for i = 1, 3 do s = s + co() + x end
        local get, set
        local c2 = coroutine.create(function()
          local y = 10
          get = function() return y end
          set = function(v) y = v end
          coroutine.yield()
          y = y + 5
          coroutine.yield()
          return y
        end)
        coroutine.resume(c2)
        local a = get(); set(7)
        coroutine.resume(c2)
        local b = get()
        local _, c = coroutine.resume(c2)
        return s, a, b, c, get()
        ",
    );
}


/// The fast loop's `return x` writes the one result from the register window straight to
/// the function's slot when no upvalue needs closing; the rest (several results, nils to
/// add, open upvalues, a vararg frame whose function slot is further down) takes the
/// general copy. Every count wanted (none, one, several, all) and both kinds of frame.
#[test]
fn returns_of_every_shape_reach_the_caller() {
    same_as_c(
        r"
        local log = {}
        local function id(x) return x end
        local function two() return 1, 2 end
        local function none() return end
        local function cap() local v = 5; local g = function() return v end; return v end
        local function capret() local v = {}; return function() return v end end
        local function va(...) local a = ...; return a end
        local function va2(...) return (select('#', ...)) end
        local function deep(n) if n == 0 then return 0 end return n + deep(n - 1) end
        for i = 1, 3 do
          log[#log + 1] = id(i)
          log[#log + 1] = select('#', id(i))
          local a, b, c = id(i)
          log[#log + 1] = tostring(a) .. tostring(b) .. tostring(c)
          id(i)
          log[#log + 1] = select('#', none())
          log[#log + 1] = cap()
          log[#log + 1] = type(capret()())
          log[#log + 1] = va(i, 5, 6)
          log[#log + 1] = va2(1, nil, 3)
          log[#log + 1] = table.concat({ id(1), id(2), two() }, ',')
          log[#log + 1] = deep(100 + i)
        end
        return table.concat(log, ' ')
        ",
    );
}


/// `obj:m()` reads the method through one `__index` class link at the instruction's hint
/// (`SelfK`), falling back to `chain_from` for everything else: a method replaced or
/// removed (a dead key at the hint), an instance field shadowing it, `__index` moved to
/// another table or to a function, the metatable swapped, the class rehashed (the hint
/// moves), two links. Same results as C Lua, and the same fuel on both paths and under
/// every hash seed.
#[test]
fn method_lookups_through_one_class_link() {
    let src = r"
        local A = {}
        A.__index = A
        function A:m() return 'Am' end
        function A:n() return 'An' end
        local B = setmetatable({}, A)
        B.__index = B
        function B:n() return 'Bn' end
        local o = setmetatable({}, B)
        local log = {}
        for i = 1, 30 do
          log[#log + 1] = o:n()
          log[#log + 1] = o:m()
          if i == 5 then function B:n() return 'Bn2' end end
          if i == 8 then B.n = nil end
          if i == 9 then function B:n() return 'Bn3' end end
          if i == 10 then o.n = function() return 'own' end end
          if i == 12 then o.n = nil end
          if i == 15 then B.__index = A end
          if i == 18 then B.__index = function(t, k) return function() return 'fn:' .. k end end end
          if i == 21 then B.__index = B end
          if i == 24 then setmetatable(o, A) end
          if i == 25 then setmetatable(o, B) end
          if i == 26 then for k = 1, 40 do B['pad' .. k] = k end end
        end
        return table.concat(log, ' ')
    ";
    same_as_c(src);
    let mut fuel = Vec::new();
    for seed in 1..=8u64 {
        for fast in [true, false] {
            let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default().hash_seed(seed), remedy_lua::Libs::ALL);
            lua.set_fast_loop(fast);
            lua.exec(src, "=t").unwrap();
            fuel.push(lua.fuel_left());
        }
    }
    assert!(fuel.iter().all(|f| *f == fuel[0]), "fuel differs between seeds or paths: {fuel:?}");
}

/// Every instruction of the fast loop that reads a variable count (`top`: a call's
/// arguments, a tail call's, a math function's, a return's results) after a call that
/// left one, at several counts.
#[test]
fn variable_counts_through_calls_and_returns() {
    same_as_c(
        r"
        local log = {}
        local function many(n) if n == 0 then return end return n, many(n - 1) end
        local function count(...) return select('#', ...) end
        local function sum(a, b, c, d) return (a or 0) + (b or 0) + (c or 0) + (d or 0) end
        local function pass(n) return many(n) end
        local function tail(n) return sum(many(n)) end
        for n = 0, 6 do
          log[#log + 1] = count(many(n))
          log[#log + 1] = sum(many(n))
          log[#log + 1] = n > 0 and math.max(many(n)) or 0
          log[#log + 1] = count(pass(n))
          log[#log + 1] = tail(n)
          log[#log + 1] = table.concat({ many(n) }, ',')
        end
        return table.concat(log, ' ')
        ",
    );
}

/// A native's one result is stored straight into its slot when the stack is long enough
/// already (else `place_rets` grows it, as before): natives returning none, one, two and
/// many results, each count wanted, and calls at the top of a stack grown by recursion.
#[test]
fn native_results_of_every_count() {
    same_as_c(
        r"
        local log = {}
        local s = 'hello'
        local function at_depth(n)
          if n > 0 then return at_depth(n - 1) end
          local a = type(n)
          local b, c = s:byte(2)
          local d = select('#', s:byte(1, 5))
          local e = select('#', math.type(1), rawlen(s), (s:upper()))
          return a, b, tostring(c), d, e, s:sub(2, 3), tostring(nil), select('#', rawset({}, 1, 2))
        end
        for _, n in ipairs({ 0, 1, 10, 200, 3000 }) do
          log[#log + 1] = table.concat({ tostring(select(1, at_depth(n))), select(2, at_depth(n)) }, ',')
          log[#log + 1] = select('#', at_depth(n))
        end
        return table.concat(log, ' ')
        ",
    );
}

/// After the fast loop switches frames and stops at an instruction it cannot run (a
/// native call, a long concatenation, a metamethod), that instruction runs in full
/// without the fast loop being entered again first. Same results as C Lua, and the same
/// result and fuel on both paths when the fuel runs out at any step around those
/// instructions.
#[test]
fn instructions_left_to_the_full_path_after_a_frame_switch() {
    let src = r"
        local mt = { __add = function(a, b) return a.v + b end }
        local function leaf(x) return x end
        local function f(i)
          local s = tostring(leaf(i))
          local t = leaf(s) .. string.rep('x', 40) .. leaf(s)
          local o = setmetatable({ v = leaf(i) }, mt)
          return #t + (o + 1) + select('#', leaf(i), leaf(i))
        end
        local acc = 0
        for i = 1, 60 do acc = acc + f(i) end
        return acc
    ";
    same_as_c(src);
    for fuel in (1..4000u64).step_by(3) {
        // (`remedy` compares both paths itself; the outcome, often out of fuel, is not the point.)
        let _ = remedy(src, fuel);
    }
}

/// A frame's record no longer keeps the end of its registers (it is `base +
/// frame_size`): the three places that read it see the same end. The stack above the
/// running frame is cleared when the collector runs (a weak table sees a returned
/// callee's values go), metamethods and callbacks are placed past the caller's registers,
/// and a suspended coroutine keeps every register.
#[test]
fn the_end_of_a_frames_registers() {
    same_as_c(
        r"
        local log = {}
        local weak = setmetatable({}, { __mode = 'k' })
        local function fill()
          local a, b, c = {}, {}, {}
          weak[a] = 1; weak[b] = 2; weak[c] = 3
          return 0
        end
        fill()
        collectgarbage(); collectgarbage()
        local n = 0
        for _ in pairs(weak) do n = n + 1 end
        log[#log + 1] = n
        local mt = { __index = function(t, k) return k * 2 end, __add = function(a, b) return 7 end }
        local function regs(o)
          local r1, r2, r3, r4, r5, r6, r7, r8 = 1, 2, 3, 4, 5, 6, 7, 8
          return o[5] + r8 + (o + o) + r1 + r7
        end
        log[#log + 1] = regs(setmetatable({}, mt))
        log[#log + 1] = table.concat({ string.gsub('abc', '%w', function(c) local x, y = c, c; return x .. y end) }, ',')
        local co = coroutine.wrap(function()
          local keep = {}
          for i = 1, 100 do keep[i] = { i } end
          coroutine.yield()
          collectgarbage()
          local s = 0
          for i = 1, 100 do s = s + keep[i][1] end
          coroutine.yield(s)
        end)
        co()
        collectgarbage(); collectgarbage()
        log[#log + 1] = co()
        return table.concat(log, ' ')
        ",
    );
}

/// A Lua function called from Rust (a comparator, a `gsub` replacement, a metamethod)
/// returns on the full path's quick branch when another frame is below it; the bottom
/// frame of a coroutine still finishes the coroutine through `finish_return`.
#[test]
fn returns_from_callbacks_into_rust() {
    same_as_c(
        r"
        local log = {}
        local t = {}
        for i = 1, 40 do t[i] = (i * 7) % 41 end
        table.sort(t, function(a, b) return a > b end)
        log[#log + 1] = table.concat(t, ',')
        log[#log + 1] = (string.gsub('abc', '%w', function(c) return c .. c end))
        log[#log + 1] = select('#', string.gsub('abc', '%w', function() end))
        local mt = { __index = function(_, k) return k, 'extra' end, __lt = function() return 1, 2 end }
        local o = setmetatable({}, mt)
        log[#log + 1] = tostring(o.x) .. tostring(o < o)
        local co = coroutine.wrap(function(a) return a, a + 1, a + 2 end)
        log[#log + 1] = select('#', co(5))
        local c2 = coroutine.create(function() return end)
        log[#log + 1] = tostring(coroutine.resume(c2)) .. coroutine.status(c2)
        log[#log + 1] = table.concat({ pcall(function() return 1, 2, 3 end) }, ',', 2)
        local many = function() return 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18 end
        log[#log + 1] = select('#', table.unpack({ many() }))
        return table.concat(log, ' ')
        ",
    );
}

/// A callback from Rust that is a Lua function gets its frame pushed directly; natives,
/// callable tables and values that cannot be called still go through `precall`, and
/// errors in callbacks come out the same.
#[test]
fn callbacks_of_every_kind_from_rust() {
    same_as_c(
        r"
        local log = {}
        local t = { 5, 3, 9, 1 }
        local cmp = setmetatable({}, { __call = function(_, a, b) return a < b end })
        table.sort(t, cmp)
        log[#log + 1] = table.concat(t, ',')
        local o = setmetatable({}, { __index = rawget })
        log[#log + 1] = tostring(o.missing)
        log[#log + 1] = (string.gsub('a1b2', '%d', { ['1'] = 'one' }))
        log[#log + 1] = select(2, pcall(table.sort, { 3, 2, 1 }, function() error('cmp failed') end))
        log[#log + 1] = select(2, pcall(table.sort, { 3, 2, 1 }, 42))
        log[#log + 1] = select(2, pcall(string.gsub, 'ab', '%w', function(c) return {} end))
        local depth = 0
        local function deep(a, b) depth = depth + 1; if depth < 50 then table.sort({ 2, 1 }, deep) end return a < b end
        table.sort({ 2, 1 }, deep)
        log[#log + 1] = depth
        return table.concat(log, ' ')
        ",
    );
}

/// The fast loop returns out of `Lua::run` to the Rust that called (a callback above
/// another frame) and stops there; a coroutine's last frame still returns on the full
/// path. Results of every count, the fuel running out on the way, both paths.
#[test]
fn fast_loop_returns_out_of_a_callback() {
    let src = r"
        local t = {}
        for i = 1, 200 do t[i] = (i * 37) % 101 end
        local calls = 0
        table.sort(t, function(a, b) calls = calls + 1; return a < b end)
        local s = {}
        for _, k in ipairs({ 0, 1, 2, 5 }) do
          s[#s + 1] = select('#', string.gsub('xy', '%w', function(c) return table.unpack({ c, c, c, c, c }, 1, k) end))
        end
        local co = coroutine.wrap(function(...) return ... end)
        local mt = { __index = function(_, k) return k * 2 end }
        local o = setmetatable({}, mt)
        local acc = 0
        for i = 1, 50 do acc = acc + o[i] end
        return calls, t[1], t[200], table.concat(s, ','), select('#', co(1, 2, 3)), acc
    ";
    same_as_c(src);
    for fuel in (1..20_000u64).step_by(37) {
        // (`remedy` compares both paths itself.)
        let _ = remedy(src, fuel);
    }
}

/// A call from the host puts the function and its arguments in place in one pass: every
/// count of arguments arrives, in order, on both paths with the same fuel.
#[test]
fn host_calls_pass_every_count_of_arguments() {
    let mut seen = Vec::new();
    for fast in [true, false] {
        let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default().hash_seed(crate::common::TEST_SEED), remedy_lua::Libs::ALL);
        lua.set_fast_loop(fast);
        lua.exec("function f(...) local s = select('#', ...) for i = 1, s do s = s * 31 + (select(i, ...)) end return s end", "=t").unwrap();
        let mut out = Vec::new();
        for n in [0usize, 1, 2, 3, 20] {
            let args: Vec<remedy_lua::Value> = (0..n).map(|i| remedy_lua::Value::Int(i as i64 + 1)).collect();
            let r = lua.call_global("f", &args).unwrap().unwrap();
            out.push(format!("{:?}", r.first().copied()));
        }
        seen.push((out, lua.fuel_left()));
    }
    assert_eq!(seen[0], seen[1]);
    let expect = |n: i64| (1..=n).fold(n, |s, i| s.wrapping_mul(31).wrapping_add(i));
    let want: Vec<String> = [0, 1, 2, 3, 20].iter().map(|&n| format!("{:?}", Some(remedy_lua::Value::Int(expect(n))))).collect();
    assert_eq!(seen[0].0, want);
}

/// A Lua callback runs in the fast loop straight from `call_nested_at`, and goes on in
/// `Lua::run` only when the fast loop stops before it returns: one that calls a native,
/// one that needs more than 256 registers, one that errors, and the fuel running out
/// inside one, with the same results and fuel on both paths.
#[test]
fn callbacks_the_fast_loop_does_not_finish() {
    let ones = vec!["1"; 300].join(", ");
    let src = format!(
        "local t = {{}}
         for i = 1, 60 do t[i] = (i * 13) % 61 end
         table.sort(t, function(a, b) return a < b end)
         local u = {{}}
         for i = 1, 30 do u[i] = (i * 7) % 31 end
         table.sort(u, function(a, b) return tostring(a) < tostring(b) end)
         local function wide(a, b) return select('#', {ones}) > 0 and a > b end
         local v = {{ 3, 1, 2 }}
         table.sort(v, wide)
         local ok, err = pcall(table.sort, {{ 2, 1, 3 }}, function(a, b) error('in cmp') end)
         return table.concat(t, ','), table.concat(u, ','), table.concat(v, ','), ok, err"
    );
    // (No C Lua: a call with 300 arguments is past its registers.)
    let r = remedy(&src, 50_000_000);
    assert!(r.as_ref().is_ok_and(|s| s.contains("in cmp")), "{r:?}");
    for fuel in (1..30_000u64).step_by(101) {
        // (`remedy` compares both paths itself.)
        let _ = remedy(&src, fuel);
    }
}
