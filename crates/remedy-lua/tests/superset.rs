//! RemedyLua is a strict superset of Lua 5.4: programs C Lua rejects only because of one of
//! its implementation limits run, bounded only by the host's budgets (fuel, memory, call
//! depth, output). Each test checks the result, that nothing panics, and the exact error
//! when a budget is what stops the program.

use remedy_lua::{Error, LimitKind, Libs, Limits, Lua};

fn run(src: &str, limits: Limits) -> Result<Vec<String>, Error> {
    let mut lua = Lua::new(limits, Libs::ALL);
    let v = lua.exec(src, "=t")?;
    Ok(v.iter().map(|x| super::common::show(&lua, *x)).collect())
}

fn big() -> Limits {
    Limits::default().fuel(1_000_000_000).memory(1 << 30)
}

#[test]
fn unpack_of_two_million_values() {
    let src = "local t = {} for i = 1, 2000000 do t[i] = i end \
               return select('#', table.unpack(t)), select(2000000, table.unpack(t))";
    assert_eq!(run(src, big()).unwrap(), ["2000000", "2000000"]);
    // Under a small cap the stack is memory like any other: an exact memory error.
    let small = Limits::default().fuel(1_000_000_000).memory(40_000_000);
    assert_eq!(run(src, small), Err(Error::Limit(LimitKind::Memory)));
}

/// C Lua's `table.unpack({}, 1, 1e8)` raises "too many results to unpack"; here the only
/// limit is the memory budget, which stops the script.
#[test]
fn huge_unpack_is_a_memory_limit() {
    let r = run("return select('#', table.unpack({}, 1, 1e8))", Limits::default().fuel(u64::MAX >> 1));
    assert_eq!(r, Err(Error::Limit(LimitKind::Memory)));
}

#[test]
fn unpack_and_byte_of_large_counts() {
    for n in [100_000, 999_999] {
        let src = format!("local t = {{}} for i = 1, {n} do t[i] = i end return select('#', table.unpack(t))");
        assert_eq!(run(&src, big()).unwrap(), [n.to_string()]);
    }
    let src = "local s = string.rep('a', 500000) local n = select('#', s:byte(1, -1)) return n, (select(500000, s:byte(1, -1)))";
    assert_eq!(run(src, big()).unwrap(), ["500000", "97"]);
}

#[test]
fn deep_recursion_with_varargs() {
    let src = "local function f(n, ...) if n == 0 then return select('#', ...) end return f(n - 1, n, ...) end \
               return f(1500)";
    assert_eq!(run(src, big()).unwrap(), ["1500"]);
}

#[test]
fn coroutines_yield_many_values() {
    let src = "local t = {} for i = 1, 300000 do t[i] = i end \
               local co = coroutine.wrap(function(...) local n = select('#', ...) coroutine.yield(table.unpack(t)) return n end) \
               local first = select('#', co(table.unpack(t))) return first, co()";
    assert_eq!(run(src, big()).unwrap(), ["300000", "300000"]);
}

#[test]
fn table_literal_with_ten_thousand_items() {
    let items: Vec<String> = (1..=10_000).map(|i| i.to_string()).collect();
    let src = format!("local t = {{{}}} return #t, t[10000], t[5000]", items.join(","));
    assert_eq!(run(&src, big()).unwrap(), ["10000", "10000", "5000"]);
}

#[test]
fn calls_with_hundreds_of_arguments() {
    for n in [250, 300, 1000] {
        let args: Vec<String> = (1..=n).map(|i| i.to_string()).collect();
        let src = format!(
            "local function f(...) return select('#', ...), (select({n}, ...)) end return f({})",
            args.join(",")
        );
        assert_eq!(run(&src, big()).unwrap(), [n.to_string(), n.to_string()], "{n} arguments");
        // A fixed-parameter function too.
        let params: Vec<String> = (1..=n).map(|i| format!("p{i}")).collect();
        let src = format!("local function g({}) return p1 + p{n} end return g({})", params.join(","), args.join(","));
        assert_eq!(run(&src, big()).unwrap(), [(n + 1).to_string()], "{n} parameters");
    }
}

