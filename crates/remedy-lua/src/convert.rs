//! Typed conversions between Rust values and script values, for host functions made with
//! [`Lua::create_typed`] (and for hosts that want them anywhere else).
//!
//! [`FromLua`] reads one value, [`IntoLua`] makes one; [`FromLuaMulti`] and
//! [`IntoLuaMulti`] do argument lists and result lists (tuples up to twelve, `()`, and
//! [`Variadic`] for "the rest"). Conversions follow C Lua's `luaL_check*`: numbers and
//! numeric strings convert as `luaL_checkinteger` and `luaL_checknumber` do, numbers become
//! strings as `luaL_checklstring` makes them, and a mismatch is an
//! [`Error::BadArgument`] that the script sees as `bad argument #2 to 'f' (number expected,
//! got no value)`.
//!
//! Copying out of the script is charged: a string converted to a Rust `String` costs the
//! fuel of copying its bytes, so a script cannot make the host copy more than its fuel
//! pays for (`f(table.unpack(t))` with a big string a million times over, into a
//! `Variadic<String>`). There is no `FromLua` for `Vec<T>` or maps for the same reason:
//! take a table as a [`TableRef`], or as [`Data`], which [`DataLimits`] bound.

use crate::{Data, DataLimits, Error, FuncRef, Lua, Root, TableRef, Value};

/// A Rust type that can be read from one script value.
pub trait FromLua: Sized {
    /// Reads `v`. A type mismatch is an [`Error::BadArgument`] (its position is filled in
    /// by the argument list) or an [`Error::Runtime`]; a limit (fuel for copying) stops the
    /// call.
    fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error>;

    /// Reads an argument the caller did not pass. By default the same as `nil`; the
    /// library's types say "got no value" there, as C Lua does.
    fn from_missing(lua: &mut Lua) -> Result<Self, Error> {
        Self::from_lua(Value::Nil, lua)
    }
}

/// A Rust value that can become one script value.
pub trait IntoLua {
    /// Makes the value (allocating, and charging memory, for strings and tables).
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error>;
}

/// A Rust type that can be read from a list of script values: a host function's arguments.
pub trait FromLuaMulti: Sized {
    /// Reads `args`. An [`Error::BadArgument`]'s position counts from 1 at `args[0]`.
    fn from_lua_multi(args: &[Value], lua: &mut Lua) -> Result<Self, Error>;
}

/// A Rust value that can become a list of script values: a host function's results.
pub trait IntoLuaMulti {
    /// Makes the values.
    fn into_lua_multi(self, lua: &mut Lua) -> Result<Vec<Value>, Error>;
}

/// Any number of values of one type: as arguments, the rest of them (it may end a tuple:
/// `(String, Variadic<i64>)`); as results, that many results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variadic<T>(pub Vec<T>);

impl<T> std::ops::Deref for Variadic<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Variadic<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T> From<Vec<T>> for Variadic<T> {
    fn from(v: Vec<T>) -> Self {
        Variadic(v)
    }
}

impl<T> IntoIterator for Variadic<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<T> FromIterator<T> for Variadic<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        Variadic(iter.into_iter().collect())
    }
}

/// `{what} expected, got {type of v}` (the type as `__name` names it, as C Lua's
/// `luaL_typeerror` does).
pub(crate) fn expected(lua: &Lua, what: &str, v: Value) -> Error {
    Error::BadArgument { position: 0, message: format!("{what} expected, got {}", lua.arg_type_name(v)) }
}

fn expected_missing(what: &str) -> Error {
    Error::BadArgument { position: 0, message: format!("{what} expected, got no value") }
}

/// The error of argument `index` (0-based) of a list, with its position filled in (offset
/// by `index` when the error came from a nested list).
fn at(index: usize, e: Error) -> Error {
    match e {
        Error::BadArgument { position, message } => {
            Error::BadArgument { position: index + position.max(1), message }
        }
        Error::Runtime { message, .. } => Error::BadArgument { position: index + 1, message },
        Error::Host(message) => Error::BadArgument { position: index + 1, message },
        other => other,
    }
}

fn read_arg<T: FromLua>(args: &[Value], i: usize, lua: &mut Lua) -> Result<T, Error> {
    match args.get(i) {
        Some(&v) => T::from_lua(v, lua),
        None => T::from_missing(lua),
    }
    .map_err(|e| at(i, e))
}

// ---- FromLua ----

impl FromLua for Value {
    fn from_lua(v: Value, _lua: &mut Lua) -> Result<Self, Error> {
        Ok(v)
    }
}

impl FromLua for bool {
    /// Lua's truth: everything but `nil` and `false` is true (`lua_toboolean`).
    fn from_lua(v: Value, _lua: &mut Lua) -> Result<Self, Error> {
        Ok(v.truthy())
    }
}

impl<T: FromLua> FromLua for Option<T> {
    fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error> {
        if v.is_nil() { Ok(None) } else { T::from_lua(v, lua).map(Some) }
    }

