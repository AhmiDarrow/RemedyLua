//! Programs nested 10k and 100k deep (100k is 3x past the old
//! 32,767 cap and far past C Lua's 200 levels): each compiles and runs to the expected result,
//! and agrees with C Lua wherever C Lua manages that depth (elsewhere C Lua must stop with
//! one of its implementation-limit errors, the superset's allowed difference).

use std::fmt::Write;
use std::sync::Mutex;

use remedy_lua::{Libs, Limits, Lua};

/// The nesting shapes.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Parens,
    Neg,
    Not,
    BNot,
    Concat,
    Pow,
    Do,
    If,
    While,
    Repeat,
    Function,
    Table,
    Args,
    Calls,
    Methods,
    Fields,
    Elseif,
    OrChain,
    AddChain,
    /// RemedyLua only: nested interpolated strings.
    Interp,
    /// RemedyLua only: nested type annotations.
    Types,
}

pub const ALL: &[Kind] = &[
    Kind::Parens,
    Kind::Neg,
    Kind::Not,
    Kind::BNot,
    Kind::Concat,
    Kind::Pow,
    Kind::Do,
    Kind::If,
    Kind::While,
    Kind::Repeat,
    Kind::Function,
    Kind::Table,
    Kind::Args,
    Kind::Calls,
    Kind::Methods,
    Kind::Fields,
    Kind::Elseif,
    Kind::OrChain,
    Kind::AddChain,
    Kind::Interp,
    Kind::Types,
];

/// A program `n` deep of `kind` and what it returns (as `tostring` prints it).
pub fn program(kind: Kind, n: usize) -> (String, String) {
    let mut s = String::new();
    let rep = |s: &mut String, t: &str, k: usize| {
        s.reserve(t.len() * k);
        for _ in 0..k {
            s.push_str(t);
        }
    };
    let want = match kind {
        Kind::Parens => {
            s.push_str("return ");
            rep(&mut s, "(", n);
            s.push('1');
            rep(&mut s, ")", n);
            "1".to_owned()
        }
        Kind::Neg => {
            s.push_str("local x = 3 return ");
            rep(&mut s, "- ", n);
            s.push('x');
            if n % 2 == 0 { "3" } else { "-3" }.to_owned()
        }
        Kind::Not => {
            s.push_str("return ");
            rep(&mut s, "not ", n);
            s.push_str("false");
            (n % 2 == 1).to_string()
        }
        Kind::BNot => {
            s.push_str("local x = 5 return ");
            rep(&mut s, "~ ", n);
            s.push('x');
            if n % 2 == 0 { "5" } else { "-6" }.to_owned()
        }
        Kind::Concat => {
            s.push_str("local a = 'a' return #(a");
            rep(&mut s, " .. a", n);
            s.push(')');
            (n + 1).to_string()
        }
        Kind::Pow => {
            s.push_str("local a = 1 return a");
            rep(&mut s, " ^ a", n);
            "1.0".to_owned()
        }
        Kind::Do => {
            s.push_str("local x = 0 ");
            rep(&mut s, "do ", n);
            s.push_str("x = x + 1 ");
            rep(&mut s, "end ", n);
            s.push_str("return x");
            "1".to_owned()
        }
        Kind::If => {
            s.push_str("local x = 0 ");
            rep(&mut s, "if x >= 0 then ", n);
            s.push_str("x = 7 ");
            rep(&mut s, "end ", n);
            s.push_str("return x");
            "7".to_owned()
        }
        Kind::While => {
            s.push_str("local c = 0 ");
            rep(&mut s, "while true do c = c + 1 ", n);
            rep(&mut s, "break end ", n);
            s.push_str("return c");
            n.to_string()
        }
        Kind::Repeat => {
            s.push_str("local c = 0 ");
            rep(&mut s, "repeat c = c + 1 ", n);
            rep(&mut s, "until true ", n);
            s.push_str("return c");
            n.to_string()
        }
        Kind::Function => {
            // Every level captures `a` from the outermost function: an n-long upvalue chain.
            s.push_str("local a = 7 local f = ");
            rep(&mut s, "function() return ", n);
            s.push('a');
            rep(&mut s, " end", n);
            let _ = write!(s, " for _ = 1, {n} do f = f() end return f");
            "7".to_owned()
        }
        Kind::Table => {
            s.push_str("local t = ");
            rep(&mut s, "{", n);
            s.push('1');
            rep(&mut s, "}", n);
            s.push_str(" local d = 0 while type(t) == 'table' do t = t[1] d = d + 1 end return d");
            n.to_string()
        }
        Kind::Args => {
            s.push_str("local function f(x) return x end return ");
            rep(&mut s, "f(", n);
            s.push('1');
            rep(&mut s, ")", n);
            "1".to_owned()
        }
        Kind::Calls => {
            s.push_str("local function f() return f end return f");
            rep(&mut s, "()", n);
            s.push_str(" == f");
            "true".to_owned()
        }
        Kind::Methods => {
            s.push_str("local o = {} function o:m() return self end return o");
            rep(&mut s, ":m()", n);
            s.push_str(" == o");
            "true".to_owned()
        }
        Kind::Fields => {
            s.push_str("local t = {} t.t = t return t");
            rep(&mut s, ".t", n);
            s.push_str(" == t");
            "true".to_owned()
        }
        Kind::Elseif => {
            let _ = write!(s, "local x = {} local r if x == 0 then r = 0 ", n - 1);
            for i in 1..n {
                let _ = write!(s, "elseif x == {i} then r = {i} ");
            }
            s.push_str("end return r");
            (n - 1).to_string()
        }
        Kind::OrChain => {
            s.push_str("local f = false return f");
            rep(&mut s, " or f", n);
            s.push_str(" or 9");
            "9".to_owned()
        }
        Kind::AddChain => {
            s.push_str("local a = 1 return a");
            rep(&mut s, " + a", n);
            (n + 1).to_string()
        }
        Kind::Interp => {
            s.push_str("return #");
            rep(&mut s, "`a{", n);
            s.push('1');
            rep(&mut s, "}`", n);
            (n + 1).to_string()
        }
        Kind::Types => {
            s.push_str("local x: ");
            rep(&mut s, "{", n);
            s.push_str("number");
            rep(&mut s, "}", n);
            s.push_str(" = 5 return x");
            "5".to_owned()
        }
    };
    (s, want)
}