#[test]
fn a_thousand_locals() {
    let mut src = String::new();
    for i in 0..1000 {
        src.push_str(&format!("local v{i} = {i}\n"));
    }
    src.push_str("local s = 0\n");
    for i in 0..1000 {
        src.push_str(&format!("s = s + v{i}\n"));
    }
    // Closures capturing high locals, and a loop over them.
    src.push_str("local f = function() v999 = v999 + 1 return v999 + v0 end\n");
    src.push_str("for i = 1, 3 do s = s + i end\n");
    src.push_str("return s, f(), v999, select('#', v1, v500, v998)");
    assert_eq!(run(&src, big()).unwrap(), ["499506", "1000", "1000", "3"]);
}

#[test]
fn ten_thousand_deep_nesting() {
    let n = 10_000;
    let parens = format!("return {}1{}", "(".repeat(n), ")".repeat(n));
    assert_eq!(run(&parens, big()).unwrap(), ["1"]);
    let blocks = format!("local x = 0 {} x = 7 {} return x", "do ".repeat(n), "end ".repeat(n));
    assert_eq!(run(&blocks, big()).unwrap(), ["7"]);
    let funcs = format!("return {} 5 {}", "(function() return ".repeat(n), "end)()".repeat(n));
    assert_eq!(run(&funcs, big()).unwrap(), ["5"]);
    let tables = format!("local t = {}1{} return #t", "{".repeat(n), "}".repeat(n));
    assert_eq!(run(&tables, big()).unwrap(), ["1"]);
    // Nesting is memory: under a small cap it is an exact memory error, not a crash.
    let small = Limits::default().memory(2 << 20);
    let deep = format!("return {}1{}", "(".repeat(200_000), ")".repeat(200_000));
    assert_eq!(run(&deep, small).map_err(|e| matches!(e, Error::Limit(LimitKind::Memory))), Err(true));
}

/// A program nested `n` deep in one way, and what it returns.
fn nested(kind: &str, n: usize) -> (String, &'static str) {
    match kind {
        "parens" => (format!("return {}1{}", "(".repeat(n), ")".repeat(n)), "1"),
        "blocks" => (format!("local x = 0 {} x = 7 {} return x", "do ".repeat(n), "end ".repeat(n)), "7"),
        // Nested function expressions (each returns the next).
        "functions" => (format!("local f = {} 5 {} return type(f)", "function() return ".repeat(n), " end".repeat(n)), "function"),
        // Each called as it is made: a chain of tail calls.
        "called functions" => (format!("return {} 5 {}", "(function() return ".repeat(n), "end)()".repeat(n)), "5"),
        "calls" => (format!("local k = 0 local function f() k = k + 1 return f end f{} return k", "()".repeat(n)), "count"),
        "methods" => (format!("local o = {{k = 0}} function o:m() self.k = self.k + 1 return self end o{} return o.k", ":m()".repeat(n)), "count"),
        // Left-associative: a left-deep tree.
        "sums" => (format!("local x = 1 return x{}", " + x".repeat(n - 1)), "count"),
        "ands" => (format!("local a = 1 return a{}", " and a".repeat(n)), "1"),
        "ors" => (format!("local a = false return a{} or 1", " or a".repeat(n)), "1"),
        // Right-associative: a right-deep tree (constants fold as it is parsed).
        "powers" => (format!("return 2{}", " ^ 1".repeat(n)), "2.0"),
        "elseifs" => {
            let mut s = format!("local x = {} if x == 0 then return 0", n - 1);
            for i in 1..n {
                s.push_str(&format!(" elseif x == {i} then return {i}"));
            }
            (s + " end", "count-1")
        }
        _ => panic!("{kind}"),
    }
}

