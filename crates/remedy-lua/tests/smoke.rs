//! Quick end-to-end checks.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use remedy_lua::{Libs, Limits, Lua};

fn run(src: &str) -> String {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    match lua.exec(src, "=t") {
        Ok(v) => {
            let parts: Vec<String> = v
                .iter()
                .map(|x| {
                    let s = lua.call_global("tostring", &[*x]).unwrap().unwrap();
                    lua.to_string_lossy(s[0]).unwrap()
                })
                .collect();
            parts.join(",")
        }
        Err(e) => format!("ERR {e}"),
    }
}

#[test]
fn basics() {
    assert_eq!(run("return 1 + 2"), "3");
    assert_eq!(run("return 10 / 2, 7 // 2, 7 % 3, 2^10"), "5.0,3,1,1024.0");
    assert_eq!(run("local t = {} for i = 1, 10 do t[i] = i * i end return #t, t[10]"), "10,100");
    assert_eq!(run("local function f(n) if n < 2 then return n end return f(n-1) + f(n-2) end return f(20)"), "6765");
    assert_eq!(run("return ('hello'):upper(), string.format('%5.2f|%d|%s', 3.14159, 42, 'x')"), "HELLO, 3.14|42|x");
    assert_eq!(run("local s = 0 for k, v in pairs({a=1, b=2, c=3}) do s = s + v end return s"), "6");
    assert_eq!(run("return select('#', 1, 2, 3), select(2, 'a', 'b', 'c')"), "3,b,c");
    assert_eq!(run("local ok, e = pcall(error, 'boom') return ok, e"), "false,boom");
    assert_eq!(run("local ok, e = pcall(function() error('boom') end) return ok, e"), "false,t:1: boom");
    assert_eq!(run("local co = coroutine.create(function(a) local b = coroutine.yield(a + 1) return b * 2 end) local _, x = coroutine.resume(co, 1) local _, y = coroutine.resume(co, 10) return x, y, coroutine.status(co)"), "2,20,dead");
    assert_eq!(run("local mt = {__add = function(a, b) return a.v + b.v end} local a = setmetatable({v=1}, mt) local b = setmetatable({v=2}, mt) return a + b"), "3");
    assert_eq!(run("return x.y"), "ERR t:1: attempt to index a nil value (global 'x')");
    assert_eq!(run("local t = {} return t.a.b"), "ERR t:1: attempt to index a nil value (field 'a')");
    assert_eq!(run("return string.find('hello world', 'o w'), string.match('key=val', '(%w+)=(%w+)')"), "5,key,val");
    assert_eq!(run("return (string.gsub('hello', 'l', 'L'))"), "heLLo");
    assert_eq!(run("local t = {5, 2, 8, 1} table.sort(t) return table.concat(t, ',')"), "1,2,5,8");
    assert_eq!(run("goto skip; error('no') ::skip:: return 'ok'"), "ok");
    assert_eq!(run("local x <const> = 5 do local y <close> = setmetatable({}, {__close = function() x2 = 1 end}) end return x, x2"), "5,1");
    assert_eq!(run("return math.type(1), math.type(1.0), math.maxinteger + 1 == math.mininteger"), "integer,float,true");
}

#[test]
fn remedy_syntax() {
    let mut lua = Lua::new(Limits::default(), Libs::ALL);
    let v = lua.exec("local x: number = 1 x += 2 local s = `x is {x}` local n = 0 for i = 1, 10 do if i % 2 == 0 then continue end n += i end return s, n", "=t.rlua").unwrap();
    assert_eq!(lua.to_string_lossy(v[0]).unwrap(), "x is 3");
    assert!(matches!(v[1], remedy_lua::Value::Int(25)));
}

#[test]
fn limits() {
    let mut lua = Lua::new(Limits::default().fuel(100_000), Libs::ALL);
    assert_eq!(lua.exec("while true do end", "=t"), Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel)));
    assert_eq!(lua.exec("while true do pcall(function() while true do end end) end", "=t"), Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Fuel)));
    let mut lua = Lua::new(Limits::default().memory(1 << 20), Libs::ALL);
    let r = lua.exec("local t = {} for i = 1, 1e8 do t[i] = string.rep('x', 100) .. i end", "=t");
    assert_eq!(r, Err(remedy_lua::Error::Limit(remedy_lua::LimitKind::Memory)));
    assert!(lua.memory_used() <= 1 << 20);
    assert!(lua.exec("return 1", "=t").is_ok());
}