/// The chunk name `kind` is written for (its dialect).
pub fn chunk(kind: Kind) -> &'static str {
    if matches!(kind, Kind::Interp | Kind::Types) { "=t.rlua" } else { "=t" }
}

/// Runs `src` without budgets: the first result as `tostring` prints it, or the error.
pub fn run(src: &str, name: &str) -> Result<String, String> {
    let mut lua = Lua::new(Limits::unmetered(), Libs::ALL);
    let v = lua.exec(src, name).map_err(|e| e.to_string())?;
    Ok(v.first().map_or_else(|| "nil".to_owned(), |x| crate::common::show(&lua, *x)))
}

/// One deep program at a time past 10k levels: they take hundreds of megabytes each.
pub static BIG: Mutex<()> = Mutex::new(());

pub fn check(kind: Kind, n: usize) {
    let _big = (n > 10_000).then(|| (crate::common::memory_heavy(), BIG.lock().unwrap_or_else(|e| e.into_inner())));
    let (src, want) = program(kind, n);
    let ours = run(&src, chunk(kind));
    assert_eq!(ours.as_deref(), Ok(want.as_str()), "{kind:?} {n} deep");
    if chunk(kind) == "=t" {
        match remedy_lua_oracle::run(&src, u64::MAX) {
            Ok(theirs) => assert_eq!(theirs, want, "C Lua differs on {kind:?} {n} deep"),
            Err(e) => assert!(
                crate::common::C_LIMIT_ERRORS.iter().any(|m| e.contains(m)),
                "C Lua failed {kind:?} {n} deep with a non-limit error: {e}"
            ),
        }
    }
}

/// Whether `kind` at depth `n` still fails today (its tests are ignored until the rewrite
/// lands). Keep in step with the groups in `deep_tests!` below.
pub fn pending(kind: Kind, n: usize) -> bool {
    use Kind::*;
    // Each nested interpolation is lexed again from its own text: time and memory grow
    // with the square of the depth, so 100k levels are left out (10k run).
    n > 10_000 && matches!(kind, Interp)
}

macro_rules! deep_tests {
    (ok: [$($ok:ident: $okk:ident),* $(,)?],
     shallow_only: [$($so:ident: $sok:ident),* $(,)?],
     pending: [$($p:ident: $pk:ident),* $(,)?] $(,)?) => {
        mod d10k {
            $( #[test] fn $ok() { super::check(super::Kind::$okk, 10_000); } )*
            $( #[test] fn $so() { super::check(super::Kind::$sok, 10_000); } )*
            $( #[test] #[ignore = "awaits iterative front end"] fn $p() { super::check(super::Kind::$pk, 10_000); } )*
        }
        mod d100k {
            $( #[test] fn $ok() { super::check(super::Kind::$okk, 100_000); } )*
            $( #[test] #[ignore = "awaits iterative front end"] fn $so() { super::check(super::Kind::$sok, 100_000); } )*
            $( #[test] #[ignore = "awaits iterative front end"] fn $p() { super::check(super::Kind::$pk, 100_000); } )*
        }
    };
}

deep_tests! {
    ok: [
        parens: Parens,
        do_blocks: Do,
        if_blocks: If,
        while_blocks: While,
        repeat_blocks: Repeat,
        functions: Function,
        calls: Calls,
        methods: Methods,
        elseif_chain: Elseif,
        or_chain: OrChain,
        add_chain: AddChain,
        types: Types,
        neg: Neg,
        not: Not,
        bnot: BNot,
        fields: Fields,
        concat: Concat,
        pow: Pow,
        tables: Table,
        args: Args,
    ],
    shallow_only: [
        interp: Interp,
    ],
    pending: [],
}