const SHAPES: &[&str] =
    &["parens", "blocks", "functions", "called functions", "calls", "methods", "sums", "ands", "ors", "powers", "elseifs"];

fn expect(e: &str, n: usize) -> String {
    match e {
        "count" => n.to_string(),
        "count-1" => (n - 1).to_string(),
        _ => e.to_owned(),
    }
}

/// Nesting is bounded by memory alone: 100 000 deep, every shape compiles and runs (the
/// front end does not recurse), and under a small cap stops with an exact memory error.
#[test]
fn hundred_thousand_deep_nesting() {
    let huge = Limits::default().fuel(1_000_000_000).memory(4 << 30);
    for n in [100_000] {
        for kind in SHAPES {
            let (src, e) = nested(kind, n);
            let r = run(&src, huge);
            assert_eq!(r.as_deref(), Ok(&[expect(e, n)][..]), "{kind} x{n}");
        }
    }
    let small = Limits::default().memory(16 << 20);
    for kind in SHAPES {
        let (src, _) = nested(kind, 100_000);
        assert_eq!(run(&src, small), Err(Error::Limit(LimitKind::Memory)), "{kind}");
    }
}

/// Under every cap from tiny to enough, a deep program either runs or stops with the
/// exact memory error (whichever of tokens, parser stack, compiler stack or prototypes
/// runs out first): never another error, never a crash.
#[test]
fn deep_nesting_under_memory_caps() {
    for kind in ["parens", "ands", "calls", "functions", "blocks"] {
        let (src, e) = nested(kind, 50_000);
        let mut outcomes = (0, 0);
        for mb in [1, 2, 4, 6, 8, 10, 12, 14, 16, 20, 24, 32, 48, 64, 128] {
            match run(&src, Limits::default().fuel(1_000_000_000).memory(mb << 20)) {
                Ok(v) => {
                    assert_eq!(v, [expect(e, 50_000)], "{kind} {mb} MB");
                    outcomes.0 += 1;
                }
                Err(Error::Limit(LimitKind::Memory)) => outcomes.1 += 1,
                Err(other) => panic!("{kind} {mb} MB: {other:?}"),
            }
        }
        assert!(outcomes.0 > 0 && outcomes.1 > 0, "{kind}: {outcomes:?}");
    }
}

/// Shapes that hold a register a level (C Lua stops at 255; 16-bit operands at 32 767):
/// 100 000 deep they run (registers past one window are reached through `Window` and
/// `MoveAbs`), and under a small cap stop with an exact memory error.
#[test]
fn nesting_that_takes_registers() {
    type Shape = (&'static str, fn(usize) -> String, &'static str);
    let shapes: [Shape; 6] = [
        ("tables", |n: usize| format!("local t = {}1{} return #t", "{".repeat(n), "}".repeat(n)), "1"),
        ("concat", |n: usize| format!("local s = 'a' return #(s{})", " .. s".repeat(n - 1)), "count"),
        ("nots", |n: usize| format!("local x = 1 return {}x", "not ".repeat(n)), "true"),
        ("right powers", |n: usize| format!("local x = 1 return x{}", " ^ x".repeat(n - 1)), "1.0"),
        ("args", |n: usize| format!("local function f(x) return x end return {}1{}", "f(".repeat(n), ")".repeat(n)), "1"),
        ("fields", |n: usize| format!("local t = {{}} t.t = t return t{} == t", ".t".repeat(n)), "true"),
    ];
    for (kind, make, e) in shapes {
        for n in [20_000, 100_000] {
            assert_eq!(run(&make(n), big()).as_deref(), Ok(&[expect(e, n)][..]), "{kind} x{n}");
        }
        let small = Limits::default().fuel(1_000_000_000).memory(4 << 20);
        assert_eq!(run(&make(100_000), small), Err(Error::Limit(LimitKind::Memory)), "{kind}");
    }
}

