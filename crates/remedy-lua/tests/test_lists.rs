//! The guard on `tests/timing.list`, which tools/gate.sh splits the suite by: the tests
//! whose result can depend on time run serially on a quiet machine, the rest in parallel.
//! This keeps the list true as tests are added: every name in it is a test, and every
//! test that reads a clock is in one of its sections. Deterministic: it reads the test
//! sources, runs no Lua.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The list's sections, by name, in file order.
fn sections() -> BTreeMap<String, Vec<String>> {
    let text = include_str!("timing.list");
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cur = None;
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            cur = Some(name.to_owned());
            out.entry(name.to_owned()).or_default();
        } else {
            let sec = cur.clone().unwrap_or_else(|| panic!("timing.list: {line:?} before any [section]"));
            out.entry(sec).or_default().push(line.to_owned());
        }
    }
    out
}

/// One function of a test source file.
struct Func {
    /// The test's name as `--list` prints it (`module::fn`), for a `#[test]`.
    test: Option<String>,
    /// The module segment it belongs to (`sweep3_sec`, `harness`, `common`).
    seg: String,
    name: String,
    body: String,
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// The module path of a file under tests/ (`sweep6.rs` is `sweep6`, `front_proof/mod.rs`
/// is `front_proof`, `front_proof/equiv.rs` is `front_proof::equiv`).
fn module_path(rel: &Path) -> Vec<String> {
    let mut parts: Vec<String> = rel.iter().map(|c| c.to_string_lossy().into_owned()).collect();
    let last = parts.pop().unwrap();
    let stem = last.trim_end_matches(".rs");
    if stem != "mod" {
        parts.push(stem.to_owned());
    }
    parts
}

/// The name of a top-level `fn` declared on `line` (column 0, any visibility).
fn fn_name(line: &str) -> Option<&str> {
    let rest = ["fn ", "pub fn ", "pub(crate) fn ", "pub(super) fn "].iter().find_map(|p| line.strip_prefix(p))?;
    let end = rest.find(|c: char| !(c.is_alphanumeric() || c == '_'))?;
    Some(&rest[..end])
}

/// Every top-level function of every test file, and the `use` imports of each file
/// (a bare name to the module segment it comes from).
fn functions() -> (Vec<Func>, BTreeMap<String, BTreeMap<String, String>>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = Vec::new();
    rs_files(&root, &mut files);
    let mut funcs = Vec::new();
    let mut imports: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for path in files {
        let rel = path.strip_prefix(&root).unwrap();
        if rel.starts_with("all") {
            continue;
        }
        let modpath = module_path(rel);
        let seg = modpath.last().unwrap().clone();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let uses = imports.entry(seg.clone()).or_default();
        for line in &lines {
            // `use super::sweep5b_open::{ns_per_unit, baseline};`, `use crate::x::y::f;`
            if let Some(u) = line.trim().strip_prefix("use ").and_then(|u| u.strip_suffix(';')) {
                let (path, names) = match u.split_once("::{") {
                    Some((p, n)) => (p, n.trim_end_matches('}').split(',').map(str::trim).collect::<Vec<_>>()),
                    None => match u.rsplit_once("::") {
                        Some((p, n)) => (p, vec![n.trim()]),
                        None => continue,
                    },
                };
                let from = path.rsplit("::").next().unwrap_or(path).to_owned();
                for n in names {
                    let n = n.split(" as ").last().unwrap_or(n).trim();
                    if !n.is_empty() && n != "self" && n != "*" {
                        uses.insert(n.to_owned(), from.clone());
                    }
                }
            }
        }
        let mut i = 0;
        while i < lines.len() {
            let Some(name) = fn_name(lines[i]) else {
                i += 1;
                continue;
            };
            // Attributes and comments just above say whether it is a test.
            let mut is_test = false;
            let mut j = i;
            while j > 0 {
                let above = lines[j - 1].trim();
                if above.starts_with("#[") || above.starts_with("//") || above.ends_with(")]") || above.ends_with("\",") {
                    is_test |= above == "#[test]";
                    j -= 1;
                } else {
                    break;
                }
            }
            let end = (i + 1..lines.len()).find(|&k| lines[k] == "}").unwrap_or(lines.len() - 1);
            let body = code_only(&lines[i..=end].join("\n"));
            let test = is_test.then(|| format!("{}::{name}", modpath.join("::")));
            funcs.push(Func { test, seg: seg.clone(), name: name.to_owned(), body });
            i = end + 1;
        }
    }
    (funcs, imports)
}

/// `src` with the contents of string and char literals and of `//` comments blanked, so
/// Lua text inside a test (`probe(name, v)`) is not read as a call.
fn code_only(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'r' && matches!(b.get(i + 1), Some(b'"' | b'#')) && !b[..i].last().is_some_and(|p| p.is_ascii_alphanumeric() || *p == b'_') {
            // A raw string: r"..." or r#"..."#.
            let mut j = i + 1;
            let mut hashes = 0;
            while b.get(j) == Some(&b'#') {
                hashes += 1;
                j += 1;
            }
            if b.get(j) != Some(&b'"') {
                out.push(c as char);
                i += 1;
                continue;
            }
            j += 1;
            let close: Vec<u8> = std::iter::once(b'"').chain(std::iter::repeat_n(b'#', hashes)).collect();
            while j < b.len() && !b[j..].starts_with(&close) {
                j += 1;
            }
            out.push_str("\"\"");
            i = j + close.len();
        } else if c == b'"' {
            let mut j = i + 1;
            while j < b.len() && b[j] != b'"' {
                j += if b[j] == b'\\' { 2 } else { 1 };
            }
            out.push_str("\"\"");
            i = j + 1;
        } else if c == b'\'' && b.get(i + 2) == Some(&b'\'') {
            out.push_str("' '");
            i += 3;
        } else if c == b'\'' && b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'') {
            out.push_str("' '");
            i += 4;
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    out
}

/// What reads a clock directly.
const CLOCK: &[&str] = &["thread_cpu(", "Instant::now", ".elapsed(", "set_time_limit(", "recv_timeout("];

/// Whether `body` calls `name` as a function (`name(`, not `x_name(` or `.name(`).
fn calls(body: &str, name: &str) -> bool {
    let pat = format!("{name}(");
    body.match_indices(&pat).any(|(at, _)| {
        let before = body[..at].chars().next_back();
        !before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.')
    })
}

/// The functions that read a clock, directly or through the functions they call: a
/// fixpoint over the calls a body makes by bare name (resolved in its own module or
/// through its imports) or by path (`harness::with_timeout(`, by its last segment).
fn clock_readers(funcs: &[Func], imports: &BTreeMap<String, BTreeMap<String, String>>) -> BTreeMap<usize, String> {
    let mut readers: BTreeMap<usize, String> = (0..funcs.len())
        .filter_map(|i| CLOCK.iter().find(|c| funcs[i].body.contains(**c)).map(|c| (i, (*c).to_owned())))
        .collect();
    loop {
        let known: Vec<(String, String)> =
            readers.keys().map(|&i| (funcs[i].seg.clone(), funcs[i].name.clone())).collect();
        let mut grew = false;
        for (i, f) in funcs.iter().enumerate() {
            if readers.contains_key(&i) {
                continue;
            }
            let uses = imports.get(&f.seg);
            let via = known.iter().find(|(seg, name)| {
                // A qualified call names the module; a bare one must resolve to it.
                f.body.contains(&format!("{seg}::{name}("))
                    || (calls(&f.body, name)
                        && (*seg == f.seg || uses.and_then(|u| u.get(name)).is_some_and(|from| from == seg)))
            });
            if let Some((seg, name)) = via {
                readers.insert(i, format!("{seg}::{name}"));
                grew = true;
            }
        }
        if !grew {
            return readers;
        }
    }
}

/// Every name in the list is a test of this crate (`[oracle]` ones may be compiled only
/// with `--features front-oracle`: they are found in the sources all the same), no name
/// is in two classes, and `[env]` is part of `[timing]`.
#[test]
fn timing_list_names_are_tests() {
    let secs = sections();
    let (funcs, _) = functions();
    let tests: BTreeSet<&str> = funcs.iter().filter_map(|f| f.test.as_deref()).collect();
    let mut missing = Vec::new();
    for (sec, names) in &secs {
        for n in names {
            // A [known] entry is `name FINDING`.
            let n = n.split_whitespace().next().unwrap_or("");
            if !tests.contains(n) {
                missing.push(format!("[{sec}] {n}"));
            }
        }
    }
    assert!(missing.is_empty(), "timing.list names no test: {missing:?}");
    let classes = ["timing", "clock-deterministic", "hang-guard", "time-capped", "probe"];
    let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
    for c in classes {
        for n in secs.get(c).into_iter().flatten() {
            if let Some(other) = seen.insert(n.as_str(), c) {
                panic!("timing.list: {n} is in both [{other}] and [{c}]");
            }
        }
    }
    let timing: BTreeSet<&String> = secs.get("timing").into_iter().flatten().collect();
    let not_timing: Vec<&String> = secs.get("env").into_iter().flatten().filter(|n| !timing.contains(n)).collect();
    assert!(not_timing.is_empty(), "timing.list: [env] names not in [timing]: {not_timing:?}");
}

/// Every test that reads a clock, itself or through a helper, is in a class of the list:
/// a new timing test cannot land in the parallel steps unnoticed.
#[test]
fn every_test_that_reads_a_clock_is_listed() {
    let secs = sections();
    let (funcs, imports) = functions();
    let listed: BTreeSet<&str> = ["timing", "clock-deterministic", "hang-guard", "time-capped", "probe"]
        .iter()
        .flat_map(|c| secs.get(*c).into_iter().flatten())
        .map(String::as_str)
        .collect();
    let readers = clock_readers(&funcs, &imports);
    let unlisted: Vec<String> = readers
        .iter()
        .filter_map(|(&i, via)| funcs[i].test.as_deref().map(|t| (t, via)))
        .filter(|(t, _)| !listed.contains(t))
        .map(|(t, via)| format!("{t} (via {via})"))
        .collect();
    assert!(
        unlisted.is_empty(),
        "tests that read a clock but are in no class of tests/timing.list (add each to [timing] \
         if its result can depend on time, else to the class its asserts fit): {unlisted:#?}"
    );
}

/// Whether `id` is an issue still open in docs/known-issues.md: a `### ID: ... (open)`
/// heading, or a status-table row `| ID | ... |` whose status (its last cell) says open.
fn finding_open(id: &str) -> bool {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/known-issues.md");
    let mut open = false;
    {
        for line in std::fs::read_to_string(&p).unwrap().lines() {
            let l = line.trim();
            if l.starts_with(&format!("### {id}:")) && l.contains("(open)") {
                open = true;
            }
            if l.starts_with(&format!("| {id} |")) {
                let status = l.trim_end_matches('|').rsplit('|').next().unwrap_or("").to_lowercase();
                if status.contains("open") {
                    open = true;
                }
            }
        }
    }
    open
}

/// Every [known] entry (a test tied to an open issue, run and reported but
/// not failing its step) names a test and a finding that is still open. When the finding
/// closes, this fails until the entry goes, so the test counts again.
#[test]
fn known_entries_name_open_findings() {
    let secs = sections();
    let (funcs, _) = functions();
    let tests: BTreeSet<&str> = funcs.iter().filter_map(|f| f.test.as_deref()).collect();
    for entry in secs.get("known").into_iter().flatten() {
        let mut parts = entry.split_whitespace();
        let (Some(name), Some(id), None) = (parts.next(), parts.next(), parts.next()) else {
            panic!("timing.list [known]: {entry:?} is not `module::fn FINDING`");
        };
        assert!(tests.contains(name), "timing.list [known]: {name} is not a test");
        assert!(finding_open(id), "timing.list [known]: {name} names {id}, which docs/known-issues.md does not show open: remove the entry if {id} is closed");
    }
}