    fn from_missing(_lua: &mut Lua) -> Result<Self, Error> {
        Ok(None)
    }
}

/// `luaL_checkinteger`: an integer, a float with an exact integer value, or a string that
/// converts to one.
fn check_integer(v: Value, lua: &Lua) -> Result<i64, Error> {
    if let Some(i) = lua.tointeger(v) {
        return Ok(i);
    }
    // A number (or numeric string) that is not integral says so, as C Lua does.
    if lua.tonumber(v).is_some() {
        return Err(Error::BadArgument {
            position: 0,
            message: "number has no integer representation".into(),
        });
    }
    Err(expected(lua, "number", v))
}

macro_rules! from_lua_int {
    ($($t:ty),*) => {$(
        impl FromLua for $t {
            fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error> {
                let i = check_integer(v, lua)?;
                <$t>::try_from(i).map_err(|_| Error::BadArgument {
                    position: 0,
                    message: format!("value out of range for {}", stringify!($t)),
                })
            }

            fn from_missing(_lua: &mut Lua) -> Result<Self, Error> {
                Err(expected_missing("number"))
            }
        }
    )*};
}
from_lua_int!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

macro_rules! from_lua_float {
    ($($t:ty),*) => {$(
        impl FromLua for $t {
            fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error> {
                match lua.tonumber(v) {
                    Some(Value::Int(i)) => Ok(i as $t),
                    Some(Value::Num(f)) => Ok(f as $t),
                    _ => Err(expected(lua, "number", v)),
                }
            }

            fn from_missing(_lua: &mut Lua) -> Result<Self, Error> {
                Err(expected_missing("number"))
            }
        }
    )*};
}
from_lua_float!(f32, f64);

impl FromLua for String {
    /// A string (it must be UTF-8), or a number as `tostring` writes it. Charges the fuel
    /// of copying the bytes.
    fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error> {
        match v {
            Value::Str(_) => {
                let n = lua.str_bytes(v).map_or(0, <[u8]>::len);
                lua.charge_copy(n)?;
                let b = lua.str_bytes(v).unwrap_or_default();
                match std::str::from_utf8(b) {
                    Ok(s) => Ok(s.to_owned()),
                    Err(_) => {
                        Err(Error::BadArgument { position: 0, message: "string is not valid UTF-8".into() })
                    }
                }
            }
            Value::Int(i) => Ok(i.to_string()),
            Value::Num(f) => Ok(crate::number::fmt_float(f)),
            _ => Err(expected(lua, "string", v)),
        }
    }

    fn from_missing(_lua: &mut Lua) -> Result<Self, Error> {
        Err(expected_missing("string"))
    }
}

impl FromLua for TableRef {
    fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error> {
        match v {
            Value::Table(t) => Ok(t),
            _ => Err(expected(lua, "table", v)),
        }
    }

    fn from_missing(_lua: &mut Lua) -> Result<Self, Error> {
        Err(expected_missing("table"))
    }
}

impl FromLua for FuncRef {
    fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error> {
        match v {
            Value::Func(f) => Ok(f),
            _ => Err(expected(lua, "function", v)),
        }
    }

    fn from_missing(_lua: &mut Lua) -> Result<Self, Error> {
        Err(expected_missing("function"))
    }
}

impl FromLua for Data {
    /// [`Lua::to_data`] under the default [`DataLimits`], charging the fuel of what it
    /// copied (each value as 16 bytes, plus the string bytes): each copy is bounded by the
    /// limits, but a `Variadic<Data>` makes as many as the script passes.
    fn from_lua(v: Value, lua: &mut Lua) -> Result<Self, Error> {
        let d = lua.to_data(v, DataLimits::default())?;
        lua.charge_copy(data_bytes(&d))?;
        Ok(d)
    }
}

/// What copying `d` out moved: 16 bytes a value, plus its strings' bytes.
fn data_bytes(d: &Data) -> usize {
    let mut n = 0usize;
    let mut stack = vec![d];
    while let Some(d) = stack.pop() {
        n = n.saturating_add(16);
        match d {
            Data::Str(s) => n = n.saturating_add(s.len()),
            Data::Table(kv) => stack.extend(kv.iter().flat_map(|(k, v)| [k, v])),
            _ => {}
        }
    }
    n
}

// ---- IntoLua ----

impl IntoLua for Value {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(self)
    }
}

impl IntoLua for bool {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(Value::Bool(self))
    }
}

macro_rules! into_lua_int {
    ($($t:ty),*) => {$(
        impl IntoLua for $t {
            fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
                Ok(Value::Int(i64::from(self)))
            }
        }
    )*};
}
into_lua_int!(i8, i16, i32, u8, u16, u32);

impl IntoLua for i64 {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(Value::Int(self))
    }
}