/// The front end takes no native stack per level: parse, compile and drop of programs
/// 100 000 deep on a thread with a 64 KiB stack.
#[test]
fn deep_programs_on_a_small_stack() {
    let _heavy = super::common::memory_heavy();
    let srcs: Vec<String> = SHAPES.iter().map(|k| nested(k, 100_000).0).collect();
    let n = 100_000;
    // Shapes past the register cap: compiled until the error, then dropped.
    let past_cap = [
        format!("local function f(x) return x end return {}1{}", "f(".repeat(n), ")".repeat(n)),
        format!("local t = {}1{}", "{".repeat(n), "}".repeat(n)),
        format!("local x = 1 return {}x", "not ".repeat(n)),
        format!("local t = {{}} return t{}", ".t".repeat(n)),
        format!("local x = 1 return x{}", " ^ x".repeat(n)),
        format!("local o = {{}} function o:m(x) return x end return {}1{}", "o:m(".repeat(n), ")".repeat(n)),
        format!("local t = {}1{}", "{x = ".repeat(n), "}".repeat(n)),
        format!("local t = {{}} return {}1{}", "t[".repeat(n), "]".repeat(n)),
        format!("local a = 1 if {}a{} then return 1 end", "(a and ".repeat(n), ")".repeat(n)),
        format!("local a = 1 return {}a{}", "(a == ".repeat(n), ")".repeat(n)),
        format!("local a = 1 while {}a{} do break end", "(a or ".repeat(n), ")".repeat(n)),
        format!("local a = 1 return {}a", "- ".repeat(n)),
        format!("local a = 'x' return {}a", "#".repeat(n)),
        format!("local a = 1 return {}a{}", "(a + ".repeat(n), ")".repeat(n)),
        format!("local s = 'a' return {}s{}", "(s .. ".repeat(n), ")".repeat(n)),
        format!("{}{}", "while true do ".repeat(n), "break end ".repeat(n)),
        format!("{}{}", "repeat ".repeat(n), "until true ".repeat(n)),
        format!("{}{}", "for i = 1, 1 do ".repeat(n), "end ".repeat(n)),
        format!("{}{}", "if true then ".repeat(n), "end ".repeat(n)),
        format!("local a {}", "= a ".repeat(2)),
    ];
    // RemedyLua dialect: nested interpolations and type annotations.
    let rlua = [
        format!("local x = 1 return {}x{}", "`{".repeat(2000), "}`".repeat(2000)),
        format!("local x: {}number{} = 1 return x", "{".repeat(n), "}".repeat(n)),
        format!("local x: {}number = 1 return x", "(number) -> ".repeat(n)),
        format!("local x: number{} = 1 return x", "?".repeat(n)),
    ];
    let t = std::thread::Builder::new()
        .stack_size(64 << 10)
        .spawn(move || {
            let mut lua = Lua::new(Limits::default().memory(2 << 30), Libs::NONE);
            for src in &srcs {
                // Parse, compile (and the tree dropped): the chunk is a value.
                lua.load(src, "=deep").unwrap();
                // Parse and drop only.
                assert!(remedy_lua::check(src.as_bytes(), "deep.lua").is_empty());
            }
            for src in &past_cap {
                // Runs or stops with a syntax error (the register cap): never a crash.
                assert!(matches!(lua.load(src, "=deep"), Ok(_) | Err(Error::Syntax(_))), "{}", &src[..60.min(src.len())]);
            }
            for src in &rlua {
                let r = lua.load(src, "=deep.rlua");
                assert!(matches!(r, Ok(_) | Err(Error::Syntax(_))), "{}: {r:?}", &src[..60.min(src.len())]);
                assert!(remedy_lua::check(src.as_bytes(), "deep.rlua").len() <= 1);
            }
            // An error deep inside drops the partial tree too.
            let bad = format!("return {}1", "(".repeat(100_000));
            assert!(matches!(lua.load(&bad, "=deep"), Err(Error::Syntax(_))));
            let bad = format!("local x = {}", "{".repeat(100_000));
            assert!(matches!(lua.load(&bad, "=deep"), Err(Error::Syntax(_))));
        })
        .unwrap();
    t.join().unwrap();
}

