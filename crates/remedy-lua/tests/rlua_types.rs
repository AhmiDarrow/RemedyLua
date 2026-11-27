//! The type checker (`remedy_lua::check` on `.rlua` files; `docs/reference.md`, "The type
//! checker"):
//!
//! - typed programs get exactly their expected diagnostics (message, line, range), and
//!   well-typed ones none;
//! - plain Lua gets none: every corpus and spec program, the benchmarks and generated
//!   programs, checked as `.rlua`;
//! - annotations never change behaviour: each typed program runs exactly as its plain
//!   twin (results, errors, fuel; fast loop on and off), and the generator's `.rlua`
//!   programs as their plain twins;
//! - the checker is bounded: 100,000-deep and 100,000-wide inputs on the 64 KiB small
//!   stack, and a fuzz of `check` on mutated `.rlua` programs (no panic, no overflow,
//!   bounded time).

use std::time::{Duration, Instant};

use remedy_lua::{Libs, Limits, Lua, Pos, Severity};

use crate::sweep1_fuzz::grammar::Gen;
use crate::sweep1_fuzz::harness;
use crate::sweep1_fuzz::mutate;
use crate::sweep1_fuzz::rng::Rng;

/// `check`'s diagnostics as `line: message`.
fn diags(src: &str) -> Vec<String> {
    remedy_lua::check(src.as_bytes(), "t.rlua")
        .iter()
        .map(|d| format!("{}: {}", d.pos.line, d.message))
        .collect()
}