macro_rules! into_lua_wide_int {
    ($($t:ty),*) => {$(
        impl IntoLua for $t {
            /// An integer; one past `i64`'s range is an error rather than a wrapped value.
            fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
                i64::try_from(self)
                    .map(Value::Int)
                    .map_err(|_| Error::runtime(format!("{self} does not fit a Lua integer")))
            }
        }
    )*};
}
into_lua_wide_int!(u64, usize, isize);

impl IntoLua for f32 {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(Value::Num(f64::from(self)))
    }
}

impl IntoLua for f64 {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(Value::Num(self))
    }
}

impl IntoLua for &str {
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error> {
        lua.create_string(self)
    }
}

impl IntoLua for String {
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error> {
        lua.create_string(self)
    }
}

impl IntoLua for &String {
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error> {
        lua.create_string(self)
    }
}

impl IntoLua for &[u8] {
    /// A string of these bytes.
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error> {
        lua.create_string(self)
    }
}

impl<T: IntoLua> IntoLua for Option<T> {
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error> {
        match self {
            Some(x) => x.into_lua(lua),
            None => Ok(Value::Nil),
        }
    }
}

impl IntoLua for TableRef {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(Value::Table(self))
    }
}

impl IntoLua for FuncRef {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(Value::Func(self))
    }
}

impl IntoLua for &Root {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(self.value())
    }
}

impl IntoLua for Root {
    fn into_lua(self, _lua: &mut Lua) -> Result<Value, Error> {
        Ok(self.value())
    }
}

impl IntoLua for &Data {
    /// [`Lua::from_data`].
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error> {
        lua.from_data(self)
    }
}

impl IntoLua for Data {
    fn into_lua(self, lua: &mut Lua) -> Result<Value, Error> {
        lua.from_data(&self)
    }
}

// ---- Multi ----

impl<T: FromLua> FromLuaMulti for T {
    fn from_lua_multi(args: &[Value], lua: &mut Lua) -> Result<Self, Error> {
        read_arg(args, 0, lua)
    }
}

impl FromLuaMulti for () {
    fn from_lua_multi(_args: &[Value], _lua: &mut Lua) -> Result<Self, Error> {
        Ok(())
    }
}

impl<T: FromLua> FromLuaMulti for Variadic<T> {
    fn from_lua_multi(args: &[Value], lua: &mut Lua) -> Result<Self, Error> {
        let mut out = Vec::with_capacity(args.len());
        for (i, &v) in args.iter().enumerate() {
            out.push(T::from_lua(v, lua).map_err(|e| at(i, e))?);
        }
        Ok(Variadic(out))
    }
}

impl<T: IntoLua> IntoLuaMulti for T {
    fn into_lua_multi(self, lua: &mut Lua) -> Result<Vec<Value>, Error> {
        Ok(vec![self.into_lua(lua)?])
    }
}

impl IntoLuaMulti for () {
    fn into_lua_multi(self, _lua: &mut Lua) -> Result<Vec<Value>, Error> {
        Ok(Vec::new())
    }
}

impl<T: IntoLua> IntoLuaMulti for Variadic<T> {
    fn into_lua_multi(self, lua: &mut Lua) -> Result<Vec<Value>, Error> {
        self.0.into_iter().map(|x| x.into_lua(lua)).collect()
    }
}

impl IntoLuaMulti for Vec<Value> {
    /// The values as they are (what [`Lua::create_function`]'s functions return).
    fn into_lua_multi(self, _lua: &mut Lua) -> Result<Vec<Value>, Error> {
        Ok(self)
    }
}

// Tuples: every element but the last is one argument; the last takes the rest (so it may
// be a `Variadic`). As results, one value each.
macro_rules! tuples {
    ($( ($($a:ident $i:tt),* ; $z:ident $zi:tt) )*) => {$(
        impl<$($a: FromLua,)* $z: FromLuaMulti> FromLuaMulti for ($($a,)* $z,) {
            fn from_lua_multi(args: &[Value], lua: &mut Lua) -> Result<Self, Error> {
                Ok((
                    $(read_arg::<$a>(args, $i, lua)?,)*
                    $z::from_lua_multi(args.get($zi..).unwrap_or_default(), lua).map_err(|e| at($zi, e))?,
                ))
            }
        }

        impl<$($a: IntoLua,)* $z: IntoLua> IntoLuaMulti for ($($a,)* $z,) {
            #[allow(non_snake_case)]
            fn into_lua_multi(self, lua: &mut Lua) -> Result<Vec<Value>, Error> {
                let ($($a,)* $z,) = self;
                Ok(vec![$($a.into_lua(lua)?,)* $z.into_lua(lua)?])
            }
        }
    )*};
}

tuples! {
    (; A 0)
    (A 0; B 1)
    (A 0, B 1; C 2)
    (A 0, B 1, C 2; D 3)
    (A 0, B 1, C 2, D 3; E 4)
    (A 0, B 1, C 2, D 3, E 4; F 5)
    (A 0, B 1, C 2, D 3, E 4, F 5; G 6)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6; H 7)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7; I 8)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8; J 9)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9; K 10)
    (A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10; L 11)
}