/// Functions past 255 registers run in the wide interpreter loop: calls between narrow
/// and wide functions, closures over high registers, loops and errors there behave the
/// same with the fast loop on and off.
#[test]
fn wide_functions_mix_with_narrow_ones() {
    let mut src = String::from("local function narrow(a, b) return a * b end\nlocal function wide()\n");
    for i in 0..400 {
        src.push_str(&format!("local v{i} = {i}\n"));
    }
    src.push_str(
        "local s = 0\nfor i = 1, 50 do s = s + narrow(v399, i) + v0 end\n\
         local get = function() return v398 end\n\
         local ok, e = pcall(function() return v397 + {} end)\n\
         return s, get(), ok, e\nend\nreturn wide()",
    );
    let prog = src.as_str();
    let fast = super::common::remedy_measured(prog, 100_000_000, true);
    assert_eq!(fast, super::common::remedy_measured(prog, 100_000_000, false));
    assert!(fast.0.as_ref().is_ok_and(|s| s.contains("508725") && s.contains("398") && s.contains("attempt to perform arithmetic")), "{fast:?}");
}

/// Every kind of expression evaluated 40 000 registers up (inside 40 000 nested table
/// constructors, so past one window): the same results as at the bottom, fast loop on or
/// off. Comparisons, logic, closures over low locals, upvalue writes, varargs, method
/// calls, concatenation, table constructors, errors and their messages.
#[test]
fn expressions_far_up_the_registers() {
    let cases = [
        ("x == 5", "true"),
        ("x < 3 and 'a' or 'b'", "b"),
        ("not x", "false"),
        ("(function() return x + y.k end)()", "7"),
        ("(function() x = x + 1 return x end)()", "6"),
        ("select('#', g(1, 2, 3))", "4"),
        ("#{1, 2, 3, x, g(7, 8)}", "7"),
        ("('abc'):upper() .. x .. y.k", "ABC52"),
        ("y[x - 3] or -x", "-5"),
        ("select(2, pcall(function() return x + {} end))", "attempt to perform arithmetic on a table value"),
        ("select(2, pcall(error, 'e' .. x, 0))", "e5"),
    ];
    let n = 40_000;
    for (e, want) in cases {
        let src = format!(
            "local x, y = 5, {{k = 2}} local function g(...) return select('#', ...), ... end \
             local t = {}{e}{} local v = t for i = 1, {n} do v = v[1] end return v",
            "{".repeat(n),
            "}".repeat(n)
        );
        for fast in [true, false] {
            let r = super::common::remedy_measured(&src, 100_000_000, fast).0.unwrap_or_else(|e| e);
            // (Strings come quoted; a message after its chunk and line.)
            let got = r.trim_matches('"');
            assert!(got == want || got.ends_with(&format!(": {want}")), "{e} (fast loop {fast}): {r}");
        }
    }
}

/// Locals past one window (70 000): read, written, captured, looped over and closed. No
/// register cap; the stack is memory.
#[test]
fn seventy_thousand_locals() {
    let mut src = String::new();
    for i in 0..70_000 {
        src.push_str(&format!("local v{i} = {i}\n"));
    }
    src.push_str("v0 = v69999 + v40000\nlocal f = function() return v60000 + v1 end\nlocal s = 0\n");
    src.push_str("for i = 1, 3 do s = s + i + v65000 end\n");
    src.push_str("do local t <close> = setmetatable({}, {__close = function() s = s + 1 end}) end\n");
    src.push_str("return v0, f(), s, v69999 == 69999 and v0 > v1\n");
    for fast in [true, false] {
        let r = super::common::remedy_measured(&src, 100_000_000, fast);
        assert_eq!(r.0.as_deref(), Ok("109999 60001 195007 true"), "fast loop {fast}");
    }
}
