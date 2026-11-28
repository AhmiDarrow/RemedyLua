//! Typed host functions (`create_typed`, `FromLua` / `IntoLua`): every conversion, the
//! argument errors C Lua's `luaL_check*` give, and the fuel charged for copying out.

use remedy_lua::{Data, Error, Libs, LimitKind, Limits, Lua, TableRef, Value, Variadic};

fn lua() -> Lua {
    Lua::new(Limits::default(), Libs::SAFE)
}

/// The error message `src` raises, caught by `pcall` in the script.
fn caught(lua: &mut Lua, src: &str) -> String {
    let r =
        lua.exec(format!("local ok, e = pcall(function() {src} end) assert(not ok) return e"), "=t").unwrap();
    lua.to_string_lossy(r[0]).unwrap()
}

fn strs(lua: &Lua, vs: &[Value]) -> Vec<String> {
    vs.iter()
        .map(|&v| match v {
            Value::Nil => "nil".into(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Num(f) => format!("{f:?}"),
            Value::Str(_) => format!("{:?}", lua.to_string_lossy(v).unwrap()),
            other => other.type_name().into(),
        })
        .collect()
}

#[test]
fn numbers_convert_as_lual_check_does() {
    let mut lua = lua();
    lua.register_typed("int", |_, i: i64| Ok(i)).unwrap();
    lua.register_typed("num", |_, f: f64| Ok(f)).unwrap();
    lua.register_typed("small", |_, i: u8| Ok(i)).unwrap();
    let r = lua
        .exec("return int(7), int(3.0), int('10'), int(' 0x10 '), num(1), num('2.5'), small(255)", "=t")
        .unwrap();
    assert_eq!(strs(&lua, &r), ["7", "3", "10", "16", "1.0", "2.5", "255"]);

    assert_eq!(caught(&mut lua, "int('x')"), "t:1: bad argument #1 to 'int' (number expected, got string)");
    assert_eq!(caught(&mut lua, "int()"), "t:1: bad argument #1 to 'int' (number expected, got no value)");
    assert_eq!(caught(&mut lua, "int(nil)"), "t:1: bad argument #1 to 'int' (number expected, got nil)");
    assert_eq!(
        caught(&mut lua, "int(1.5)"),
        "t:1: bad argument #1 to 'int' (number has no integer representation)"
    );
    assert_eq!(
        caught(&mut lua, "int('1.5')"),
        "t:1: bad argument #1 to 'int' (number has no integer representation)"
    );
    assert_eq!(caught(&mut lua, "num({})"), "t:1: bad argument #1 to 'num' (number expected, got table)");
    assert_eq!(caught(&mut lua, "small(256)"), "t:1: bad argument #1 to 'small' (value out of range for u8)");
    assert_eq!(caught(&mut lua, "small(-1)"), "t:1: bad argument #1 to 'small' (value out of range for u8)");
}

#[test]
fn positions_count_every_argument_and_methods_shift_them() {
    let mut lua = lua();
    lua.register_typed("f", |_, (a, b, c): (i64, String, bool)| Ok(format!("{a}{b}{c}"))).unwrap();
    let r = lua.exec("return f(1, 'x', nil), f(2, 3.5, 0)", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["\"1xfalse\"", "\"23.5true\""]);
    assert_eq!(caught(&mut lua, "f(1)"), "t:1: bad argument #2 to 'f' (string expected, got no value)");
    assert_eq!(caught(&mut lua, "f(1, {})"), "t:1: bad argument #2 to 'f' (string expected, got table)");

    // A field function called as a method: C Lua's numbering, self as "bad self".
    let m = lua.create_typed("m", |_, (_t, n): (TableRef, i64)| Ok(n * 2)).unwrap();
    lua.exec("obj = {}", "=t").unwrap();
    let obj = lua.get_global("obj").unwrap();
    let k = lua.create_string("m").unwrap();
    lua.set(obj, k, m).unwrap();
    let r = lua.exec("return obj:m(21)", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["42"]);
    assert_eq!(caught(&mut lua, "obj:m('x')"), "t:1: bad argument #1 to 'm' (number expected, got string)");
    assert_eq!(caught(&mut lua, "obj.m(5, 1)"), "t:1: bad argument #1 to 'm' (table expected, got number)");
    assert_eq!(
        caught(&mut lua, "local o = setmetatable({}, {__index = obj}) o.m(o)"),
        "t:1: bad argument #2 to 'm' (number expected, got no value)"
    );
}

#[test]
fn strings_options_and_values() {
    let mut lua = lua();
    lua.register_typed("s", |_, s: String| Ok(s.len())).unwrap();
    lua.register_typed("opt", |_, (a, b): (Option<i64>, Option<String>)| Ok(format!("{a:?} {b:?}"))).unwrap();
    lua.register_typed("ty", |_, v: Value| Ok(v.type_name())).unwrap();
    let r = lua
        .exec("return s('héllo'), s(12), s(-0.5), opt(), opt(nil, 'x'), opt(3), ty(), ty({})", "=t")
        .unwrap();
    assert_eq!(
        strs(&lua, &r),
        [
            "6",
            "2",
            "4",
            "\"None None\"",
            "\"None Some(\\\"x\\\")\"",
            "\"Some(3) None\"",
            "\"nil\"",
            "\"table\""
        ]
    );
    assert_eq!(caught(&mut lua, "s('\\xff')"), "t:1: bad argument #1 to 's' (string is not valid UTF-8)");
    assert_eq!(caught(&mut lua, "opt('x')"), "t:1: bad argument #1 to 'opt' (number expected, got string)");
    // `__name` names the type, as in `luaL_typeerror`.
    assert_eq!(
        caught(&mut lua, "s(setmetatable({}, {__name = 'Widget'}))"),
        "t:1: bad argument #1 to 's' (string expected, got Widget)"
    );
}

#[test]
fn variadic_takes_the_rest_and_numbers_its_errors() {
    let mut lua = lua();
    lua.register_typed("sum", |_, xs: Variadic<f64>| Ok(xs.iter().fold(0.0, |a, b| a + b))).unwrap();
    lua.register_typed("join", |_, (sep, xs): (String, Variadic<i64>)| {
        Ok(xs.iter().map(i64::to_string).collect::<Vec<_>>().join(&sep))
    })
    .unwrap();
    let r = lua.exec("return sum(), sum(1, 2, 3.5), join('-'), join(',', 1, 2, 3)", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["0.0", "6.5", "\"\"", "\"1,2,3\""]);
    assert_eq!(
        caught(&mut lua, "sum(1, 2, 'x')"),
        "t:1: bad argument #3 to 'sum' (number expected, got string)"
    );
    assert_eq!(
        caught(&mut lua, "join(',', 1, {})"),
        "t:1: bad argument #3 to 'join' (number expected, got table)"
    );
    assert_eq!(caught(&mut lua, "join()"), "t:1: bad argument #1 to 'join' (string expected, got no value)");
}

#[test]
fn results_tuples_unit_and_variadic() {
    let mut lua = lua();
    lua.register_typed("many", |_, ()| Ok((1i64, "two", 3.0f64, true, None::<i64>, String::from("six"))))
        .unwrap();
    lua.register_typed("none", |_, ()| Ok(())).unwrap();
    lua.register_typed("range", |_, n: i64| Ok((1..=n).collect::<Variadic<i64>>())).unwrap();
    let r = lua.exec("return select('#', many()), select('#', none()), many()", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["6", "0", "1", "\"two\"", "3.0", "true", "nil", "\"six\""]);
    let r = lua.exec("return range(4)", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["1", "2", "3", "4"]);
    // Twelve arguments, the widest tuple.
    lua.register_typed(
        "twelve",
        |_,
         (a, b, c, d, e, f, g, h, i, j, k, l): (
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
            i64,
        )| { Ok(a + b + c + d + e + f + g + h + i + j + k + l) },
    )
    .unwrap();
    let r = lua.exec("return twelve(1,2,3,4,5,6,7,8,9,10,11,12)", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["78"]);
    assert_eq!(
        caught(&mut lua, "twelve(1,2,3,4,5,6,7,8,9,10,11)"),
        "t:1: bad argument #12 to 'twelve' (number expected, got no value)"
    );
}

#[test]
fn integers_that_do_not_fit_are_errors_not_wraps() {
    let mut lua = lua();
    lua.register_typed("big", |_, ()| Ok(u64::MAX)).unwrap();
    lua.register_typed("ok", |_, ()| Ok(i64::MAX as u64)).unwrap();
    let r = lua.exec("return ok()", "=t").unwrap();
    assert_eq!(strs(&lua, &r), [i64::MAX.to_string()]);
    let e = caught(&mut lua, "big()");
    assert!(e.contains("18446744073709551615 does not fit a Lua integer"), "{e}");
}

#[test]
fn data_crosses_both_ways() {
    let mut lua = lua();
    lua.register_typed("echo", |_, d: Data| {
        let n = match &d {
            Data::Table(kv) => kv.len() as i64,
            _ => -1,
        };
        Ok((n, d))
    })
    .unwrap();
    let r = lua.exec("local n, t = echo({a = 1, 2}) return n, t.a, t[1]", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["2", "1", "2"]);
    assert_eq!(
        caught(&mut lua, "echo(function() end)"),
        "t:1: bad argument #1 to 'echo' (a function cannot be copied out)"
    );
}

#[test]
fn errors_from_the_body_pass_through_and_are_catchable() {
    let mut lua = lua();
    lua.register_typed("fail", |_, msg: String| -> Result<(), Error> { Err(Error::runtime(msg)) }).unwrap();
    lua.register_typed("checked", |_, n: i64| {
        if n < 0 {
            return Err(Error::BadArgument { position: 1, message: "must not be negative".into() });
        }
        Ok(n)
    })
    .unwrap();
    assert_eq!(caught(&mut lua, "fail('boom')"), "t:1: boom");
    assert_eq!(caught(&mut lua, "checked(-1)"), "t:1: bad argument #1 to 'checked' (must not be negative)");
    // An uncaught one reaches the host as a runtime error with the same text.
    match lua.exec("checked(-2)", "=t") {
        Err(Error::Runtime { message, .. }) => {
            assert_eq!(message, "t:1: bad argument #1 to 'checked' (must not be negative)")
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn bad_argument_from_an_untyped_function_is_named_too() {
    let mut lua = lua();
    lua.register("lib.raw", |_, _args| Err(Error::BadArgument { position: 2, message: "nope".into() }))
        .unwrap();
    assert_eq!(caught(&mut lua, "lib.raw(1, 2)"), "t:1: bad argument #2 to 'raw' (nope)");
}

#[test]
fn mut_and_registered_in_tables() {
    let mut lua = lua();
    let mut count = 0i64;
    lua.register_typed_mut("counter.next", move |_, step: Option<i64>| {
        count += step.unwrap_or(1);
        Ok(count)
    })
    .unwrap();
    let r = lua.exec("counter.next() counter.next(10) return counter.next()", "=t").unwrap();
    assert_eq!(strs(&lua, &r), ["12"]);
}

#[test]
fn copying_strings_out_is_charged_fuel() {
    // A script may hand the same big string to a host function a thousand times over:
    // copying it into `String`s is the host's work, and the script pays for it in fuel.
    let src = "local s = string.rep('x', 1 << 20) local t = {} for i = 1, 1000 do t[i] = s end return f(table.unpack(t))";
    let limits = Limits::default().fuel(4_000_000).memory(64 << 20);

    let mut lua = Lua::new(limits, Libs::SAFE);
    lua.register_typed("f", |_, xs: Variadic<Value>| Ok(xs.len())).unwrap();
    let r = lua.exec(src, "=t").unwrap();
    assert!(matches!(r.as_slice(), [Value::Int(1000)]));

    let mut lua = Lua::new(limits, Libs::SAFE);
    lua.register_typed("f", |_, xs: Variadic<String>| Ok(xs.len())).unwrap();
    assert_eq!(lua.exec(src, "=t").unwrap_err(), Error::Limit(LimitKind::Fuel));
}

#[test]
fn copying_data_out_is_charged_fuel() {
    let src = "local s = string.rep('x', 1 << 20) local t = {} for i = 1, 1000 do t[i] = s end return f(table.unpack(t))";
    let limits = Limits::default().fuel(4_000_000).memory(64 << 20);
    let mut lua = Lua::new(limits, Libs::SAFE);
    lua.register_typed("f", |_, xs: Variadic<Data>| Ok(xs.len())).unwrap();
    assert_eq!(lua.exec(src, "=t").unwrap_err(), Error::Limit(LimitKind::Fuel));
}

#[test]
fn conversions_outside_a_call_need_no_fuel() {
    use remedy_lua::{FromLua, IntoLua};
    let mut lua = Lua::new(Limits::default().fuel(1), Libs::SAFE);
    let v = "a long string ".repeat(1000).into_lua(&mut lua).unwrap();
    let s = String::from_lua(v, &mut lua).unwrap();
    assert_eq!(s.len(), 14_000);
    assert_eq!(i64::from_lua(Value::Num(4.0), &mut lua).unwrap(), 4);
}
