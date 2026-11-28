//! Typed host functions: arguments read into Rust types, results made from them, and C
//! Lua's argument errors for free. Run with `cargo run -p remedy-lua --example typed`.

use remedy_lua::{Error, Libs, Limits, Lua, Value, Variadic};

fn main() -> Result<(), Error> {
    let mut lua = Lua::new(Limits::default(), Libs::SAFE);

    lua.register_typed("geo.dist", |_, (x, y, scale): (f64, f64, Option<f64>)| {
        Ok((x * x + y * y).sqrt() * scale.unwrap_or(1.0))
    })?;
    lua.register_typed("text.join", |_, (sep, parts): (String, Variadic<String>)| Ok(parts.join(&sep)))?;
    lua.register_typed("text.split", |_, (s, sep): (String, String)| {
        Ok(s.split(sep.as_str()).map(str::to_owned).collect::<Variadic<String>>())
    })?;

    let out = lua.exec(
        "local a, b, c = text.split('red,green,blue', ',') \
         local ok, err = pcall(function() return geo.dist(3) end) \
         return geo.dist(3, 4), text.join(' + ', a, b, c), err",
        "=example",
    )?;
    for v in out {
        match v {
            Value::Num(f) => println!("{f}"),
            other => println!("{}", lua.to_string_lossy(other).unwrap_or_default()),
        }
    }
    Ok(())
}
