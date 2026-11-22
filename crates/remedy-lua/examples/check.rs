//! Prints what `remedy_lua::check` says about a file (`.rlua` files are type-checked).
//!
//! `cargo run -p remedy-lua --example check -- file.rlua [more files]`, or `-e 'code'`
//! (checked as `.rlua`).
#![allow(clippy::unwrap_used)]

fn main() {
    let mut args = std::env::args().skip(1);
    let mut status = 0;
    while let Some(a) = args.next() {
        let (src, name) = if a == "-e" {
            (args.next().unwrap_or_default().into_bytes(), "=e.rlua".to_owned())
        } else {
            (std::fs::read(&a).unwrap(), a)
        };
        let t = std::time::Instant::now();
        let diags = remedy_lua::check(&src, &name);
        for d in &diags {
            println!("{d}  [{}:{}-{}:{}]", d.pos.line, d.pos.col, d.end.line, d.end.col);
        }
        eprintln!("{name}: {} diagnostic(s) in {:?}", diags.len(), t.elapsed());
        if !diags.is_empty() {
            status = 1;
        }
    }
    std::process::exit(status);
}
