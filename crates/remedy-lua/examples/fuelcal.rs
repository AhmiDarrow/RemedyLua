//! Fuel calibration: nanoseconds per unit of fuel for each instruction kind and library
//! function, at small and large inputs. Each case runs `setup` and then `body` in an
//! endless loop until the fuel runs out; the time over the fuel spent is the cost of a
//! unit. A budget bounds wall time only when no case is far above the median.
//!
//! `cargo run -p remedy-lua --example fuelcal` (`FUELCAL_FUEL`, default 3M per run;
//! `FUELCAL_ONLY=<substring>` runs matching cases; `FUELCAL_MD=1` prints a Markdown table).
#![allow(clippy::unwrap_used, missing_docs, clippy::print_stdout)]

use remedy_lua::{Error, LimitKind, Libs, Limits, Lua};

/// (name, setup, body): `body` is one step of the loop.
const CASES: &[(&str, &str, &str)] = &[
    // ---- instructions (unrolled so the loop's own jump is a small part)
    ("op: move/loadk", "local a, b, c = 1, 2, 3", "a = b b = c c = a a = 1 b = 2 c = 3 a = b b = c"),
    ("op: int add/sub/mul", "local a, b = 1, 2", "a = a + b a = a - b a = a * 1 a = a + 1 a = a - 1 b = b * 1"),
    ("op: float div/pow/mod", "local a, b = 1.5, 2.5", "a = a / b a = a * b a = a ^ 1.0 a = a % 7.5 a = a // 1 + 1.5"),
    ("op: bitwise", "local a, b = 12345, 678", "a = a & b a = a | b a = a ~ b a = a << 1 a = a >> 1 a = ~a"),
    ("op: compare/jump", "local a, b, n = 1, 2, 0", "if a < b then n = 1 end if a == b then n = 2 end if a <= b then n = 3 end"),
    ("op: not/unm/len str", "local s, a = 'hello', 3", "a = -a a = #s a = not a a = 3"),
    ("op: upvalue get/set", "local u = 0 local function f() u = u + 1 u = u - 1 return u end", "f()"),
    ("op: global get/set", "g1 = 0", "g1 = g1 + 1 g1 = g1 - 1"),
    ("op: field get/set", "local t = {x = 1, y = 2}", "t.x = t.y t.y = t.x t.x = t.x + 1"),
    ("op: int index get/set", "local t = {1, 2, 3, 4}", "t[1] = t[2] t[3] = t[4] t[2] = t[1]"),
    ("op: hash int get/set", "local t = {[1000] = 1, [2000] = 2}", "t[1000] = t[2000] t[2000] = t[1000]"),
    ("op: string key get", "local t = {} for i = 1, 100 do t['k' .. i] = i end local k = 'k50'", "local v = t[k] v = t[k] v = t[k]"),
    ("op: method call", "local C = {} C.__index = C function C:m() return 1 end local o = setmetatable({}, C)", "o:m()"),
    ("op: call 0 args", "local function f() end", "f() f() f()"),
    ("op: call 8 args", "local function f(a, b, c, d, e, g, h, i) return a end", "f(1, 2, 3, 4, 5, 6, 7, 8)"),
    ("op: vararg call", "local function f(...) return select('#', ...) end", "f(1, 2, 3, 4, 5)"),
    ("op: tail call", "local function g(x) return x end local function f(x) return g(x) end", "f(1)"),
    ("op: closure", "", "local f = function() end"),
    ("op: closure w/ upvalues", "local a, b, c = 1, 2, 3", "local f = function() return a + b + c end"),
    ("op: new table {}", "", "local t = {}"),
    ("op: table ctor 3+2", "", "local t = {1, 2, 3, x = 1, y = 2}"),
    ("op: table ctor 50", "", "local t = {1,2,3,4,5,6,7,8,9,10,1,2,3,4,5,6,7,8,9,10,1,2,3,4,5,6,7,8,9,10,1,2,3,4,5,6,7,8,9,10,1,2,3,4,5,6,7,8,9,10}"),
    ("op: numeric for", "", "for i = 1, 100 do end"),
    ("op: pairs 100", "local t = {} for i = 1, 50 do t[i] = i t['k' .. i] = i end", "for k, v in pairs(t) do end"),
    ("op: ipairs 100", "local t = {} for i = 1, 100 do t[i] = i end", "for i, v in ipairs(t) do end"),
    ("op: concat short", "local a, b = 'ab', 'cd'", "local s = a .. b .. a"),
    ("op: concat 1 KB", "local a = string.rep('x', 512)", "local s = a .. a"),
    ("op: concat 64 KB", "local a = string.rep('x', 32768)", "local s = a .. a"),
    ("op: concat numbers", "local a, b = 12345, 1.5", "local s = a .. b"),
    ("op: eq long strings", "local a, b = string.rep('x', 10000), string.rep('x', 10000)", "local r = a == b"),
    ("op: lt long strings", "local a, b = string.rep('x', 10000), string.rep('x', 10000) .. 'y'", "local r = a < b"),
    ("op: string coercion", "local s = '12345'", "local n = s + 1"),
    ("op: len table", "local t = {} for i = 1, 1000 do t[i] = i end", "local n = #t"),
    ("op: len hash border", "local t = {} for i = 1, 1000 do t[i] = i end t[500] = nil", "local n = #t"),
    ("mm: __index fn", "local t = setmetatable({}, {__index = function(t, k) return k end})", "local v = t.x"),
    ("mm: __index chain 5", "local b = {x = 1} for i = 1, 5 do b = setmetatable({}, {__index = b}) end local t = b", "local v = t.x"),
    ("mm: __newindex fn", "local t = setmetatable({}, {__newindex = function(t, k, v) end})", "t.x = 1"),
    ("mm: __call", "local t = setmetatable({}, {__call = function(self, a) return a end})", "t(1)"),
    ("mm: __eq", "local mt = {__eq = function() return true end} local a, b = setmetatable({}, mt), setmetatable({}, mt)", "local r = a == b"),
    ("mm: __lt", "local mt = {__lt = function() return true end} local a, b = setmetatable({}, mt), setmetatable({}, mt)", "local r = a < b"),
    ("mm: __add", "local mt = {__add = function(a, b) return 1 end} local a = setmetatable({}, mt)", "local r = a + 1"),
    ("mm: __concat", "local mt = {__concat = function(a, b) return 'x' end} local a = setmetatable({}, mt)", "local r = a .. 'y'"),
    ("mm: __len", "local t = setmetatable({}, {__len = function() return 3 end})", "local n = #t"),
    ("mm: __tostring", "local t = setmetatable({}, {__tostring = function() return 'T' end})", "local s = tostring(t)"),
    ("err: pcall ok", "local function f() end", "pcall(f)"),
    ("err: error/pcall", "local function f() error('x') end", "pcall(f)"),
    ("err: error table", "local e = {} local function f() error(e) end", "pcall(f)"),
    ("co: resume/yield", "local co = coroutine.wrap(function() while true do coroutine.yield(1) end end)", "co()"),
    ("co: create+run", "local f = function() return 1 end", "coroutine.resume(coroutine.create(f))"),
    // ---- base
    ("base: type/select", "", "local a = type(1) a = select('#', 1, 2, 3)"),
    ("base: tostring int", "", "local s = tostring(123456)"),
    ("base: tostring float", "", "local s = tostring(1.2345)"),
    ("base: tostring table", "local t = {}", "local s = tostring(t)"),
    ("base: tonumber short", "", "local n = tonumber('12345')"),
    ("base: tonumber 1 KB", "local s = string.rep(' ', 1000) .. '1'", "local n = tonumber(s)"),
    ("base: tonumber base", "", "local n = tonumber('ffff', 16)"),
    ("base: rawget/rawset", "local t = {}", "rawset(t, 'x', 1) local v = rawget(t, 'x')"),
    ("base: rawequal/rawlen", "local t = {1, 2}", "local a = rawequal(t, t) a = rawlen(t)"),
    ("base: next", "local t = {a = 1, b = 2}", "local k = next(t) k = next(t, k)"),
    ("base: setmetatable", "local mt = {}", "local t = setmetatable({}, mt) local m = getmetatable(t)"),
    ("base: assert", "", "assert(true, 'x')"),
    ("base: unpack 3", "local t = {1, 2, 3}", "local a, b, c = table.unpack(t)"),
    ("base: unpack 200", "local t = {} for i = 1, 200 do t[i] = i end", "local n = select('#', table.unpack(t))"),
    ("base: select 200", "local t = {} for i = 1, 200 do t[i] = i end", "local n = select(100, table.unpack(t))"),
    ("base: load small", "", "local f = load('return 1 + 2')"),
    ("base: load 4 KB", "local src = string.rep('local x = 1 + 2 * 3\\n', 200)", "local f = load(src)"),
    ("base: load deep", "local src = 'return ' .. string.rep('(', 150) .. '1' .. string.rep(')', 150)", "local f = load(src)"),
    ("base: collectgarbage step", "local keep = {} for i = 1, 1000 do keep[i] = {} end", "collectgarbage('step')"),
    ("base: collectgarbage count", "", "local n = collectgarbage('count')"),
    // ---- string
    ("str: len/byte", "local s = 'hello world'", "local n = #s n = s:byte(3) n = s:len()"),
    ("str: byte 1 KB all", "local s = string.rep('x', 1000)", "local n = select('#', s:byte(1, -1))"),
    ("str: char 8", "", "local s = string.char(65, 66, 67, 68, 69, 70, 71, 72)"),
    ("str: char 200", "local t = {} for i = 1, 200 do t[i] = 65 end", "local s = string.char(table.unpack(t))"),
    ("str: sub short", "local s = 'hello world'", "local r = s:sub(2, 5)"),
    ("str: sub 64 KB", "local s = string.rep('x', 70000)", "local r = s:sub(2, 65537)"),
    ("str: upper short", "local s = 'hello'", "local r = s:upper()"),
    ("str: upper 64 KB", "local s = string.rep('x', 65536)", "local r = s:upper()"),
    ("str: reverse 64 KB", "local s = string.rep('xy', 32768)", "local r = s:reverse()"),
    ("str: rep short", "", "local r = ('ab'):rep(10)"),
    ("str: rep 64 KB", "", "local r = ('ab'):rep(32768)"),
    ("str: rep sep 64 KB", "", "local r = ('a'):rep(32768, 'b')"),
    ("str: format %d %s", "", "local r = string.format('%d %s', 42, 'x')"),
    ("str: format %5.2f %g", "", "local r = string.format('%5.2f %g', 3.14159, 1e300)"),
    ("str: format %q short", "", "local r = string.format('%q', 'a\\nb')"),
    ("str: format %q 64 KB ctl", "local s = string.rep('\\1', 65536)", "local r = string.format('%q', s)"),
    ("str: format %q 64 KB text", "local s = string.rep('x', 65536)", "local r = string.format('%q', s)"),
    ("str: format %.3s 64 KB", "local s = string.rep('x', 65536)", "local r = string.format('%.3s', s)"),
    ("str: format %s 64 KB", "local s = string.rep('x', 65536)", "local r = string.format('%s', s)"),
    ("str: format %99.99f", "", "local r = string.format('%99.99f', 1e300)"),
    ("str: format %a", "", "local r = string.format('%a', 1.5)"),
    ("str: find plain short", "local s = 'hello world'", "local r = s:find('wor', 1, true)"),
    ("str: find plain 64 KB", "local s = string.rep('x', 65536) .. 'y'", "local r = s:find('y', 1, true)"),
    ("str: find plain worst", "local s = string.rep('a', 65536) local p = string.rep('a', 64) .. 'b'", "local r = s:find(p, 1, true)"),
    ("str: find pattern short", "local s = 'key=value'", "local r = s:find('(%w+)=(%w+)')"),
    ("str: find pattern 64 KB miss", "local s = string.rep('x', 65536)", "local r = s:find('%d')"),
    ("str: find backtrack", "local s = string.rep('a', 2000)", "local r = pcall(string.find, s, 'a*a*b')"),
    ("str: match short", "local s = '  hi there  '", "local r = s:match('^%s*(.-)%s*$')"),
    ("str: gmatch words", "local s = string.rep('ab cd ', 100)", "for w in s:gmatch('%a+') do end"),
    ("str: gmatch chars", "local s = string.rep('a', 1000)", "for c in s:gmatch('a') do end"),
    ("str: gmatch empty", "local s = string.rep('a', 1000)", "for c in s:gmatch('') do end"),
    ("str: gsub str", "local s = string.rep('hello ', 200)", "local r = s:gsub('l', 'L')"),
    ("str: gsub %0", "local s = string.rep('hello ', 200)", "local r = s:gsub('%w+', '<%0>')"),
    ("str: gsub table", "local s = string.rep('ab', 500) local t = {a = '1', b = '2'}", "local r = s:gsub('%a', t)"),
    ("str: gsub function", "local s = string.rep('ab', 500)", "local r = s:gsub('%a', function(c) return c end)"),
    ("str: gsub 64 KB no match", "local s = string.rep('x', 65536)", "local r = s:gsub('y', 'z')"),
    ("str: pack/unpack", "", "local p = string.pack('i4 d s1', 1, 2.5, 'x') local a, b, c = string.unpack('i4 d s1', p)"),
    ("str: pack 64 KB s", "local s = string.rep('x', 65536)", "local p = string.pack('s4', s)"),
    ("str: packsize", "", "local n = string.packsize('i4 i8 d c100')"),
    // ---- table
    ("tab: insert/remove end", "local t = {}", "table.insert(t, 1) table.remove(t)"),
    ("tab: insert front 1000", "local t = {} for i = 1, 1000 do t[i] = i end", "table.insert(t, 1, 0) table.remove(t, 1)"),
    ("tab: concat 10", "local t = {} for i = 1, 10 do t[i] = 'ab' end", "local s = table.concat(t, ',')"),
    ("tab: concat 1000", "local t = {} for i = 1, 1000 do t[i] = 'ab' end", "local s = table.concat(t, ',')"),
    ("tab: concat numbers", "local t = {} for i = 1, 1000 do t[i] = i * 1.5 end", "local s = table.concat(t, ',')"),
    ("tab: sort 10", "local src = {5, 3, 8, 1, 9, 2, 7, 4, 6, 0}", "local t = {table.unpack(src)} table.sort(t)"),
    ("tab: sort 1000", "local src = {} for i = 1, 1000 do src[i] = (i * 7919) % 1000 end", "local t = table.move(src, 1, 1000, 1, {}) table.sort(t)"),
    ("tab: sort 1000 cmp", "local src = {} for i = 1, 1000 do src[i] = (i * 7919) % 1000 end", "local t = table.move(src, 1, 1000, 1, {}) table.sort(t, function(a, b) return a > b end)"),
    ("tab: sort strings", "local src = {} for i = 1, 500 do src[i] = string.rep('k', 50) .. ((i * 7919) % 500) end", "local t = table.move(src, 1, 500, 1, {}) table.sort(t)"),
    ("tab: move 1000", "local src = {} for i = 1, 1000 do src[i] = i end", "local t = table.move(src, 1, 1000, 1, {})"),
    ("tab: pack 10", "", "local t = table.pack(1, 2, 3, 4, 5, 6, 7, 8, 9, 10)"),
    ("tab: grow 1000", "", "local t = {} for i = 1, 1000 do t[i] = i end"),
    ("tab: grow hash 1000", "", "local t = {} for i = 1, 1000 do t[i * 1000] = i end"),
    ("tab: string keys 1000", "local ks = {} for i = 1, 1000 do ks[i] = 'k' .. i end", "local t = {} for i = 1, 1000 do t[ks[i]] = i end"),
    // ---- math, utf8, os
    ("math: floor/sqrt/abs", "", "local a = math.floor(3.7) a = math.sqrt(2) a = math.abs(-3)"),
    ("math: random", "", "local a = math.random(1, 100)"),
    ("math: max 50", "local t = {} for i = 1, 50 do t[i] = i end", "local a = math.max(table.unpack(t))"),
    ("math: fmod/tointeger", "", "local a = math.fmod(7, 3) a = math.tointeger(3.0)"),
    ("utf8: char", "", "local s = utf8.char(72, 228, 8364, 128512)"),
    ("utf8: len 64 KB", "local s = string.rep('\\u{20AC}', 20000)", "local n = utf8.len(s)"),
    ("utf8: codepoint 1 KB", "local s = string.rep('\\u{20AC}', 300)", "local n = select('#', utf8.codepoint(s, 1, -1))"),
    ("utf8: offset", "local s = string.rep('\\u{20AC}', 300)", "local n = utf8.offset(s, 150)"),
    ("utf8: codes 1 KB", "local s = string.rep('\\u{20AC}', 300)", "for p, c in utf8.codes(s) do end"),
    ("os: time/clock", "", "local a = os.time() a = os.clock()"),
    ("os: date", "", "local s = os.date('%Y-%m-%d %H:%M:%S', 0)"),
    ("os: date *t", "", "local t = os.date('*t', 0)"),
];

