//! One export, `smoke`: makes a state with every library, runs a script that touches
//! tables, strings, closures, coroutines, `pcall`, `os` and the fuel limit, and returns
//! 1 on success or a negative step number on failure.

use remedy_lua::{Error, LimitKind, Libs, Limits, Lua, Value};

const SCRIPT: &str = r#"
local t = {}
for i = 1, 1000 do t[i] = ("x"):rep(i % 7) .. i end
table.sort(t)
local co = coroutine.wrap(function(a) local b = coroutine.yield(a + 1) return b * 2 end)
assert(co(1) == 2 and co(5) == 10)
local ok, e = pcall(error, {code = 7})
assert(not ok and e.code == 7)
assert(os.time() == 0 and os.clock() == 0)
assert(#os.date("!%Y-%m-%d", 86400) == 10)
assert(("%d|%s"):format(42, "hi") == "42|hi")
assert(string.gsub("hello world", "o", "0") == "hell0 w0rld")
return #t, math.type(1 // 1)
"#;

#[unsafe(no_mangle)]
pub extern "C" fn smoke() -> i32 {
    let mut lua = Lua::new(Limits::default().fuel(5_000_000), Libs::ALL);
    let Ok(out) = lua.exec(SCRIPT, "=smoke") else { return -1 };
    if !matches!(out[..], [Value::Int(1000), Value::Str(_)]) {
        return -2;
    }
    if !matches!(lua.exec("while true do end", "=spin"), Err(Error::Limit(LimitKind::Fuel))) {
        return -3;
    }
    // A host-supplied wall clock reaches os.time.
    lua.set_os_clocks(Some(std::sync::Arc::new(|| std::time::Duration::from_secs(1_700_000_000))), None);
    match lua.exec("return os.time()", "=clock") {
        Ok(v) if matches!(v[..], [Value::Int(1_700_000_000)]) => 1,
        _ => -4,
    }
}