/// Typed programs and their diagnostics, in order.
const CASES: &[(&str, &[&str])] = &[
    // Locals.
    ("local x: number = 'a'", &["1: local 'x': expected number, got \"a\""]),
    ("local x: integer = 1.5", &["1: local 'x': expected integer, got number"]),
    ("local x: integer = 3 / 1", &["1: local 'x': expected integer, got number"]),
    ("local x: integer = 7 // 2 local y: integer = 2 ^ 2", &["1: local 'y': expected integer, got number"]),
    ("local x: number = nil", &["1: local 'x': expected number, got nil"]),
    ("local x: number?, y: string = nil", &["1: local 'y': expected string, got nil"]),
    ("local a: number, b: string = (function() return 1, 2 end)()", &[]),
    ("local x: number = 1\nx = 'two'", &["2: local 'x': expected number, got \"two\""]),
    ("local x: number = 1\nx += 1\nx ..= 'a'", &["3: local 'x': expected number, got string"]),
    ("local x: integer = 1\nx /= 2", &["2: local 'x': expected integer, got number"]),
    (
        "local s: \"left\" | \"right\" = \"left\"\ns = \"up\"",
        &["2: local 's': expected \"left\" | \"right\", got \"up\""],
    ),
    ("local b: boolean = 1 < 2 local c: boolean = 'x'", &["1: local 'c': expected boolean, got \"x\""]),
    ("local u: Foo = 1", &["1: unknown type 'Foo'"]),
    ("local h: host.Widget = 1", &[]),
    // Tables.
    (
        "local p: {x: number, y: number} = {x = 1}",
        &["1: local 'p': expected {x: number, y: number}, got {x: integer}"],
    ),
    ("local p: {x: number, y: number?} = {x = 1}", &[]),
    (
        "local p: {x: number} = {x = 1, z = 2}",
        &["1: local 'p': expected {x: number}, got {x: integer, z: integer}"],
    ),
    (
        "local p: {x: number} = {x = 1}\np.x = 'a'\np.z = 1\nreturn p.w",
        &[
            "2: field 'x': expected number, got \"a\"",
            "3: no field 'z' in {x: number}",
            "4: no field 'w' in {x: number}",
        ],
    ),
    ("local a: {number} = {1, 2.5, 'x'}", &["1: local 'a': expected {number}, got {integer, number, \"x\"}"]),
    (
        "local a: {number} = {1, 2}\na[1] = 'x'\na[2] = nil\na.n = 1",
        &["2: array element: expected number?, got \"x\"", "4: field 'n' is not a valid index for {number}"],
    ),
    (
        "local m: {[string]: boolean} = {a = true, b = 1}",
        &["1: local 'm': expected {[string]: boolean}, got {a: true, b: integer}"],
    ),
    (
        "local m: {[string]: number} = {}\nm[1] = 2\nm.k = 'v'",
        &[
            "2: this key is not a valid key for {[string]: number}",
            "3: map value: expected number?, got \"v\"",
        ],
    ),
    ("local a: {number} = {1}\nlocal b: {integer} = a", &["2: local 'b': expected {integer}, got {number}"]),
    ("local a: {integer} = {1}\nlocal b: {number} = a", &["2: local 'b': expected {number}, got {integer}"]),
    ("local a: {{number}} = {{1}, {2, 3}, {}}", &[]),
    ("local a: {{number}} = {{1}, {'x'}}", &["1: local 'a': expected {{number}}, got {{integer}, {\"x\"}}"]),
    ("local a: {number} = {1}\nlocal n: number = a[1]", &["2: local 'n': expected number, got number?"]),
    ("local a: {number} = {1}\nlocal n: number = a[1] or 0", &[]),
    // Functions.
    (
        "local function f(a: number, b: string?): string\n  return a\nend",
        &["2: return value #1: expected string, got number"],
    ),
    (
        "local function f(a: number) end\nf('a')\nf(1, 2)\nf()",
        &[
            "2: argument #1 to 'f': expected number, got \"a\"",
            "3: too many arguments to 'f': expected 1, got 2",
            "4: missing argument #1 to 'f' (expected number)",
        ],
    ),
    ("local function f(a: number?) end\nf()", &[]),
    (
        "local function f(...: number) end\nf(1, 2, 'x')",
        &["2: argument #3 to 'f': expected number, got \"x\""],
    ),
    ("local function f(): (number, string) return 1 end", &["1: missing return value #2 (expected string)"]),
    ("local function f(): () return 1 end", &["1: too many return values: expected 0, got 1"]),
    (
        "local function f(): number\n  if math.random() > 0.5 then return 1 end\nend",
        &["3: function may end without returning number"],
    ),
    ("local function f(): number?\n  if math.random() > 0.5 then return 1 end\nend", &[]),
    ("local function f(x: number): number\n  if x > 0 then return 1 else error('neg') end\nend", &[]),
    ("local function f(): number\n  while true do end\nend", &[]),
    (
        "local function f(): number\n  while true do break end\nend",
        &["3: function may end without returning number"],
    ),
    (
        "local function f(): number\n  goto done\n  ::done::\nend",
        &["4: function may end without returning number"],
    ),
    (
        "local function f(a: number): number return a end\nlocal s: string = f(1)",
        &["2: local 's': expected string, got number"],
    ),
    (
        "local function f(): (number, string) return 1, 'a' end\nlocal a: number, b: number = f()",
        &["2: local 'b': expected number, got string"],
    ),
    (
        "local function f(a: number) end\nf = function(a: string) end",
        &["2: local 'f': expected (number) -> any..., got (string) -> any..."],
    ),
    ("local function f(a: number) end\nf = function(a: number?) end", &[]),
    ("local f: (number) -> string = function(n) return tostring(n) end", &[]),
    (
        "local f: (number) -> string = function(n: number): number return n end",
        &["1: local 'f': expected (number) -> string, got (number) -> number"],
    ),
    ("local f: (number, string) -> () = function(n: number) end", &[]),
    (
        "local f: (number) -> () = function(n: number, s: string) end",
        &["1: local 'f': expected (number) -> (), got (number, string) -> any..."],
    ),
    (
        "local function apply(f: (number) -> number, x: number): number return f(x) end\napply(function(n: string): number return 1 end, 2)",
        &["2: argument #1 to 'apply': expected (number) -> number, got (string) -> number"],
    ),
    // Operators on typed values.
    (
        "local s: string? = nil\nlocal n = s .. 'x'",
        &["2: attempt to concatenate a value that may be nil (local 's' is string?)"],
    ),
    (
        "local t: {number}? = nil\nreturn #t",
        &["2: attempt to get length of a value that may be nil (local 't' is {number}?)"],
    ),
    ("local x: number = 1\nreturn x.y", &["2: attempt to index a number value (local 'x')"]),
    ("local x: number = 1\nreturn x()", &["2: attempt to call a number value (local 'x')"]),
    (
        "local b: boolean = true\nreturn b + 1",
        &["2: attempt to perform arithmetic on a boolean value (local 'b')"],
    ),
    (
        "local x: number = 1\nreturn x + 'abc', x + '10'",
        &["2: attempt to perform arithmetic on a string value (constant 'abc')"],
    ),
    ("local x: number = 1\nreturn x < 'a'", &["2: attempt to compare number with string"]),
    (
        "local x: number? = nil\nreturn x < 1",
        &[
            "2: comparison may fail: attempt to compare nil with number (the operands are number? and integer)",
        ],
    ),
    (
        "local t: {x: number}? = nil\nreturn t.x",
        &["2: attempt to index a value that may be nil (local 't' is {x: number}?)"],
    ),
    ("local t: table = {}\nreturn t + 1, t.x, t(), #t", &[]),
    ("local x: any = nil\nreturn x.y + x()", &[]),
    ("local f: function = print\nreturn f.x", &["2: attempt to index a function value (local 'f')"]),
    // Narrowing.
    ("local t: {x: number}? = nil\nif t then return t.x end", &[]),
    ("local t: {x: number}? = nil\nif t ~= nil then return t.x end", &[]),
    ("local t: {x: number}? = nil\nif nil == t then return 0 else return t.x end", &[]),
    ("local t: {x: number}? = nil\nreturn t and t.x", &[]),
    ("local t: {x: number}? = nil\nreturn not t or t.x", &[]),
    ("local t: {x: number}? = nil\nif not t then return end\nreturn t.x", &[]),
    ("local t: {x: number}? = nil\nif t == nil then error('none') end\nreturn t.x", &[]),
    ("local t: {x: number}? = nil\nassert(t)\nreturn t.x", &[]),
    (
        "local t: {x: number}? = nil\nwhile t do t = nil end\nreturn t.x",
        &["3: attempt to index a value that may be nil (local 't' is {x: number}?)"],
    ),
    (
        "local t: {x: number}? = nil\nif t then return 1 end\nreturn t.x",
        &["3: attempt to index a nil value (local 't')"],
    ),
    (
        "local t: {x: number}? = nil\nif not t then goto skip end\nlocal y = t.x\n::skip::",
        &["3: attempt to index a value that may be nil (local 't' is {x: number}?)"],
    ),
    (
        "local t: {x: number}? = nil\nlocal function f() t = nil end\nif t then f() return t.x end",
        &["3: attempt to index a value that may be nil (local 't' is {x: number}?)"],
    ),
    ("local t: {x: number}? = nil\nif t then local function g() return t.x end end", &[]),
    ("local t: {x: number}? = nil\nlocal v = t or {x = 0}\nreturn v.x", &[]),
    ("local c: boolean = math.random() > 0.5\nlocal n: number = c and 1 or 2", &[]),
    (
        "local c: boolean = math.random() > 0.5\nlocal n: number = c or 2",
        &["2: local 'n': expected number, got true | integer"],
    ),
    // Narrowing of reassigned locals: by assignment, until a loop or label may undo it.
    ("local x: number? = nil\nx = 5\nreturn x + 1", &[]),
    (
        "local x: number? = 5\nx = nil\nreturn x + 1",
        &["3: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    (
        "local x: number? = nil\nif math.random() > 0.5 then x = 1 end\nreturn x + 1",
        &["3: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    (
        "local x: number? = nil\nx = 1\nif math.random() > 0.5 then x = nil end\nif x then return x + 1 end",
        &[],
    ),
    ("local x: number? = nil\nx = 2\nfor i = 1, 3 do local y = x + 1 end", &[]),
    // (`any` on the other side may have the metamethod that makes it work.)
    ("local x: number? = nil\nlocal y = x + (...)", &[]),
    (
        "local x: number? = nil\nx = 2\nfor i = 1, 3 do local y = x + 1 x = nil end",
        &["3: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    (
        "local x: number? = nil\nx = 2\nwhile x < 10 do x = nil end",
        &[
            "3: comparison may fail: attempt to compare nil with number (the operands are number? and integer)",
        ],
    ),
    (
        "local x: number? = nil\nx = 1\n::top::\nlocal y = x + 1\nx = nil\ngoto top",
        &["4: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    (
        "local x: number? = nil\nx = 1\nlocal f = function() return x + 1 end\nx = nil",
        &["3: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    (
        "local x: number? = nil\nlocal function clear() x = nil end\nx = 1\nclear()\nreturn x + 1",
        &["5: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    (
        "local x: number? = nil\nlocal function clear() x = nil end\nif x then return x + 1 end",
        &["3: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    (
        "type N = {next: N?, v: number}\nlocal n: N? = {v = 1}\nlocal s = 0\nwhile n do s = s + n.v n = n.next end",
        &[],
    ),
    (
        "local x: number? = nil\nif x == nil then x = 0 end\nreturn x + 1",
        &["3: attempt to perform arithmetic on a value that may be nil (local 'x' is number?)"],
    ),
    // Aliases and recursion.
    (
        "type Point = {x: number, y: number}\nlocal p: Point = {x = 1, y = 2}\nlocal q: {x: number, y: number} = p",
        &[],
    ),
    (
        "type List = {value: number, next: List?}\nlocal l: List = {value = 1, next = {value = 2, next = nil}}\nlocal n: number = l.next.value",
        &["3: attempt to index a value that may be nil (field 'next' is List?)"],
    ),
    (
        "type List = {value: number, next: List?}\nlocal l: List = {value = 1, next = {value = 'x'}}",
        &["2: local 'l': expected List, got {next: {value: \"x\"}, value: integer}"],
    ),
    ("type A = {a: A?}\ntype B = {a: B?}\nlocal x: A = {}\nlocal y: B = x", &[]),
    ("type T = T", &["1: type 'T' is defined only in terms of itself"]),
    ("type A = B\ntype B = A", &["1: type 'A' is defined only in terms of itself"]),
    ("type number = string", &["1: cannot redefine the built-in type 'number'"]),
    ("type T = number\ntype T = string", &["2: type 'T' is already defined in this block"]),
    ("do type T = number end\nlocal x: T = 1", &["2: unknown type 'T'"]),
    ("local x: T = 1\ntype T = string", &["1: local 'x': expected T, got integer"]),
    // Methods and `self`.
    (
        "type Counter = {n: number, add: (self: Counter, k: number) -> ()}\nlocal c: Counter = {n = 0, add = function(self: Counter, k: number) self.n = self.n + k end}\nc:add(1)\nc:add('x')\nc:sub(1)",
        &["4: argument #1 to method 'add': expected number, got \"x\"", "5: no field 'sub' in Counter"],
    ),
    ("local o: {n: number} = {n = 1}\nfunction o.f() end", &["2: no field 'f' in {n: number}"]),
    (
        "local o: {n: number, f: (number) -> ()} = {n = 1, f = function(x) end}\nfunction o.f(s: string) end",
        &["2: field 'f': expected (number) -> (), got (string) -> any..."],
    ),
    // Loops.
    ("for i: string = 1, 10 do end", &["1: loop variable 'i': expected string, got integer"]),
    ("for i: integer = 1, 2.5 do end", &[]),
    ("for i: integer = 1.0, 2 do end", &["1: loop variable 'i': expected integer, got number"]),
    ("local s: string = 'a'\nfor i = s, 10 do end", &["2: 'for' initial value must be a number"]),
    (
        "local function it(s: any, c: number?): number? if (c or 0) < 3 then return (c or 0) + 1 end end\nfor i: string in it do end\nfor i in it do local n: number = i end",
        &["2: loop variable 'i': expected string, got number"],
    ),
    // Plain code next to typed code stays dynamic.
    ("local x = nil\nreturn x.y, nil + 1, ('a')()", &[]),
    ("local t = {}\nt.x = 1\nlocal n: number = t.y", &[]),
    ("local function f(a) return a.b.c end\nlocal n: number = f(1)", &[]),
    ("local v = 1\nv = 'a'\nlocal s: string = v", &[]),
];

#[test]
fn typed_programs_get_their_diagnostics() {
    let mut failures = Vec::new();
    for (src, want) in CASES {
        let got = diags(src);
        let want: Vec<String> = want.iter().map(|s| (*s).to_owned()).collect();
        if got != want {
            failures.push(format!("{src}\n  want {want:?}\n  got  {got:?}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        CASES.len(),
        failures.join("\n")
    );
}

#[test]
fn diagnostics_have_ranges_and_lua_lines() {
    let d = remedy_lua::check(b"local x: number = 1\nlocal s: string =\n  'a' .. x", "f.rlua");
    assert!(d.is_empty(), "{d:?}");
    let d = remedy_lua::check(b"local x: number = 1\nlocal s: string =\n  x + 2", "f.rlua");
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].severity, Severity::Error);
    assert_eq!((d[0].pos, d[0].end), (Pos { line: 3, col: 3 }, Pos { line: 3, col: 8 }));
    assert_eq!(d[0].lua_message(), "f.rlua:3: local 's': expected string, got number");
    assert_eq!(d[0].to_string(), "f.rlua:3:3: local 's': expected string, got number");
    // An unknown type is marked by its name.
    let d = remedy_lua::check(b"local x: Widget = 1", "f.rlua");
    assert_eq!((d[0].pos, d[0].end), (Pos { line: 1, col: 10 }, Pos { line: 1, col: 16 }));
    // A syntax error is the only diagnostic, as before.
    let d = remedy_lua::check(b"local x: number = 'a'\nlocal = 1", "f.rlua");
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].lua_message(), "f.rlua:2: <name> expected near '='");
    // A `.lua` file has no annotations to check.
    assert!(remedy_lua::check(b"local x = nil return x.y", "f.lua").is_empty());
}

/// Correct typed programs: no diagnostics.
const WELL_TYPED: &[&str] = &[
    "type Vec = {x: number, y: number}
local function add(a: Vec, b: Vec): Vec
  return {x = a.x + b.x, y = a.y + b.y}
end
local function len(v: Vec): number return math.sqrt(v.x * v.x + v.y * v.y) end
local v: Vec = add({x = 1, y = 2}, {x = 3, y = 4})
return len(v), v.x",
    "type Tree = {value: integer, left: Tree?, right: Tree?}
local function sum(t: Tree?): integer
  if t == nil then return 0 end
  return t.value + sum(t.left) + sum(t.right)
end
local t: Tree = {value = 1, left = {value = 2}, right = {value = 3, left = {value = 4}}}
return sum(t)",
    "local names: {string} = {}
for i = 1, 5 do names[#names + 1] = 'n' .. i end
local counts: {[string]: integer} = {}
for _, n in ipairs(names) do counts[n] = (counts[n] or 0) + 1 end
local total: integer = 0
for _, c in pairs(counts) do total += c end
return total, #names",
    "local function map(xs: {number}, f: (number) -> number): {number}
  local out: {number} = {}
  for i = 1, #xs do
    local v = xs[i]
    if v then out[i] = f(v) end
  end
  return out
end
local r = map({1, 2, 3}, function(x: number): number return x * 2 end)
return r[1], r[3]",
    "type Shape = {kind: \"circle\" | \"square\", size: number}
local function area(s: Shape): number
  if s.kind == \"circle\" then return 3 * s.size ^ 2 end
  return s.size * s.size
end
return area({kind = \"circle\", size = 2}), area({kind = \"square\", size = 3})",
    "local function first(...: string): string?
  local a = ...
  return a
end
local s: string? = first('a', 'b')
return s and #s or 0",
    "local function parse(s: string): (integer?, string?)
  local n = tonumber(s)
  if n then return math.tointeger(n), nil end
  return nil, 'not a number: ' .. s
end
local n, err = parse('12')
local m, err2 = parse('x')
return n, err, m, err2",
    "local Account = {}
Account.__index = Account
type Acct = {balance: number}
local function new(b: number): Acct return setmetatable({balance = b}, Account) end
local a: Acct = new(10)
a.balance -= 3
return a.balance",
    "local x: number | string = 1
x = 'one'
local y: number? = nil
y = 2
local z: integer = 1
z += 2
z //= 2
z %= 2
local w: number = z
return x, y, w",
    "local i: integer = 0
while true do
  i += 1
  if i > 10 then break end
  if i % 2 == 0 then continue end
end
repeat local k: number = i until k > 0
return i",
    "local cb: ((string) -> ())? = nil
local function on(f: (string) -> ()) cb = f end
on(function(s) print(s) end)
local f = cb -- `cb` is reassigned (by `on`), so it is not narrowed; `f` is
if f then f('hi') end
local s = `interpolated {1 + 2} {cb}`
return #s > 0",
    "local t: {x: number, y: number} = {x = 1, y = 2}
local u = t
u.x = 3
return t.x + u.y",
    "local co: thread = coroutine.create(function() end)
local ok: boolean = coroutine.resume(co)
local h: userdata? = nil
return ok, h",
    "local function f(): number
  for i = 1, 10 do
    if i == 5 then return i end
  end
  error('unreachable')
end
local function g(x: boolean): string
  if x then return 'y' elseif not x then return 'n' else return '?' end
end
local function h(): integer
  repeat until false
end
return f(), g(true)",
];

#[test]
fn well_typed_programs_get_none_and_run() {
    for src in WELL_TYPED {
        let d = diags(src);
        assert!(d.is_empty(), "{src}\n{d:?}");
        let mut lua =
            remedy_lua::Lua::new(remedy_lua::Limits::default().fuel(10_000_000), remedy_lua::Libs::ALL);
        assert!(lua.exec(src, "=t.rlua").is_ok(), "{src}");
    }
}

/// Every Lua file under `dir`: the whole file, and each `-- case:` in it.
fn lua_files(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut paths: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            lua_files(&p, out);
            continue;
        }
        if p.extension().is_none_or(|x| x != "lua") {
            continue;
        }
        let src = std::fs::read_to_string(&p).unwrap().replace("\r\n", "\n");
        let name = p.display().to_string();
        if src.contains("\n-- case:") {
            for chunk in format!("\n{src}").split("\n-- case:").skip(1) {
                let body = chunk.split_once('\n').map_or("", |x| x.1);
                out.push((body.to_owned(), name.clone()));
            }
        }
        out.push((src, name));
    }
}

#[test]
fn plain_lua_checked_as_rlua_gets_no_type_diagnostics() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut corpus = Vec::new();
    lua_files(&root.join("tests/lua"), &mut corpus);
    lua_files(&root.join("benches/lua"), &mut corpus);
    let files = corpus.len();
    // Generated plain programs too.
    for i in 0..400u64 {
        let mut rng = Rng::new(0x7e57 ^ i.wrapping_mul(0x9e37_79b9));
        let mut g = Gen::new(&mut rng, false);
        let p = g.program(6 + (i % 30) as i32);
        corpus.push((p.l, format!("generated {i}")));
    }
    let (mut checked, mut skipped) = (0, 0);
    for (src, name) in &corpus {
        // Only programs that are valid Lua (some corpus cases are syntax-error tests).
        if !remedy_lua::check(src.as_bytes(), "t.lua").is_empty() {
            skipped += 1;
            continue;
        }
        let d = remedy_lua::check(src.as_bytes(), "t.rlua");
        // The dialect may read a few plain programs differently (backticks and the like):
        // then a syntax error, never a type diagnostic.
        assert!(d.is_empty(), "{name}: {d:?}");
        checked += 1;
    }
    assert!(files > 100 && checked > 450, "{files} files, {checked} checked, {skipped} skipped");
}

/// A typed program and its plain twin: `«...»` marks what the plain one leaves out.
fn twins(marked: &str) -> (String, String) {
    let typed = marked.replace(['«', '»'], "");
    let mut plain = String::new();
    let mut depth = 0;
    for c in marked.chars() {
        match c {
            '«' => depth += 1,
            '»' => depth -= 1,
            _ if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    (typed, plain)
}

/// Typed programs (well- and ill-typed) with their annotations marked.
const MARKED: &[&str] = &[
    "local x«: number» = 1 local y«: string» = 'a' .. x return y",
    "«type P = {x: number, y: number}»\nlocal function add(a«: P», b«: P»)«: P» return {x = a.x + b.x, y = a.y + b.y} end\nlocal p = add({x = 1, y = 2}, {x = 3, y = 4}) return p.x, p.y",
    "local function f(a«: number», ...«: string»)«: (number, string)» return a, select('#', ...) end return f(1, 'a', 'b')",
    // Ill-typed: the checker complains, the program runs as written.
    "local x«: number» = 'not a number' return x .. '!'",
    "local t«: {x: number}?» = nil return pcall(function() return t.x end)",
    "local function f(n«: number»)«: string» return n * 2 end return f(21)",
    "local a«: {integer}» = {1, 2, 3} a[2] = 'two' local s = 0 for i«: integer» = 1, #a do s = s .. a[i] end return s",
    "local function g()«: number» if false then return 1 end end return g()",
    "for k«: string», v«: number» in pairs({a = 1, b = 2}) do end return select('#', pcall(error, 'x'))",
    "local n«: integer» = 0 for i = 1, 100 do if i % 3 == 0 then n = n + i end end return n",
    "local s«: string?» = nil if s then return #s end return `none {s}`",
    "local v«: (number) -> number» = function(x«: number») return x + 1 end return v(1), v(2.5)",
];

/// Runs `src` (loaded by the host, so `load`'s charge per source byte is not counted):
/// its results or error, the fuel the run used and the memory held after, with the fast
/// loop on and off (which must agree).
fn run_compiled(src: &str) -> (Result<Vec<String>, String>, u64, usize) {
    let run = |fast: bool| {
        let fuel = 10_000_000;
        let mut lua = Lua::new(Limits::default().fuel(fuel).hash_seed(crate::common::TEST_SEED), Libs::ALL);
        lua.set_fast_loop(fast);
        let f = lua.load(src, "=t.rlua").unwrap();
        let before = lua.fuel_left();
        let r = lua
            .call(f.value(), &[])
            .map(|v| v.iter().map(|x| crate::common::show(&lua, *x)).collect())
            .map_err(|e| e.to_string());
        let _ = before;
        (r, fuel.saturating_sub(lua.fuel_left()), lua.memory_used())
    };
    let (fast, slow) = (run(true), run(false));
    assert_eq!(fast, slow, "fast loop and full path differ for:\n{src}");
    fast
}

#[test]
fn annotations_never_change_behaviour() {
    for m in MARKED {
        let (typed, plain) = twins(m);
        assert!(
            !plain.contains(':') || plain.contains("':") || plain.contains("::"),
            "a mark is missing in: {plain}"
        );
        // The same results or error, fuel and memory (both as `.rlua`: the same chunk name
        // in messages).
        let (t, p) = (run_compiled(&typed), run_compiled(&plain));
        assert_eq!(t, p, "{typed}\n-- vs --\n{plain}");
        // The plain twin (where it is plain Lua) is what C Lua does.
        if !plain.contains('`') {
            let ours = harness::remedy(plain.as_bytes(), "=t");
            let theirs = harness::oracle(plain.as_bytes());
            assert_eq!(harness::compare(&ours, &theirs), None, "{plain}: {ours:?} vs {theirs:?}");
        }
    }
    // The generator's `.rlua` programs (annotations, compound assignment, `continue`,
    // interpolation) against their plain twins, and `check` on each.
    for i in 0..150u64 {
        let mut rng = Rng::new(0x711e ^ i.wrapping_mul(0x85eb_ca6b));
        let mut g = Gen::new(&mut rng, true);
        let p = g.program(8 + (i % 20) as i32);
        let _ = remedy_lua::check(p.r.as_bytes(), "gen.rlua");
        let ours = harness::remedy(p.r.as_bytes(), "=t.rlua");
        let plain = harness::remedy(p.l.as_bytes(), "=t.rlua");
        assert_eq!((&ours.class, &ours.text), (&plain.class, &plain.text), "{}\n-- plain --\n{}", p.r, p.l);
    }
}

/// `check` on the small stack, within `limit`: its diagnostics.
fn bounded(src: String, limit: Duration) -> Vec<remedy_lua::Diagnostic> {
    let len = src.len();
    let head: String = src.chars().take(60).collect();
    let r = harness::with_timeout(crate::common::SMALL_STACK, move || {
        let t = Instant::now();
        let d = remedy_lua::check(src.as_bytes(), "deep.rlua");
        (d, t.elapsed())
    });
    let Some((d, took)) = r else { panic!("check did not finish ({len} bytes): {head}") };
    assert!(took < limit, "check took {took:?} on {len} bytes: {head}");
    d
}

#[test]
fn check_is_bounded_on_deep_and_wide_inputs() {
    let _heavy = super::common::memory_heavy();
    let n = 100_000;
    let limit = Duration::from_secs(10);
    let msgs = |d: &[remedy_lua::Diagnostic]| d.iter().map(|d| d.message.clone()).collect::<Vec<_>>();
    // (source, how many diagnostics)
    let cases: Vec<(String, usize)> = vec![
        // Deep types against deep values.
        (
            format!(
                "local x: {}number{} = {}1{}",
                "{".repeat(n),
                "}".repeat(n),
                "{".repeat(n),
                "}".repeat(n)
            ),
            0,
        ),
        (
            format!(
                "local x: {}number{} = {}'a'{}",
                "{".repeat(n),
                "}".repeat(n),
                "{".repeat(n),
                "}".repeat(n)
            ),
            1,
        ),
        (format!("local x: {}number = nil", "(number) -> ".repeat(n)), 1),
        (format!("local x: number{} = 1", "?".repeat(n)), 0),
        (format!("local x: {}number{} = {{}}", "{x: ".repeat(n), "}".repeat(n)), 1),
        (format!("local x: {}number{} = nil", "{[string]: ".repeat(n), "}".repeat(n)), 1),
        // Deep and long chains of aliases and unions.
        (
            format!(
                "type T0 = number\n{}local x: T{} = true",
                (1..=n).map(|i| format!("type T{i} = T{} | \"s{i}\"\n", i - 1)).collect::<String>(),
                n
            ),
            1,
        ),
        (
            format!(
                "{}type A0 = number\nlocal x: A{n} = 'a'",
                (1..=n).rev().map(|i| format!("type A{i} = A{}\n", i - 1)).collect::<String>()
            ),
            1,
        ),
        (
            format!(
                "type L = {{next: L?, v: number}}\nlocal l: L = {}nil{}",
                "{v = 1, next = ".repeat(n / 4),
                "}".repeat(n / 4)
            ),
            0,
        ),
        (format!("type L = {{next: L?}}\nlocal l: L = {{}}\nreturn l{}", ".next".repeat(n)), 500),
        // Wide: a union of many literals, a shape of many fields.
        (
            format!(
                "type U = {}\nlocal u: U = 'zz'",
                (0..n).map(|i| format!("'k{i}'")).collect::<Vec<_>>().join(" | ")
            ),
            1,
        ),
        (
            format!(
                "local s: {{{}}} = {{{}}}",
                (0..n).map(|i| format!("f{i}: number")).collect::<Vec<_>>().join(", "),
                (0..n).map(|i| format!("f{i} = {i}")).collect::<Vec<_>>().join(", ")
            ),
            0,
        ),
        // Deep code around typed values.
        (format!("local x: number = 1\n{}local s: string = x\n{}", "do ".repeat(n), "end ".repeat(n)), 1),
        (
            format!(
                "local t: {{x: number}}? = nil\n{}local y = t.x\n{}",
                "if t then ".repeat(n),
                "end ".repeat(n)
            ),
            0,
        ),
        (format!("local t: {{x: number}}? = nil\nlocal y = t{}", " and t".repeat(n) + " and t.x"), 0),
        (format!("local t: {{x: number}}? = nil\nlocal y = {}t.x{}", "(t and ".repeat(n), ")".repeat(n)), 0),
        (format!("local x: number = 1\nlocal s: string = {}x{}", "(".repeat(n), ")".repeat(n)), 1),
        (format!("local x: number = 1\nlocal s: number = {}x", "x + ".repeat(n)), 0),
        (format!("local x: number = 1\nlocal s: number = x{}", " .. x".repeat(n)), 1),
        (format!("local x: number? = 1\nlocal s = {}", "x + ".repeat(n) + "1"), 500),
        (format!("{}return a{}", "return function(a: number) ".repeat(n / 2), " end".repeat(n / 2)), 0),
        (
            format!(
                "{}local s: string = a{}",
                "return function(a: number) ".repeat(n / 2),
                " end".repeat(n / 2)
            ),
            1,
        ),
        (
            format!(
                "{}return a{}",
                "return function(a: number): number ".repeat(n / 2),
                " end".repeat(n / 2)
            ),
            500,
        ),
        // Types that share parts (4^n paths, n distinct goals: each decided once).
        (
            format!(
                "type A0 = number\ntype B0 = number\n{}local f = function(x: B{n}) end\nlocal g: (A{n}) -> () = f",
                (1..=n)
                    .map(|i| format!(
                        "type A{i} = {{x: A{j}, y: A{j}}}\ntype B{i} = {{x: B{j}, y: B{j}}}\n",
                        j = i - 1
                    ))
                    .collect::<String>()
            ),
            0,
        ),
        (
            format!(
                "local x: number = 1\nif x then\n{}else end",
                "elseif x then local y: string = x\n".repeat(n)
            ),
            500,
        ),
        (
            format!(
                "local function f(a: number) end\nf({})",
                (0..n).map(|_| "1").collect::<Vec<_>>().join(", ")
            ),
            1,
        ),
        (
            format!(
                "{}{}",
                "local function f(): number while true do ".repeat(n / 2),
                "end end ".repeat(n / 2)
            ),
            0,
        ),
    ];
    for (src, want) in cases {
        let head: String = src.chars().take(60).collect();
        let d = bounded(src, limit);
        let errors = d.iter().filter(|d| d.severity == Severity::Error).count();
        assert!(d.iter().all(|d| d.severity == Severity::Error || d.message.starts_with("type checking stopped here")), "{head}: {:?}", msgs(&d));
        assert!(
            !d.iter().any(|d| d.message.contains("stopped")) || want == 500,
            "{head}: {:?}",
            &msgs(&d)[..d.len().min(3)]
        );
        assert_eq!(errors, want, "{head}: {:?}", &msgs(&d)[..d.len().min(3)]);
    }
}

#[test]
fn check_stops_at_its_work_bound_with_a_warning() {
    // A type check whose subtyping is exponential without a bound: nested unions of
    // functions whose parameters are unions again, compared with a near miss.
    // Two unions of the same 20,000 string literals in opposite orders: each member of one
    // is found at the far end of the other (2 * 10^8 goals), past the work a check of this
    // source may do. The answer stays "fits": a stopped check reports nothing it is unsure of.
    let n = 20_000;
    let lits = |rev: bool| {
        let v: Vec<String> = (0..n).map(|i| format!("'k{}'", if rev { n - 1 - i } else { i })).collect();
        v.join(" | ")
    };
    let src = format!(
        "local h: string = 1\ntype U = {}\ntype V = {}\nlocal v: V = 'k0'\nlocal u: U = v\nlocal w: number = 'x'\n",
        lits(false),
        lits(true)
    );
    let d = bounded(src, Duration::from_secs(10));
    let last = d.last().unwrap();
    assert!(
        last.severity == Severity::Warning
            && last.message.starts_with("type checking stopped here: the program is too large"),
        "{d:?}"
    );
    // What was found before the stop is kept; nothing after it is reported.
    assert_eq!(d.len(), 2, "{d:?}");
    assert_eq!(d[0].message, "local 'h': expected string, got integer");
    assert_eq!(last.pos.line, 5, "it stops at the statement it was checking");
}

#[test]
fn check_fuzz_on_rlua_mutations() {
    let iters: usize =
        std::env::var("RLUA_CHECK_FUZZ_ITERS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
    let seed: u64 = std::env::var("RLUA_CHECK_FUZZ_SEED").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut pool: Vec<String> = CASES.iter().map(|(s, _)| (*s).to_owned()).collect();
    pool.extend(WELL_TYPED.iter().map(|s| (*s).to_owned()));
    pool.extend(MARKED.iter().map(|m| twins(m).0));
    for i in 0..200u64 {
        let mut rng = Rng::new(0xf022 ^ seed ^ i.wrapping_mul(0x9e37_79b9));
        let mut g = Gen::new(&mut rng, true);
        pool.push(g.program(6 + (i % 24) as i32).r);
    }
    let mut rng = Rng::new(0xc4ec ^ seed);
    let started = Instant::now();
    let mut slowest = Duration::ZERO;
    let mut typed = 0;
    for _ in 0..iters {
        let base = pool[rng.below(pool.len())].clone();
        let m = mutate::mutate(&base, &mut rng, &pool);
        let src = m.clone();
        let r = harness::with_timeout(crate::common::SMALL_STACK, move || {
            let t = Instant::now();
            let r = std::panic::catch_unwind(|| remedy_lua::check(src.as_bytes(), "m.rlua"));
            (r.map(|d| d.len()), t.elapsed())
        });
        let Some((r, took)) = r else { panic!("check timed out on:\n{m}") };
        let Ok(n) = r else { panic!("check panicked on:\n{m}") };
        if n > 0 {
            typed += 1;
        }
        slowest = slowest.max(took);
        assert!(took < Duration::from_secs(2), "check took {took:?} on:\n{m}");
    }
    println!("{iters} mutations in {:?}, slowest {slowest:?}, {typed} with diagnostics", started.elapsed());
}