fn main() {
    let fuel: u64 = std::env::var("FUELCAL_FUEL").ok().and_then(|s| s.parse().ok()).unwrap_or(3_000_000);
    let only = std::env::var("FUELCAL_ONLY").ok();
    let md = std::env::var_os("FUELCAL_MD").is_some();
    let mut rows: Vec<(&str, f64, String)> = Vec::new();
    for &(name, setup, body) in CASES {
        if only.as_deref().is_some_and(|o| !name.contains(o)) {
            continue;
        }
        let src = format!("{setup}\nwhile true do\n{body}\nend");
        let mut best = f64::MAX;
        let mut note = String::new();
        for _ in 0..3 {
            let mut lua = Lua::new(Limits::default().fuel(fuel).memory(256 << 20).hash_seed(7), Libs::ALL);
            let f = match lua.load(&src, "=cal") {
                Ok(f) => f,
                Err(e) => {
                    note = format!("load: {e:?}");
                    break;
                }
            };
            let t = std::time::Instant::now();
            let r = lua.call(f.value(), &[]);
            let el = t.elapsed().as_secs_f64();
            let used = fuel - lua.fuel_left();
            match r {
                Err(Error::Limit(LimitKind::Fuel)) => {}
                other => note = format!("{other:?}").chars().take(80).collect(),
            }
            best = best.min(el * 1e9 / used.max(1) as f64);
        }
        if !md {
            println!("{best:9.1} ns/unit  {name} {note}");
        }
        rows.push((name, best, note));
    }
    let mut v: Vec<f64> = rows.iter().map(|r| r.1).collect();
    v.sort_by(f64::total_cmp);
    let median = v.get(v.len() / 2).copied().unwrap_or(0.0);
    if md {
        println!("| case | ns/unit | x median |\n|---|---:|---:|");
        for (name, ns, note) in &rows {
            println!("| {name}{} | {ns:.1} | {:.2} |", if note.is_empty() { String::new() } else { format!(" ({note})") }, ns / median);
        }
    }
    println!("median {median:.1} ns/unit, max {:.1} ({:.1}x)", v.last().copied().unwrap_or(0.0), v.last().copied().unwrap_or(0.0) / median);
    let mut worst: Vec<_> = rows.iter().filter(|r| r.1 > 4.0 * median).collect();
    worst.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (name, ns, _) in worst {
        println!("  over 4x: {name} {ns:.1} ({:.1}x)", ns / median);
    }
}
