//! Conformance: Lua programs written from the Lua 5.4 reference manual, section by
//! section (`tests/lua/conformance/*.lua`). Each must run without error in RemedyLua and
//! give the same result as the reference C Lua.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs)]

use super::common;

#[test]
fn conformance_suite() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua/conformance");
    let mut files: Vec<_> =
        std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    let mut failures = Vec::new();
    let mut count = 0;
    for f in files.iter().filter(|p| p.extension().is_some_and(|x| x == "lua")) {
        let src = std::fs::read_to_string(f).unwrap().replace("\r\n", "\n");
        // Each file is split into cases at lines starting with "-- case:".
        let mut cases: Vec<(String, String)> = Vec::new();
        for chunk in src.split("\n-- case:") {
            let (name, body) = chunk.split_once('\n').unwrap_or((chunk, ""));
            if body.trim().is_empty() {
                continue;
            }
            cases.push((name.trim().to_owned(), body.to_owned()));
        }
        for (name, body) in cases {
            count += 1;
            let ours = common::remedy(&body, 50_000_000);
            let label = format!("{}: {name}", f.file_name().unwrap().to_string_lossy());
            match &ours {
                Ok(s) if s.starts_with("error:") || s == "syntax error" => {
                    if !body.contains("-- expect error") && !name.contains("expect error") {
                        failures.push(format!("{label}: {s}"));
                    }
                }
                Err(e) => failures.push(format!("{label}: host error {e}")),
                _ => {}
            }
            let theirs = common::oracle(&body, 50_000_000);
            if theirs != ours && !common::c_limit_only_outcome(&ours, &theirs) {
                failures.push(format!("{label}: differs\n  remedy: {ours:?}\n  c lua:  {theirs:?}"));
            }
        }
    }
    println!("{count} conformance cases");
    assert!(failures.is_empty(), "{} of {count} failed:\n{}", failures.len(), failures.join("\n"));
}
