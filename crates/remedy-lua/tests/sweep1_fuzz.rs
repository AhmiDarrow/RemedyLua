//! Sweep 1, area "fuzz": a second differential fuzzer, independent of `differential.rs`.
//!
//! - `fuzz_generated`: a typed grammar-based generator of terminating Lua 5.4 programs
//!   (`sweep1_fuzz/grammar.rs`) run in RemedyLua and C Lua inside the same wrapper; plus, per
//!   program, a tiny-fuel run (must stop with the fuel limit), a small-memory run (must stop
//!   with the memory limit or give the same result), and `format`/`check` (no panic; the
//!   formatted program behaves the same).
//! - `fuzz_mutated`: token/line mutations of the conformance cases and generated programs,
//!   valid or not (syntax-error messages are compared too).
//! - `fuzz_rlua`: RemedyLua-dialect programs (annotations, compound assignment, `continue`,
//!   interpolation) against their plain-Lua desugaring run in C Lua.
//!
//! Failing programs are minimized by delta debugging and written to `SWEEP1_OUT` (default:
//! the cargo test tmp dir). Confirmed ones are kept in `tests/lua/sweep1/fuzz/` and
//! replayed by the `#[ignore = "sweep1: ..."]` tests at the bottom.
//!
//! Env: `SWEEP1_ITERS` (programs per driver), `SWEEP1_SEED`, `SWEEP1_SECS` (time cap).
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic, missing_docs, dead_code)]

#[path = "sweep1_fuzz/grammar.rs"]
pub(crate) mod grammar;
#[path = "sweep1_fuzz/harness.rs"]
pub(crate) mod harness;
#[path = "sweep1_fuzz/mutate.rs"]
pub(crate) mod mutate;
#[path = "sweep1_fuzz/rng.rs"]
pub(crate) mod rng;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use harness::{Class, Outcome, compare};
use rng::Rng;

fn env_u64(k: &str, d: u64) -> u64 {
    std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d)
}

fn out_dir() -> PathBuf {
    let d = std::env::var("SWEEP1_OUT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("sweep1-fuzz"));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Collects findings: one minimized program per distinct signature (a few per category).
pub(crate) struct Recorder {
    area: &'static str,
    pub(crate) dir: PathBuf,
    pub(crate) sigs: BTreeMap<String, u64>,
    cats: BTreeMap<String, u64>,
    saved: usize,
    pub(crate) runs: u64,
    inconclusive: u64,
    classes: BTreeMap<String, u64>,
    /// Differences `harness::classify` found to be known non-bugs, by kind.
    pub(crate) expected: BTreeMap<String, u64>,
}

impl Recorder {
    pub(crate) fn new(area: &'static str) -> Recorder {
        Recorder {
            area,
            dir: out_dir(),
            sigs: BTreeMap::new(),
            cats: BTreeMap::new(),
            saved: 0,
            runs: 0,
            inconclusive: 0,
            classes: BTreeMap::new(),
            expected: BTreeMap::new(),
        }
    }

    fn current(&self, what: &str, src: &str) {
        let _ = std::fs::write(self.dir.join(format!("current-{}.txt", self.area)), format!("{what}\n{src}"));
    }

    fn class(&mut self, ours: &Outcome, theirs: &Outcome) {
        *self.classes.entry(format!("{:?}/{:?}", ours.class, theirs.class)).or_default() += 1;
    }

    /// True when this signature should be minimized and saved.
    fn wants(&mut self, sig: &str) -> bool {
        let cat = category(sig);
        let n = self.sigs.entry(sig.to_owned()).or_default();
        *n += 1;
        let c = self.cats.entry(cat).or_default();
        *c += 1;
        *n == 1 && *c <= 25 && self.saved < 200
    }

    fn save(&mut self, sig: &str, ext: &str, files: &[(&str, &str)], note: &str) {
        self.saved += 1;
        let id = format!("{}-{:03}", self.area, self.saved);
        let mut body = format!("-- sig: {sig}\n");
        for line in note.lines() {
            body.push_str(&format!("-- {line}\n"));
        }
        for (suffix, src) in files {
            let path = self.dir.join(format!("{id}{suffix}.{ext}"));
            std::fs::write(&path, format!("{body}{src}")).unwrap();
        }
        println!("FINDING {id}: {sig}");
    }

    /// A known non-bug (`harness::classify`): counted, and the first few of each kind kept
    /// for audit (`known-<kind>-<n>.lua`), not reported as findings.
    fn known(&mut self, kind: &str, src: &str, ours: &Outcome, theirs: &Outcome) {
        let n = self.expected.entry(kind.to_owned()).or_default();
        *n += 1;
        if *n <= 3 {
            let path = self.dir.join(format!("known-{}-{kind}-{n}.lua", self.area));
            let note: String = format!("remedy: {}\nc lua:  {}", ours.text, theirs.text)
                .lines()
                .map(|l| format!("-- {l}\n"))
                .collect();
            let _ = std::fs::write(path, format!("-- known non-bug: {kind}\n{note}{src}"));
        }
    }

    pub(crate) fn report(&self, started: Instant) {
        println!(
            "== {}: {} programs in {:.0}s, {} inconclusive (C Lua step limit)",
            self.area,
            self.runs,
            started.elapsed().as_secs_f64(),
            self.inconclusive
        );
        println!("outcome classes (remedy/c): {:?}", self.classes);
        for (c, n) in &self.cats {
            println!("category {c}: {n}");
        }
        for (k, n) in &self.expected {
            println!("known non-bug {k}: {n}");
        }
        for (s, n) in &self.sigs {
            println!("  {n:6}  {s}");
        }
        let _ = std::fs::write(
            self.dir.join(format!("summary-{}.txt", self.area)),
            format!(
                "{:?}\n{:#?}\n{:#?}\nknown non-bugs: {:#?}\n",
                self.classes, self.cats, self.sigs, self.expected
            ),
        );
    }
}

/// The part of a signature used to group findings: severity and outcome classes.
fn category(sig: &str) -> String {
    sig.splitn(3, ':').take(2).collect::<Vec<_>>().join(":")
}

/// Whether `cand` still shows the same kind of difference.
fn same_kind(orig: &str, now: Option<&String>) -> bool {
    match now {
        None => false,
        Some(s) => {
            if orig.starts_with("minor") || orig.starts_with("critical:panic") {
                s == orig
            } else {
                category(s) == category(orig)
            }
        }
    }
}

fn diff_plain(src: &str) -> (Option<String>, Outcome, Outcome) {
    let ours = harness::remedy(src.as_bytes(), "=t");
    let theirs = harness::oracle(src.as_bytes());
    (compare(&ours, &theirs), ours, theirs)
}

/// Shrinks `src` by lines, then by tokens, keeping `keep`.
fn minimize(src: &str, keep: &mut dyn FnMut(&str) -> bool) -> String {
    let mut budget = 500usize;
    let lines: Vec<String> = src.split('\n').map(str::to_owned).collect();
    let lines = mutate::ddmin(lines, &mut budget, &mut |ls: &[String]| keep(&ls.join("\n")));
    let s = lines.join("\n");
    let toks = mutate::tokenize(&s);
    let mut budget = 400usize;
    let toks = mutate::ddmin(toks, &mut budget, &mut |ts: &[mutate::Tok]| keep(&mutate::join(ts)));
    mutate::join(&toks)
}

/// The checks that only need RemedyLua: fuel and memory limits stop cleanly, `check` and
/// `format` don't panic and formatting keeps behavior.
fn remedy_only_checks(src: &str, name: &str, full: &Outcome, rng: &mut Rng, rec: &mut Recorder) {
    let used = harness::remedy_raw(src.as_bytes(), name, harness::remedy_limits()).1;
    // Tiny fuel: anything below what the run used must stop with the fuel limit.
    if used > 1 && full.class != Class::Fuel {
        // (Sweep 2: more cut points; each runs with the fast loop on and off.)
        let cuts: Vec<u64> = [0, 1, used - 1].into_iter().chain((0..10).map(|_| rng.next() % used)).collect();
        for f in cuts {
            let (o, _) = harness::remedy_raw(src.as_bytes(), name, harness::remedy_limits().fuel(f));
            if o.class != Class::Fuel {
                let sig = if o.text.starts_with("<panic> fast/slow differ") {
                    "critical:fast-slow-differ-at-fuel-cut".to_owned()
                } else {
                    format!("critical:fuel-escape:{:?}", o.class)
                };
                if rec.wants(&sig) {
                    rec.save(
                        &sig,
                        "lua",
                        &[("", src)],
                        &format!("fuel {f} of {used} used\nresult: {}", o.text),
                    );
                }
            }
        }
    }
    // Small memory: the same result, or the memory limit.
    for _ in 0..3 {
        let m = 1usize << (12 + rng.below(12));
        let m = m + rng.below(m);
        let (o, _) = harness::remedy_raw(src.as_bytes(), name, harness::remedy_limits().memory(m));
        if o.class != Class::Memory
            && o.text != full.text
            && full.class != Class::Fuel
            && !overflow_from_memory(&full.text, &o.text)
        {
            let sig = if o.text.starts_with("<panic> fast/slow differ") {
                "critical:fast-slow-differ-at-memory-cut".to_owned()
            } else {
                format!("critical:memory-escape:{:?}", o.class)
            };
            if rec.wants(&sig) {
                rec.save(
                    &sig,
                    "lua",
                    &[("", src)],
                    &format!("memory {m}\nfull: {}\nsmall: {}", full.text, o.text),
                );
            }
        }
    }
    // check/format.
    let s = src.to_owned();
    let n = name.trim_start_matches('=').to_owned();
    let r = harness::with_timeout(crate::common::SMALL_STACK, move || {
        std::panic::catch_unwind(|| {
            let _ = remedy_lua::check(s.as_bytes(), &n);
            remedy_lua::format(&s, &n).ok()
        })
    });
    match r {
        None => {
            let sig = "critical:format-timeout".to_owned();
            if rec.wants(&sig) {
                rec.save(&sig, "lua", &[("", src)], "");
            }
        }
        Some(Err(_)) => {
            let sig = "critical:check-or-format-panic".to_owned();
            if rec.wants(&sig) {
                rec.save(&sig, "lua", &[("", src)], "");
            }
        }
        Some(Ok(Some(f))) => {
            let o = harness::remedy(f.as_bytes(), name);
            if o.text != full.text && full.class != Class::Fuel && o.class != Class::Fuel {
                let sig = "major:format-changes-behavior".to_owned();
                if rec.wants(&sig) {
                    rec.save(
                        &sig,
                        "lua",
                        &[("", src), ("-formatted", &f)],
                        &format!("before: {}\nafter: {}", full.text, o.text),
                    );
                }
            }
        }
        Some(Ok(None)) => {}
    }
}

/// Whether a run under a smaller memory budget differs from the full run only from a
/// `stack overflow` on (a call whose frame would fill the budget is C Lua's
/// catchable `stack overflow`, not the memory limit): its first differing line has the
/// error and the full run's does not.
fn overflow_from_memory(full: &str, small: &str) -> bool {
    let (a, b): (Vec<&str>, Vec<&str>) = (full.lines().collect(), small.lines().collect());
    let at = (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i));
    // (Its mark from the wrapper, then the error, may come first.)
    at.is_some_and(|i| {
        // (The wrapper's marks for the limit errors `pcall`s saw may come first.)
        let first = b.iter().skip(i).find(|l| !l.contains(harness::C_LIMIT_MARK)).copied().unwrap_or("");
        let ai = a.get(i).copied().unwrap_or("");
        if first.contains("stack overflow") && !ai.contains("stack overflow") {
            return true;
        }
        // A `pcall` saw the overflow right there (its mark is the first difference) and
        // caught it: outer frames go on printing before the overflow reaches a segment
        // (long-run finding). Accepted when the overflow does show
        // up later and the full run never overflows at all.
        b.get(i).is_some_and(|l| l.contains(harness::C_LIMIT_MARK))
            && b.iter().skip(i).any(|l| l.contains("stack overflow"))
            && !a.iter().any(|l| l.contains("stack overflow"))
    })
}

pub(crate) fn handle_plain(src: &str, rec: &mut Recorder, rng: &mut Rng, extra: bool) {
    rec.runs += 1;
    rec.current("plain", src);
    let (sig, ours, theirs) = diff_plain(src);
    rec.class(&ours, &theirs);
    if theirs.class == Class::Fuel {
        rec.inconclusive += 1;
    }
    let sig = sig.map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), &ours, &theirs, s));
    if let Some(kind) = sig.as_deref().and_then(|s| s.strip_prefix("expected:")) {
        rec.known(kind, src, &ours, &theirs);
    } else if let Some(sig) = sig {
        if sig == "oracle-timeout" {
            rec.save(&sig, "lua", &[("", src)], "C Lua did not finish in time");
            return;
        }
        if sig == "critical:remedy-timeout" {
            // Not stopped: fuel ends the stray thread. Check it with a lower SWEEP1_FUEL.
            rec.save(&sig, "lua", &[("", src)], "RemedyLua did not finish within the timeout");
            return;
        }
        if rec.wants(&sig) {
            let min = minimize(src, &mut |c| same_kind(&sig, diff_plain(c).0.as_ref()));
            let (_, o2, t2) = diff_plain(&min);
            rec.save(&sig, "lua", &[("", &min)], &format!("remedy: {}\nc lua:  {}", o2.text, t2.text));
        }
    }
    if extra && ours.class != Class::Panic && ours.class != Class::Timeout {
        remedy_only_checks(src, "=t", &ours, rng, rec);
    }
}

fn budget() -> (u64, u64, Instant) {
    (env_u64("SWEEP1_ITERS", 200), env_u64("SWEEP1_SECS", 600), Instant::now())
}

#[test]
#[ignore = "sweep1: fuzz driver (long); run with --ignored, SWEEP1_ITERS/SWEEP1_SEED/SWEEP1_SECS"]
fn fuzz_generated() {
    let (iters, secs, started) = budget();
    let seed = env_u64("SWEEP1_SEED", 1);
    let mut rec = Recorder::new("gen");
    let mut cov = grammar::Coverage::default();
    for i in 0..iters {
        if started.elapsed().as_secs() > secs {
            break;
        }
        let mut rng = Rng::new(seed.wrapping_mul(1_000_003).wrapping_add(i));
        let mut g = grammar::Gen::new(&mut rng, false);
        let p = if i % 20 == 19 { g.deep() } else { g.program(10 + (i % 40) as i32) };
        for (k, v) in &g.cov.forms {
            *cov.forms.entry(k).or_default() += v;
        }
        let mut r2 = Rng::new(seed ^ i);
        handle_plain(&p.l, &mut rec, &mut r2, true);
    }
    rec.report(started);
    let covtxt: String = cov.forms.iter().map(|(k, v)| format!("{v:8} {k}\n")).collect();
    let _ = std::fs::write(rec.dir.join("coverage-gen.txt"), &covtxt);
    println!("coverage:\n{covtxt}");
}

/// The conformance cases, split at `-- case:` lines.
pub(crate) fn conformance_corpus() -> Vec<String> {
    let mut out = vec![];
    for sub in ["tests/lua/conformance", "tests/lua/sweep1/spec"] {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(sub);
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut files: Vec<_> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        files.sort();
        for f in files.iter().filter(|p| p.extension().is_some_and(|x| x == "lua")) {
            let src = std::fs::read_to_string(f).unwrap().replace("\r\n", "\n");
            for chunk in src.split("\n-- case:") {
                let body = chunk.split_once('\n').map_or("", |x| x.1);
                if !body.trim().is_empty() {
                    out.push(body.to_owned());
                }
            }
        }
    }
    out
}

#[test]
#[ignore = "sweep1: fuzz driver (long); run with --ignored, SWEEP1_ITERS/SWEEP1_SEED/SWEEP1_SECS"]
fn fuzz_mutated() {
    let (iters, secs, started) = budget();
    let seed = env_u64("SWEEP1_SEED", 1);
    let mut corpus = conformance_corpus();
    println!("{} conformance cases in the corpus", corpus.len());
    let mut rng = Rng::new(seed ^ 0xABCDEF);
    for i in 0..200 {
        let mut r = Rng::new(seed.wrapping_add(7_000_000 + i));
        let mut g = grammar::Gen::new(&mut r, false);
        corpus.push(g.program(8).l);
    }
    let mut rec = Recorder::new("mut");
    for _ in 0..iters {
        if started.elapsed().as_secs() > secs {
            break;
        }
        let base = corpus[rng.below(corpus.len())].clone();
        let m = mutate::mutate(&base, &mut rng, &corpus);
        let mut r2 = Rng::new(rng.next());
        handle_plain(&m, &mut rec, &mut r2, rng.pct(20));
    }
    rec.report(started);
}

fn handle_rlua(r: &str, l: &str, rec: &mut Recorder, rng: &mut Rng) {
    rec.runs += 1;
    rec.current("rlua", &format!("{r}\n----- plain:\n{l}"));
    let diff = |r: &str, l: &str| -> (Option<String>, Outcome, Outcome, Outcome) {
        let ours_r = harness::remedy(r.as_bytes(), "=t.rlua");
        let ours_l = harness::remedy(l.as_bytes(), "=t");
        let theirs = harness::oracle(l.as_bytes());
        let sig = compare(&ours_r, &theirs).map(|s| {
            // Dialect-specific when RemedyLua's own plain run agrees with C Lua.
            if ours_l.text == theirs.text { format!("{s} [rlua-only]") } else { format!("{s} [also plain]") }
        });
        (sig, ours_r, ours_l, theirs)
    };
    let (sig, ours, ours_l, theirs) = diff(r, l);
    rec.class(&ours, &theirs);
    if theirs.class == Class::Fuel {
        rec.inconclusive += 1;
    }
    // A plain twin both engines reject while the `.rlua` program compiles is the
    // generator's bug, not the engines': kept as generated, never classified or minimized
    // (the minimizer drops lines of both sides together, so it could drop a `::contN::`
    // line and save a twin that fails with "no visible label 'contN'" instead).
    if theirs.class == Class::Syntax && ours_l.class == Class::Syntax && ours.class != Class::Syntax {
        let sig = format!("generator:invalid-twin:{}", ours_l.text.lines().last().unwrap_or(""));
        if rec.wants(&sig) {
            rec.save(
                &sig,
                "rlua",
                &[("", r), ("-plain", l)],
                &format!(
                    "remedy .rlua: {}\nremedy .lua:  {}\nc lua .lua:   {}",
                    ours.text, ours_l.text, theirs.text
                ),
            );
        }
        return;
    }
    // (Classified on the plain source, which C Lua runs: the order runs use RemedyLua's
    // `.rlua` run of it.)
    let sig = sig.map(|s| {
        let (tag, base) =
            s.rsplit_once(" [").map_or((String::new(), s.clone()), |(b, t)| (format!(" [{t}"), b.to_owned()));
        let c = harness::classify(r.as_bytes(), "=t.rlua", l.as_bytes(), &ours, &theirs, base);
        if c.starts_with("expected:") { c } else { format!("{c}{tag}") }
    });
    if let Some(kind) = sig.as_deref().and_then(|s| s.strip_prefix("expected:")) {
        rec.known(kind, r, &ours, &theirs);
    } else if let Some(sig) = sig {
        if sig.starts_with("critical:remedy-timeout") {
            rec.save(&sig, "rlua", &[("", r)], "RemedyLua did not finish within the timeout");
            return;
        }
        // Plain-Lua bugs are fuzz_generated's job; keep only a few of them here.
        let rl_only = sig.ends_with("[rlua-only]");
        if (rl_only || rec.sigs.len() < 3) && rec.wants(&sig) {
            // The program as generated, before the minimizer touches it (a reduction can
            // change what the program means, so the original is the evidence).
            let n = rec.saved + 1;
            let _ = std::fs::write(rec.dir.join(format!("rlua-{n:03}-orig.rlua")), r);
            let _ = std::fs::write(rec.dir.join(format!("rlua-{n:03}-orig-plain.rlua")), l);
            // Lines line up in both versions: shrink them together.
            let rl: Vec<&str> = r.split('\n').collect();
            let ll: Vec<&str> = l.split('\n').collect();
            if rl.len() == ll.len() {
                let idx: Vec<usize> = (0..rl.len()).collect();
                let mut budget = 500usize;
                let keep_idx = mutate::ddmin(idx, &mut budget, &mut |ix: &[usize]| {
                    let rr: Vec<&str> = ix.iter().map(|&i| rl[i]).collect();
                    let lll: Vec<&str> = ix.iter().map(|&i| ll[i]).collect();
                    let (s, _, cand_l, _) = diff(&rr.join("\n"), &lll.join("\n"));
                    // (A candidate whose twin stops compiling, e.g. without its `::contN::`
                    // line, is not the same program in two dialects any more.)
                    (cand_l.class == Class::Syntax) == (ours_l.class == Class::Syntax)
                        && same_kind(&sig, s.as_ref())
                });
                let rr: Vec<&str> = keep_idx.iter().map(|&i| rl[i]).collect();
                let lll: Vec<&str> = keep_idx.iter().map(|&i| ll[i]).collect();
                let (rr, lll) = (rr.join("\n"), lll.join("\n"));
                let (_, o, ol, t) = diff(&rr, &lll);
                rec.save(
                    &sig,
                    "rlua",
                    &[("", &rr), ("-plain", &lll)],
                    &format!("remedy .rlua: {}\nremedy .lua:  {}\nc lua .lua:   {}", o.text, ol.text, t.text),
                );
            } else {
                rec.save(&sig, "rlua", &[("", r), ("-plain", l)], "line counts differ (generator bug?)");
            }
        }
    }
    if rng.pct(30) && ours.class != Class::Panic {
        remedy_only_checks(r, "=t.rlua", &ours, rng, rec);
    }
}

#[test]
#[ignore = "sweep1: fuzz driver (long); run with --ignored, SWEEP1_ITERS/SWEEP1_SEED/SWEEP1_SECS"]
fn fuzz_rlua() {
    let (iters, secs, started) = budget();
    let seed = env_u64("SWEEP1_SEED", 1);
    let mut rec = Recorder::new("rlua");
    let mut cov = grammar::Coverage::default();
    for i in 0..iters {
        if started.elapsed().as_secs() > secs {
            break;
        }
        let mut rng = Rng::new(seed.wrapping_mul(999_983).wrapping_add(i) ^ 0x5151);
        let mut g = grammar::Gen::new(&mut rng, true);
        let p = g.program(10 + (i % 30) as i32);
        for (k, v) in &g.cov.forms {
            *cov.forms.entry(k).or_default() += v;
        }
        let mut r2 = Rng::new(seed ^ i ^ 77);
        handle_rlua(&p.r, &p.l, &mut rec, &mut r2);
    }
    rec.report(started);
    let covtxt: String = cov.forms.iter().map(|(k, v)| format!("{v:8} {k}\n")).collect();
    let _ = std::fs::write(rec.dir.join("coverage-rlua.txt"), &covtxt);
}

/// Quick self-check (not ignored): generated plain programs compile in C Lua (the
/// generator emits valid Lua), RemedyLua never panics on them, and the rlua/plain twins
/// have the same line structure.
#[test]
fn generator_self_check() {
    for i in 0..40u64 {
        let mut rng = Rng::new(424_242 + i);
        let mut g = grammar::Gen::new(&mut rng, i % 2 == 1);
        let p = g.program(12);
        assert_eq!(
            p.r.matches('\n').count(),
            p.l.matches('\n').count(),
            "line structure differs:\n{}\n----\n{}",
            p.r,
            p.l
        );
        let ours = harness::remedy(p.r.as_bytes(), if i % 2 == 1 { "=t.rlua" } else { "=t" });
        assert!(
            !matches!(ours.class, Class::Panic | Class::Timeout | Class::Host),
            "{:?} on:\n{}",
            ours,
            p.r
        );
        let theirs = harness::oracle(p.l.as_bytes());
        assert_ne!(theirs.class, Class::Syntax, "generator emitted invalid Lua:\n{}\n{}", p.l, theirs.text);
    }
}

/// Long-run finding (`fuzz_rlua`, `SWEEP1_SEED=10000`): minimizing
/// a finding whose candidates ran to their fuel (seconds each) held the driver over an
/// hour, past `SWEEP1_SECS`. `ddmin` now stops at a deadline with what it has.
#[test]
fn ddmin_stops_at_its_deadline() {
    let units: Vec<u32> = (0..64).collect();
    let mut budget = 500usize;
    let mut calls = 0;
    let past = std::time::Instant::now();
    let out = mutate::ddmin_until(units.clone(), &mut budget, past, &mut |_: &[u32]| {
        calls += 1;
        true
    });
    assert_eq!((calls, out.len()), (0, 64), "a passed deadline must stop before the first candidate");
    // Before the deadline it reduces as before.
    let mut budget = 500usize;
    let later = std::time::Instant::now() + std::time::Duration::from_secs(60);
    let out = mutate::ddmin_until(units, &mut budget, later, &mut |c: &[u32]| c.contains(&7));
    assert_eq!(out, vec![7]);
}

/// Long-run finding rlua-001 (`fuzz_rlua`, `SWEEP1_SEED=10000`): a
/// loop body whose last statement is a `return` spanning several lines (a `\z` string)
/// got its plain-side `continue` label after the `return`, which C Lua rejects (`'end'
/// expected`). The generator now asks the block whether it returned instead of reading
/// the last line. The same program (769) then showed a second generator bug: an index
/// key that starts with `[` (`t[[[x]]]`, a long string). Replays the fuzz_rlua programs up
/// to and past it; 30,000 of them were checked clean once.
const RLUA_001_LAST: u64 = 1_000;

#[test]
fn rlua_continue_label_after_multiline_return() {
    let seed = 10_000u64;
    for i in 0..RLUA_001_LAST {
        let mut rng = Rng::new(seed.wrapping_mul(999_983).wrapping_add(i) ^ 0x5151);
        let mut g = grammar::Gen::new(&mut rng, true);
        let p = g.program(10 + (i % 30) as i32);
        let theirs = harness::oracle(p.l.as_bytes());
        assert_ne!(
            theirs.class,
            Class::Syntax,
            "fuzz_rlua seed {seed} program {i}: plain twin is invalid Lua:\n{}\n{}",
            p.l,
            theirs.text
        );
    }
}

/// `.rlua` text and its plain side: `@` names `label` in both, and each `continue` becomes
/// `goto <label>` on the plain side (the generator's translation).
fn rl(text: &str, label: &str) -> grammar::Out {
    let r = text.replace('@', label);
    let mut l = String::new();
    let mut word = String::new();
    for c in r.chars().chain(std::iter::once('\n')) {
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        l.push_str(if word == "continue" { "goto " } else { &word });
        if word == "continue" {
            l.push_str(label);
        }
        word.clear();
        l.push(c);
    }
    l.pop();
    grammar::Out { r, l }
}

fn cat2(xs: &[&grammar::Out]) -> grammar::Out {
    grammar::Out {
        r: xs.iter().map(|x| x.r.as_str()).collect(),
        l: xs.iter().map(|x| x.l.as_str()).collect(),
    }
}

/// A loop of `kind` around `body` (already translated), its label placed by the
/// generator's rule (`grammar::continue_body`). Each kind's body sees a local `i` (1..4).
/// `generic` picks the generic `for` for `For`.
fn continue_loop(
    kind: grammar::LoopKind,
    generic: bool,
    label: &str,
    body: grammar::Out,
    both: bool,
    ends_return: bool,
) -> grammar::Out {
    use grammar::LoopKind::*;
    let b = grammar::continue_body(kind, body, label, both, ends_return);
    let n = format!("n_{label}");
    let counted = format!("local {n} = 0\n");
    let step = format!("{n} = {n} + 1\nlocal i = {n}\n");
    let (head, tail) = match kind {
        While => (format!("{counted}while {n} < 4 do\n{step}"), "end\n".to_owned()),
        For if generic => ("for _, i in ipairs({1, 2, 3, 4}) do\n".to_owned(), "end\n".to_owned()),
        For => ("for i = 1, 4 do\n".to_owned(), "end\n".to_owned()),
        Repeat => (format!("{counted}repeat\n{step}"), format!("until {n} >= 4\n")),
        // (The condition reads the body's local `i`.)
        RepeatScoped => (format!("{counted}repeat\n{step}"), "until i >= 4\n".to_owned()),
    };
    grammar::Out { r: format!("{head}{}{tail}", b.r), l: format!("{head}{}{tail}", b.l) }
}

/// The plain twin of every `continue` shape the generator gives (each loop kind; the
/// `continue` in nested `if`/`do` blocks; locals, `<const>` and `<close>` before and after
/// it; a body ending in `return`; the `goto continue` idiom; loops nested in loops, in a
/// nested block, and in a function in the loop) compiles in C Lua and runs as the `.rlua`
/// program does in RemedyLua (and as it does in C Lua, but for C Lua's
/// goto-out-of-a-`<close>`-block bug, LR-19). Parity finding: `fuzz_rlua` seed 10000 gave a
/// twin C Lua rejected with "no visible label 'cont13'" (a twin LR-1 had broken, reduced by
/// the minimizer without its label line; see `handle_rlua`).
#[test]
fn rlua_continue_twins_compile_and_agree() {
    let _heavy = super::common::memory_heavy();
    use grammar::LoopKind::{self, *};
    let close = |id: &str| {
        format!(
            "local {id} <close> = setmetatable({{}}, {{__close = function(o, e) __emit('close', '{id}') end}})\n"
        )
    };
    // (body, ends in `return`)
    let bodies: Vec<(String, bool)> = vec![
        ("if i % 2 == 0 then continue end\n__emit('a', i)\n".into(), false),
        ("__emit('first', i)\nif i == 3 then continue end\n".into(), false),
        ("do\nif i % 2 == 1 then\ndo\ncontinue\nend\nend\nend\n__emit('b', i)\n".into(), false),
        ("if i == 1 then\ncontinue\nelseif i == 2 then\n__emit('two')\ncontinue\nelse\n__emit('else', i)\nend\n__emit('c', i)\n".into(), false),
        ("local a = i * 10\nif a > 20 then continue end\n__emit('d', a)\n".into(), false),
        ("if i == 2 then continue end\nlocal b = i\nlocal c, d = b, b + 1\n__emit('e', b, c, d)\n".into(), false),
        ("if i ~= 3 then continue end\nlocal k <const> = i\nlocal f <const> = function() return k end\n__emit('f', f())\n".into(), false),
        (format!("if i == 4 then continue end\n{}__emit('g', i)\n", close("c1")), false),
        (format!("{}if i % 2 == 0 then continue end\n__emit('h', i)\n", close("c2")), false),
        (format!("do\n{}if i < 3 then continue end\nend\n__emit('j', i)\n", close("c3")), false),
        (format!("do\n{}if i < 3 then continue end\nend\nlocal z = {{}}\n__emit('k', i, #z)\n", close("c4")), false),
        ("if i == 1 then continue end\n__emit('l', i)\nif i == 3 then return 'ret' end\n".into(), false),
        ("if i == 1 then continue end\n__emit('m', i)\nif i > 2 then return i end\n".into(), false),
        ("if i == 1 then continue end\n__emit('m2', i)\nreturn i\n".into(), true),
        ("if i == 1 then continue end\nif i == 3 then goto @ end\n__emit('n', i)\nreturn 'ret', i\n".into(), true),
        ("if i % 2 == 0 then goto @ end\nlocal q = i\n__emit('o', q)\n".into(), false),
        ("local f = function() for j = 1, 2 do if j == 1 then goto skip end __emit('fn', j) ::skip:: end end\nf()\nif i < 4 then continue end\n__emit('p', i)\n".into(), false),
    ];
    let kinds: [(LoopKind, bool); 4] = [(While, false), (For, false), (For, true), (Repeat, false)];
    let mut checked = 0;
    let mut check = |p: &grammar::Out| {
        let wrapped = |s: &str| format!("local function run()\n{s}return 'done'\nend\n__emit(run())\n");
        let (r, l) = (wrapped(&p.r), wrapped(&p.l));
        assert_eq!(
            r.matches('\n').count(),
            l.matches('\n').count(),
            "line structure differs:\n{r}\n----\n{l}"
        );
        let theirs = harness::oracle(l.as_bytes());
        assert_ne!(theirs.class, Class::Syntax, "C Lua rejects the twin:\n{l}\n{}\n.rlua:\n{r}", theirs.text);
        let ours = harness::remedy(r.as_bytes(), "=t.rlua");
        let twin = harness::remedy(l.as_bytes(), "=t");
        assert_eq!(ours.class, Class::Ok, "{r}\n{}", ours.text);
        assert_eq!(
            (ours.class, &ours.text),
            (twin.class, &twin.text),
            ".rlua and its twin differ:\n{r}\n----\n{l}"
        );
        if !harness::close_goto_shape(l.as_bytes()) {
            assert_eq!((ours.class, &ours.text), (theirs.class, &theirs.text), "C Lua differs:\n{l}");
        }
        checked += 1;
    };
    for (n, &(kind, generic)) in kinds.iter().enumerate() {
        for (m, (body, ends_return)) in bodies.iter().enumerate() {
            let label = format!("cont{n}_{m}");
            let both = body.contains("goto @");
            let one = continue_loop(kind, generic, &label, rl(body, &label), both, *ends_return);
            check(&one);
            // Nested in each kind of loop with a `continue` of its own, in a nested block of
            // its body, after the outer loop's `continue` and before its locals.
            for (k, &(outer, og)) in kinds.iter().enumerate() {
                let ol = format!("outer{n}_{m}_{k}");
                let body = cat2(&[
                    &rl("if i == 2 then continue end\ndo\n", &ol),
                    &one,
                    &rl(
                        "end\nlocal after = i\nif after == 3 then continue end\n__emit('outer', after)\n",
                        &ol,
                    ),
                ]);
                check(&continue_loop(outer, og, &ol, body, false, false));
            }
        }
    }
    // `repeat` whose `until` reads a body local, with every body local before the `continue`.
    let scoped = [
        "if i % 2 == 0 then continue end\n__emit('s', i)\n".to_owned(),
        "do\nlocal w = i\nif w == 1 then do continue end end\nend\n__emit('t', i)\n".to_owned(),
        format!("do\n{}if i < 3 then continue end\nend\n__emit('u', i)\n", close("c5")),
    ];
    for (m, body) in scoped.iter().enumerate() {
        let label = format!("scoped{m}");
        check(&continue_loop(RepeatScoped, false, &label, rl(body, &label), false, false));
    }
    // ... and one that declares a body local after it: rejected alike.
    let bad = continue_loop(
        RepeatScoped,
        false,
        "scopedbad",
        rl("if i == 1 then continue end\nlocal late = i\n", "scopedbad"),
        false,
        false,
    );
    let (ours, theirs) = (harness::remedy(bad.r.as_bytes(), "=t.rlua"), harness::oracle(bad.l.as_bytes()));
    assert_eq!((ours.class, theirs.class), (Class::Syntax, Class::Syntax), "{}\n{}", ours.text, theirs.text);
    assert!(
        ours.text.contains("jumps into the scope of local 'late'")
            && theirs.text.contains("jumps into the scope of local 'late'"),
        "{}\n{}",
        ours.text,
        theirs.text
    );
    assert!(checked > 300, "{checked}");

    // And the generator's own twins: each `.rlua` program with a `continue` among the
    // first 300 of fuzz_rlua seeds 10000..10010 that RemedyLua compiles, C Lua compiles.
    for seed in 10_000u64..10_011 {
        for i in 0..300u64 {
            let mut rng = Rng::new(seed.wrapping_mul(999_983).wrapping_add(i) ^ 0x5151);
            let mut g = grammar::Gen::new(&mut rng, true);
            let p = g.program(10 + (i % 30) as i32);
            if !p.l.contains("::cont") {
                continue;
            }
            // (Compiled only: in a function never called.)
            let never = |s: &str| format!("local function never(...)\n{s}\nend\n");
            if harness::remedy(never(&p.r).as_bytes(), "=t.rlua").class == Class::Syntax {
                continue;
            }
            let theirs = harness::oracle(never(&p.l).as_bytes());
            assert_ne!(
                theirs.class,
                Class::Syntax,
                "fuzz_rlua seed {seed} program {i}: twin rejected: {}\n{}",
                theirs.text,
                p.l
            );
        }
    }
}

/// Runs a case file: `.lua` in both engines; `.rlua` in RemedyLua against its `-plain`
/// twin (`x.rlua` + `x-plain.lua`) in C Lua. Returns (remedy, c lua).
fn run_case(path: &std::path::Path) -> (Outcome, Outcome) {
    let src = std::fs::read_to_string(path).unwrap().replace("\r\n", "\n");
    if path.extension().is_some_and(|e| e == "rlua") {
        let stem = path.file_stem().unwrap().to_string_lossy();
        let plain = std::fs::read_to_string(path.with_file_name(format!("{stem}-plain.lua")))
            .unwrap()
            .replace("\r\n", "\n");
        (harness::remedy(src.as_bytes(), "=t.rlua"), harness::oracle(plain.as_bytes()))
    } else {
        (harness::remedy(src.as_bytes(), "=t"), harness::oracle(src.as_bytes()))
    }
}

/// `SWEEP1_PROBE=a.lua;b.rlua`: prints both outcomes for each file.
#[test]
#[ignore = "sweep1: manual probe"]
fn probe() {
    for p in std::env::var("SWEEP1_PROBE").unwrap_or_default().split(';').filter(|s| !s.is_empty()) {
        let (o, t) = run_case(std::path::Path::new(p));
        println!("== {p}\nremedy ({:?}):\n{}\nc lua ({:?}):\n{}\n", o.class, o.text, t.class, t.text);
    }
}

/// `SWEEP1_PROBE_DIRECT=a.lua;...`: compiles each file with the host `Lua::load` (no VM
/// frames below the parser), default limits, on a 2 MiB thread.
#[test]
#[ignore = "sweep1: manual probe"]
fn probe_direct() {
    for p in std::env::var("SWEEP1_PROBE_DIRECT").unwrap_or_default().split(';').filter(|s| !s.is_empty()) {
        let src = std::fs::read_to_string(p).unwrap();
        let r = harness::with_timeout(2 << 20, move || {
            let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
            let direct = lua.load(&src, "=t").map(|_| ()).map_err(|e| e.to_string());
            let via = lua
                .exec(format!("local f, e = load({src:?}) return e"), "=w")
                .map(|v| v.first().and_then(|x| lua.to_string_lossy(*x)))
                .map_err(|e| e.to_string());
            (direct, via)
        });
        println!("{p}: {r:?}");
    }
}

// ---------------------------------------------------------------- confirmed findings
//
// Each case file in tests/lua/sweep1/fuzz/ starts with a comment: what is wrong,
// expected vs actual, severity, suspected cause. The test runs it in RemedyLua and C Lua
// (same wrapper as the fuzz) and requires the same output.

fn assert_case(file: &str) {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lua/sweep1/fuzz").join(file);
    let (ours, theirs) = run_case(&p);
    assert_eq!(ours.text, theirs.text, "{file}: RemedyLua vs C Lua");
}

macro_rules! case {
    ($name:ident, $file:literal, $id:literal) => {
        #[test]
        fn $name() {
            let _ = $id;
            assert_case($file);
        }
    };
}

case!(fuzz_01_assign_concat_self, "fuzz-01-assign-concat-self.lua", "sweep1: fuzz-01");
case!(fuzz_02_assign_table_self, "fuzz-02-assign-table-self.lua", "sweep1: fuzz-02");
case!(fuzz_03_assign_self_in_function, "fuzz-03-assign-self-in-function.lua", "sweep1: fuzz-03");
case!(fuzz_04_operator_chain, "fuzz-04-operator-chain.lua", "sweep1: fuzz-04");
case!(fuzz_05_format_f, "fuzz-05-format-F.lua", "sweep1: fuzz-05");
case!(fuzz_06_gsub_capture_message, "fuzz-06-gsub-capture-message.lua", "sweep1: fuzz-06");
case!(fuzz_07_for_messages, "fuzz-07-for-messages.lua", "sweep1: fuzz-07");
case!(fuzz_08_eof_token, "fuzz-08-eof-token.lua", "sweep1: fuzz-08");
case!(fuzz_09_compile_error_chunkname, "fuzz-09-compile-error-chunkname.lua", "sweep1: fuzz-09");
case!(fuzz_10_c_error_position, "fuzz-10-c-error-position.lua", "sweep1: fuzz-10");
case!(fuzz_13_interp_braces, "fuzz-13-interp-braces.rlua", "sweep1: fuzz-13");
case!(fuzz_14_coroutine_error_close, "fuzz-14-coroutine-error-close.lua", "sweep1: fuzz-14");
case!(fuzz_15_load_reader_error, "fuzz-15-load-reader-error.lua", "sweep1: fuzz-15");
case!(fuzz_16_select_mininteger, "fuzz-16-select-mininteger.lua", "sweep1: fuzz-16");
case!(fuzz_17_load_shebang, "fuzz-17-load-shebang.lua", "sweep1: fuzz-17");
case!(fuzz_18_syntax_messages, "fuzz-18-syntax-messages.lua", "sweep1: fuzz-18");
case!(fuzz_19_runtime_messages, "fuzz-19-runtime-messages.lua", "sweep1: fuzz-19");

/// MINOR (fuzz-11): with default `Limits` the parser's real limit is half of `stack_bytes`
/// (512 KiB) of *native stack*, so what compiles depends on the build profile: an
/// unoptimized build rejected 15 nested `do` blocks and ~25 parentheses; at the current
/// test profile (opt-level 2) 100 nested parens/ifs and 150 blocks/tables are rejected.
/// The message ("chunk has too many syntax levels (limit is 200)") names a limit that
/// was not reached. C Lua accepts ~196 levels of each. Cause: parser.rs:126.
#[test]
fn fuzz_11_nesting_budget() {
    let mut bad = vec![];
    for n in [15usize, 30, 60, 100, 150] {
        for (what, src) in [
            ("do-blocks", format!("{}{}", "do ".repeat(n), "end ".repeat(n))),
            ("parens", format!("return {}1{}", "(".repeat(n), ")".repeat(n))),
            ("tables", format!("return {}1{}", "{".repeat(n), "}".repeat(n))),
            ("ifs", format!("{}local z = 1{}", "if true then ".repeat(n), " end".repeat(n))),
        ] {
            let r = harness::with_timeout(8 << 20, move || {
                let mut lua = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL);
                lua.load(&src, "=t").map(|_| ()).map_err(|e| e.to_string())
            });
            if let Some(Err(e)) = r {
                bad.push(format!("{n} nested {what}: {e}"));
            }
        }
    }
    assert!(bad.is_empty(), "rejected (C Lua accepts all of these):\n{}", bad.join("\n"));
}

/// MAJOR (fuzz-12): `format` re-indents the continuation line of a short string that
/// spans lines with a backslash-newline (or `\z`), which changes the string's value: the
/// formatted program behaves differently. Expected: those lines are left alone (as for
/// long strings). Cause: fmt.rs:37 ends a short string at every newline.
#[test]
fn fuzz_12_format_short_string_continuation() {
    let src = "if true then\nlocal s = \"a\\\nb\"\nreturn s, #s\nend\n";
    let before = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL)
        .exec(src, "=t")
        .map(|v| format!("{v:?}"));
    let f = remedy_lua::format(src, "t.lua").unwrap();
    let after = remedy_lua::Lua::new(remedy_lua::Limits::default(), remedy_lua::Libs::ALL)
        .exec(&f, "=t")
        .map(|v| format!("{v:?}"));
    assert_eq!(before.is_ok(), after.is_ok());
    let f2 = remedy_lua::format("do\nlocal s = \"a\\z\n      b\"\nend\n", "t.lua").unwrap();
    assert!(f.contains("\"a\\\nb\""), "formatting changed a backslash-newline continuation:\n{f}");
    assert!(f2.contains("\"a\\z\n      b\""), "formatting changed a \\z continuation:\n{f2}");
}

/// The harness takes out exactly the known non-bugs (`harness::classify` and
/// the wrapper), and planted real differences in the same programs still show.
#[test]
fn harness_known_non_bugs_and_planted_differences() {
    let run = |src: &str| {
        let ours = harness::remedy(src.as_bytes(), "=t");
        let theirs = harness::oracle(src.as_bytes());
        (ours, theirs)
    };
    let verdict = |src: &str, ours: &Outcome, theirs: &Outcome| {
        compare(ours, theirs)
            .map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), ours, theirs, s))
    };
    // An object named by `__name` prints its address, which is normalized; short hex is not.
    assert_eq!(harness::normalize("x locked: 0x5510000000c0 y"), "x locked: ADDR y");
    assert_eq!(harness::normalize("v: 0x1f"), "v: 0x1f");
    // ... and so is one `string.upper` made uppercase (fuzz_rlua SWEEP1_SEED=10007).
    assert_eq!(harness::normalize("TABLE: 0X7F259801E660"), harness::normalize("TABLE: 0X5510000000D0"));
    assert_eq!(harness::normalize("V: 0X1F"), "V: 0X1F");
    // The oracle's step limit inside a `load` reader is the oracle's, not a result.
    let src = "local n = 0 local f = load(function() n = n + 1 if n < 2000000 then return 'x' end end) __emit(f == nil)";
    assert_eq!(harness::oracle(src.as_bytes()).class, Class::Fuel);
    // Made the same by the wrapper: no difference at all.
    for src in [
        "local r = {} for i = 1, 5 do r[i] = math.random(1, 1000) end __emit(table.concat(r, ' '), math.random())",
        "math.randomseed() __emit(math.random(0))",
        "local ks = {} for k in __spairs(_G) do ks[#ks + 1] = k end __emit(table.concat(ks, ' '))",
        "local ks = {} for k in __spairs(os) do ks[#ks + 1] = k end __emit(table.concat(ks, ' '), io, debug, require)",
        "__emit(pcall(function() return io.write('x') end))",
        "local t = {} for i = 1, 1000 do t[i] = {} end __emit(collectgarbage('count'), collectgarbage('step'), collectgarbage())",
    ] {
        let (o, t) = run(src);
        assert_eq!(compare(&o, &t), None, "{src}\nremedy: {}\nc lua:  {}", o.text, t.text);
    }
    // Classified, each with its plant: the same program with a real difference added to
    // RemedyLua's output stays a finding.
    let plant = |o: &Outcome, from: &str, to: &str| {
        assert!(o.text.contains(from), "plant {from:?} not in {}", o.text);
        Outcome { class: o.class, text: o.text.replacen(from, to, 1) }
    };
    let order = "local t = {} for i = 1, 12 do t['k' .. i] = i end for k, v in pairs(t) do __emit(k, v) end \
                 local s = '' for k in pairs(t) do s = s .. k end __emit(#s)";
    let fmod = "__emit(pcall(math.fmod, {}, 'x')) __emit(pcall(math.fmod, 1, 'x')) __emit(pcall(math.fmod))";
    // (After the point where C Lua hit its limit, C Lua runs another program: nothing
    // there can be compared. A difference before it shows.)
    let limit = "__emit('before') local f, e = load('return ' .. string.rep('1 .. ', 300) .. '1') __emit(f and #f() or e) __emit('after')";
    // `math.fmod` with both arguments bad was a class here (C checks the second first);
    // RemedyLua now checks in C's order, so the outputs are equal and a difference is a
    // finding.
    let (o, t) = run(fmod);
    assert_eq!(verdict(fmod, &o, &t), None, "{fmod}\nremedy: {}\nc lua:  {}", o.text, t.text);
    let planted = plant(
        &o,
        "#2 to 'math.fmod' (number expected, got string)",
        "#1 to 'math.fmod' (number expected, got table)",
    );
    assert!(verdict(fmod, &planted, &t).is_some_and(|s| !s.starts_with("expected:")), "plant hidden: {fmod}");
    let cases: [(&str, &str, &str, &str); 3] = [
        (order, "order", "\"k5\" 5", "\"k5\" 6"),
        (limit, "c-limit", "before", "beforehand"),
        ("__emit(string.format('%p', {})) __emit('stable')", "c-nondeterministic", "stable", "unstable"),
    ];
    for (src, kind, from, to) in cases {
        let (o, t) = run(src);
        let v = verdict(src, &o, &t);
        assert_eq!(
            v.as_deref(),
            Some(format!("expected:{kind}").as_str()),
            "{src}\nremedy: {}\nc lua:  {}",
            o.text,
            t.text
        );
        let planted = plant(&o, from, to);
        let v = verdict(src, &planted, &t);
        assert!(v.as_ref().is_some_and(|s| !s.starts_with("expected:")), "plant hidden as {v:?}: {src}");
    }
    // `next`'s first key: C Lua's own runs disagree on it (its seed is random).
    // (Whether it is classified depends on what C Lua's runs print; the plants never are.)
    let src = "local t = {a = 1, b = 2} __emit((next(t))) __emit('stable')";
    let (o, t) = run(src);
    let v = verdict(src, &o, &t);
    assert!(
        v.is_none()
            || v.as_deref() == Some("expected:c-nondeterministic")
            || v.as_deref().is_some_and(|s| s.starts_with("major")),
        "{v:?}"
    );
    for (from, to) in [("stable", "unstable"), ("\"a\"", "\"c\""), ("\"b\"", "\"c\"")] {
        if o.text.contains(from) {
            let planted = plant(&o, from, to);
            assert!(
                verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")),
                "plant {to} hidden"
            );
        }
    }
    // C Lua stopped by its limit as the program's verdict: classified; a difference before
    // it is not.
    // (The fuzz's case: nothing printed before; RemedyLua's own limit error after it.)
    let src = "return select('#', table.unpack({}, 1, 2000000)), pcall(table.unpack, {}, math.mininteger)";
    let (o, t) = run(src);
    assert_eq!(verdict(src, &o, &t).as_deref(), Some("expected:c-limit"), "{}\n{}", o.text, t.text);
    let src = "__emit('before') return select('#', table.unpack({}, 1, 2000000)), pcall(table.unpack, {}, math.mininteger)";
    let (o, t) = run(src);
    assert!(matches!(verdict(src, &o, &t).as_deref(), None | Some("expected:c-limit")));
    let planted = plant(&o, "before", "beforehand");
    assert!(verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")));
    // RemedyLua naming #1 for a number is real.
    let src = "__emit(pcall(math.fmod, 1, 'x'))";
    let (o, t) = run(src);
    let planted = plant(
        &o,
        "#2 to 'math.fmod' (number expected, got string)",
        "#1 to 'math.fmod' (number expected, got number)",
    );
    assert!(verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")));
    // Both hit a limit at the same point first (a refused resume, as C Lua refuses it:
    // LR-3), then C Lua only (a `load` of nesting past C Lua's syntax levels): classified.
    // A difference between the two is not.
    let src = "local function f() local ok, e = pcall(coroutine.wrap(f)) if not ok then error(e, 0) end end \
               __emit(pcall(f)) __emit('between') \
               local g, e = load('return ' .. string.rep('(', 300) .. '1' .. string.rep(')', 300)) __emit(g and g() or e)";
    let (o, t) = run(src);
    assert!(
        o.text.matches(harness::C_LIMIT_MARK).count() >= 1
            && t.text.matches(harness::C_LIMIT_MARK).count() >= 2,
        "{}\n{}",
        o.text,
        t.text
    );
    assert_eq!(verdict(src, &o, &t).as_deref(), Some("expected:c-limit"), "{}\n{}", o.text, t.text);
    let planted = plant(&o, "between", "betwixt");
    assert!(verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")));
    // C Lua's 220 C levels: a suspended coroutine resumed at its 199th runs at the 200th,
    // where protected calls nest on until C Lua gives "error in error handling" with no
    // handler run (ours runs them: item 3). Classified; a difference before it is not.
    // (Depth 197 puts the coroutine's resume at C Lua's 199th level under this wrapper;
    // shallower ones stop at its 200th, "C stack overflow", in a protected call first.)
    let src = "__emit('first') \
               for depth = 197, 197 do \
                 local co = coroutine.create(function() coroutine.yield() \
                   local function nest(k) if k == 0 then return xpcall(function() return 'in' end, function(e) return e end) end return pcall(nest, k - 1) end \
                   __emit(depth, nest(25)) end) \
                 coroutine.resume(co) \
                 local function deep(d) if d == 0 then return coroutine.resume(co) end return coroutine.resume(coroutine.create(deep), d - 1) end \
                 deep(depth) \
               end";
    let (o, t) = run(src);
    assert!(t.text.contains("error in error handling"), "{}", t.text);
    assert_eq!(verdict(src, &o, &t).as_deref(), Some("expected:c-limit"), "{}\n{}", o.text, t.text);
    let planted = plant(&o, "first", "fist");
    assert!(verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")));
    // The same where C Lua's 200th level falls on the call `xpcall` protects: its handler
    // gets "C stack overflow", keeps failing and reaches 220 (ours runs the call; the
    // fuzz's case). Nested `pcall`s put one of these depths there.
    let src = "__emit('first') \
               local function nest(k) if k == 0 then return xpcall(function() return 'in' end, function(e) error('in handler') end) end return pcall(nest, k - 1) end \
               for k = 190, 199 do local r = {nest(k)} __emit(k, r[#r - 1], r[#r]) end";
    let (o, t) = run(src);
    assert_eq!(verdict(src, &o, &t).as_deref(), Some("expected:c-limit"), "{}\n{}", o.text, t.text);
    let planted = plant(&o, "first", "fist");
    assert!(verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")));
    // A handler that keeps failing reaches it in both, marked in both: still compared.
    let src = "__emit(xpcall(error, function(e) error(e, 0) end, 'x')) __emit('after')";
    let (o, t) = run(src);
    assert_eq!(o.text, t.text);
    assert!(t.text.contains("error in error handling"), "{}", t.text);
    let planted = plant(&o, "after", "afterwards");
    assert!(verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")));
    // A limit error RemedyLua gives where C Lua does not is a finding (the mark on our side).
    let src = "local t = {} for i = 1, 10 do t[i] = i end __emit(select('#', table.unpack(t)))";
    let (o, t) = run(src);
    let planted = plant(&o, "10", "\u{2}c-limit\n\"too many results to unpack\"");
    assert!(verdict(src, &planted, &t).is_some_and(|s| !s.starts_with("expected:")));
}

/// LR-10, LR-14, LR-15: C Lua's own runs disagreeing explain a
/// difference only where they disagree, and only with what they printed; a difference
/// where every run agrees stays a finding.
#[test]
fn c_nondeterminism_explains_only_what_varies() {
    let runs =
        |rs: &[&str]| rs.iter().map(|r| r.lines().map(str::to_owned).collect::<Vec<_>>()).collect::<Vec<_>>();
    let ex = |ours: &str, rs: &[&str], free: bool| {
        harness::c_runs_explain(&ours.lines().collect::<Vec<_>>(), &runs(rs), free)
    };
    // LR-10: `next` over two tables on one line; RemedyLua's pair is in no single run.
    let c = ["a\nnil \"c\" 3 \"x\" 1\nz", "a\nnil \"d\" 4 \"y\" 2\nz", "a\nnil \"c\" 3 \"y\" 2\nz"];
    assert!(ex("a\nnil \"d\" 4 \"x\" 1\nz", &c, false));
    assert!(!ex("a\nnil \"e\" 4 \"x\" 1\nz", &c, false), "a value no run printed");
    assert!(!ex("a\nnull \"c\" 3 \"x\" 1\nz", &c, false), "a token every run agrees on");
    assert!(!ex("b\nnil \"c\" 3 \"x\" 1\nz", &c, false), "a stable line before");
    assert!(!ex("a\nnil \"c\" 3 \"x\" 1\nzz", &c, false), "a stable line after");
    assert!(!ex("a\nnil \"c\" 3 \"x\" 1\nz", &c[..1], false), "one run, nothing varies");
    assert!(!ex("a\nnil \"c\" 3 \"x\" 1\nz", &[c[0], c[0]], false), "runs that agree explain nothing");
    // A clock (not a walk): any value where the runs vary, nothing where they do not.
    let clock = ["t 0.01 ok", "t 0.02 ok"];
    assert!(ex("t 0.37 ok", &clock, true));
    assert!(!ex("t 0.37 ko", &clock, true));
    // LR-14: the number of lines follows an address; each line must be one some run printed.
    let g = |n: usize| format!("head\n{}tail", "print nil nil\nfunction\n".repeat(n));
    let (g12, g15, g13) = (g(12), g(15), g(13));
    assert!(ex(&g12, &[&g15, &g13], false));
    let planted = g12.replacen("function", "functio", 1);
    assert!(!ex(&planted, &[&g15, &g13], false), "a line no run printed");
    assert!(!ex(&g12.replace("tail", "tale"), &[&g15, &g13], false));
    // LR-15: the name C Lua's error finds for a function.
    let e = [
        "false bad argument #1 to 'obj' (table expected, got number)",
        "false bad argument #1 to 'setmetatable' (table expected, got number)",
    ];
    assert!(ex(e[1], &e, true));
    assert!(!ex("false bad argument #2 to 'setmetatable' (table expected, got number)", &e, true));
}

/// The same through `classify`, deterministically (no case depends on C Lua's runs
/// varying, LR-15's flake): a function named by another of its names is classified
/// whatever name C Lua's runs pick; names of different functions, and plants on stable
/// lines, stay findings.
#[test]
fn c_nondeterminism_classify_is_deterministic() {
    let fake =
        |o: &Outcome, from: &str, to: &str| Outcome { class: o.class, text: o.text.replacen(from, to, 1) };
    let verdict = |src: &str, ours: &Outcome, theirs: &Outcome| {
        compare(ours, theirs)
            .map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), ours, theirs, s))
    };
    // LR-15: C Lua's runs may all say 'obj' (they did on a gate run) or all 'setmetatable'.
    let src = "obj = setmetatable __emit(pcall(setmetatable, 1)) __emit('stable')";
    let ours = harness::remedy(src.as_bytes(), "=t");
    assert!(ours.text.contains("'setmetatable'"), "{}", ours.text);
    let c_obj = fake(&ours, "'setmetatable'", "'obj'");
    assert_eq!(verdict(src, &ours, &c_obj).as_deref(), Some("expected:name-alias"));
    assert_eq!(verdict(src, &c_obj, &ours).as_deref(), Some("expected:name-alias"));
    // Plants: another argument number, a stable line, a name that holds another function.
    for (from, to) in [("#1", "#2"), ("stable", "unstable")] {
        let planted = fake(&ours, from, to);
        let v = verdict(src, &planted, &c_obj);
        assert!(v.as_ref().is_some_and(|s| !s.starts_with("expected:")), "plant {to} hidden as {v:?}");
    }
    let src2 = "obj = print __emit(pcall(setmetatable, 1)) __emit('stable')";
    // (A wrong name planted in RemedyLua's output, against C Lua's real one: `obj` holds
    // `print`, so 'obj' is no name of `setmetatable`.)
    let theirs2 = harness::oracle(src2.as_bytes());
    let ours2 = fake(&harness::remedy(src2.as_bytes(), "=t"), "'setmetatable'", "'obj'");
    let v = verdict(src2, &ours2, &theirs2);
    assert!(
        v.as_ref().is_some_and(|s| !s.starts_with("expected:")),
        "names of different functions hidden as {v:?}"
    );
    assert_eq!(
        harness::quoted_name_swap("to 'obj' (x)", "to 'string.rep' (x)"),
        Some(("obj".into(), "string.rep".into()))
    );
    assert_eq!(harness::quoted_name_swap("#1 to 'obj' (x)", "#2 to 'obj' (x)"), None);
    assert_eq!(harness::quoted_name_swap("to 'obj' (x)", "to 'setmetatable' (y)"), None);
    // `next`'s first key (LR-10), live: whatever C Lua's runs print, a plant on a stable line
    // stays a finding (only plants are asserted: whether the unplanted output is classified
    // depends on the runs).
    let src = "local t = {c = 3, d = 4} __emit(next(t)) __emit('stable')";
    let ours = harness::remedy(src.as_bytes(), "=t");
    let theirs = harness::oracle(src.as_bytes());
    let planted = fake(&ours, "stable", "unstable");
    assert!(verdict(src, &planted, &theirs).is_some_and(|s| !s.starts_with("expected:")), "plant hidden");
}

/// LR-10 again (`fuzz_mutated` seed 10017): `next`'s first key among four. RemedyLua's runs
/// under other seeds count as runs; a plant on a stable line or a key no run gives stays.
#[test]
fn next_first_key_counts_remedy_seed_runs_but_not_plants() {
    let src = "local t = {a = 1, b = 2, c = 3, d = 4} __emit(#'abcd', next(t)) __emit('stable')";
    let ours = harness::remedy(src.as_bytes(), "=t");
    let theirs = harness::oracle(src.as_bytes());
    let verdict = |o: &Outcome| {
        compare(o, &theirs).map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), o, &theirs, s))
    };
    for (from, to) in [("stable", "unstable"), ("4", "5")] {
        let planted = Outcome { class: ours.class, text: ours.text.replacen(from, to, 1) };
        let v = verdict(&planted);
        assert!(v.as_ref().is_some_and(|s| !s.starts_with("expected:")), "plant {to} hidden as {v:?}");
    }
    let key = ["\"a\"", "\"b\"", "\"c\"", "\"d\""].into_iter().find(|k| ours.text.contains(k)).unwrap();
    let planted = Outcome { class: ours.class, text: ours.text.replacen(key, "\"e\"", 1) };
    assert!(verdict(&planted).is_some_and(|s| !s.starts_with("expected:")), "a key no run gives hidden");
}

/// LR-20: a `.rlua` program and its plain twin may walk a table in another order under the
/// same seed; RemedyLua's runs of both count as runs. A plant on a stable line stays.
#[test]
fn rlua_twin_order_counts_its_runs_but_not_plants() {
    let (r, l) = (
        "local t: {[string]: number} = {a = 1, b = 2, c = 3} __emit(next(t)) __emit('stable')",
        "local t = {a = 1, b = 2, c = 3} __emit(next(t)) __emit('stable')",
    );
    let ours = harness::remedy(r.as_bytes(), "=t.rlua");
    let theirs = harness::oracle(l.as_bytes());
    let planted = Outcome { class: ours.class, text: ours.text.replacen("stable", "unstable", 1) };
    let v = compare(&planted, &theirs)
        .map(|s| harness::classify(r.as_bytes(), "=t.rlua", l.as_bytes(), &planted, &theirs, s));
    assert!(v.as_ref().is_some_and(|s| !s.starts_with("expected:")), "plant hidden as {v:?}");
}

/// A program whose output reads a clock differs between any two runs, fast loop or not:
/// no fast/slow difference (`fuzz_mutated` SWEEP1_SEED=10019), and
/// no difference from C Lua, whose clock is the process's (LR-24, seed 10026).
#[test]
fn clock_output_is_no_fast_slow_difference() {
    let src = "__emit(os.clock() - os.clock({}), os.time({year = 2020, month = 1, day = 1}) - os.clock())";
    let o = harness::remedy(src.as_bytes(), "=t");
    assert_ne!(o.class, Class::Panic, "{}", o.text);
    let src = "return(os),(os.clock()), os.time({year = 2020, month = 1, day = 1}) - os.time({year = 2020, month = 1, day = 12})";
    let (ours, theirs) = (harness::remedy(src.as_bytes(), "=t"), harness::oracle(src.as_bytes()));
    let v = |o: &Outcome| {
        compare(o, &theirs).map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), o, &theirs, s))
    };
    assert!(
        matches!(v(&ours).as_deref(), None | Some("expected:c-nondeterministic")),
        "{}\n{}",
        ours.text,
        theirs.text
    );
    let planted = Outcome { class: ours.class, text: ours.text.replacen("-950400", "-950401", 1) };
    assert!(v(&planted).is_some_and(|s| !s.starts_with("expected:")), "plant hidden");
}

/// LR-19's symptom as a segment's error mid-output (`fuzz_rlua` SWEEP1_SEED=10019): classified
/// in the shape only, and only when what C Lua printed before it is RemedyLua's.
#[test]
fn c_lua_close_goto_bug_segment_error() {
    let src = "for i = 0, 1 do\n  do\n    local c <close> = setmetatable({}, {__close = function() end})\n    \
               if true then goto cont end\n  end\n  local v = {}\n  ::cont::\nend";
    let ours = Outcome { class: Class::Ok, text: "seg 1 ok\nseg 2 ok\n\u{1}=> OK ".into() };
    let theirs = Outcome {
        class: Class::Ok,
        text: "seg 1 ok\nseg 2 error \"t:9: attempt to call a nil value\"\n\u{1}=> OK ".into(),
    };
    let v = |o: &Outcome, t: &Outcome| {
        compare(o, t).map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), o, t, s))
    };
    assert_eq!(v(&ours, &theirs).as_deref(), Some("expected:c-lua-close-goto-bug"));
    let planted = Outcome { class: Class::Ok, text: "seg 1 okay\nseg 2 ok\n\u{1}=> OK ".into() };
    assert!(
        v(&planted, &theirs).is_some_and(|s| !s.starts_with("expected:")),
        "a difference before the error hidden"
    );
    let other = "local v = {} __emit(v)";
    let w = compare(&ours, &theirs)
        .map(|s| harness::classify(other.as_bytes(), "=t", other.as_bytes(), &ours, &theirs, s));
    assert!(w.is_some_and(|s| !s.starts_with("expected:")), "outside the shape hidden");
}

const FUZZ_10013: &str = r#"local g1 = 32963
__seg(1, function(p4, p5, p6)
local function f7(p8, p9)
function Gf10()
end
p4 = string.format("%#+.2e%.12s%%%+-.7f", (tonumber("inf") or 0.5) + 0.0, ~(p9), -0x8000000000000000 / #('abca0x10\t\z  
   xa'))
end
do
;for i12 = 0, 4 do
do
local c15 <close> = setmetatable({v = "c15"}, {__close = function(o, e) print("close", e) end})
if type(nil) then goto cont14 end
end
local v17 <const> = {p6; (utf8.len((string.gsub(string.format("%+.8o", (('\1740xz9k'):byte(-3) or 0)), ".+%f[%w]%[.-", function(c) return c .. "!" end))) or -1)}
::cont14::
end
end
f7, g2 = function(p18, ...)
for i19 = 0, 0, 1 do
do local s = p4
end
end
end, (g1) + 0.0
end, math.ult(g1, #{g1, 0x0, 11, 0xfffffffffffffffff}) and ([=["'x]_]y]=]) or (g1), g3)
"#;

/// LR-19: C Lua 5.4.7 skips the close when a `goto` leaves a block
/// with a `<close>` local, past a local with a run-time value, to a label ending the
/// enclosing block. Classified in that shape with that symptom only; a real difference
/// nearby, and the shapes C Lua gets right, stay findings.
#[test]
fn c_lua_close_goto_bug_is_classified_narrowly() {
    let prog = |decl: &str| {
        format!(
            "for i = 0, 1 do\n  do\n    local c <close> = setmetatable({{}}, {{__close = function() __emit('close') end}})\n    \
             if true then goto cont end\n  end\n  {decl}\n  ::cont::\nend\n__emit('done')\n"
        )
    };
    let verdict = |src: &str, ours: &Outcome, theirs: &Outcome| {
        compare(ours, theirs)
            .map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), ours, theirs, s))
    };
    for decl in ["local v = {}", "local v <const> = {1}", "local v <const> = function() end"] {
        let src = prog(decl);
        assert!(harness::close_goto_shape(src.as_bytes()), "{decl}");
        let (ours, theirs) = (harness::remedy(src.as_bytes(), "=t"), harness::oracle(src.as_bytes()));
        assert_eq!(
            verdict(&src, &ours, &theirs).as_deref(),
            Some("expected:c-lua-close-goto-bug"),
            "{decl}\n{}\n{}",
            ours.text,
            theirs.text
        );
        // A real difference nearby stays a finding.
        let planted = Outcome { class: ours.class, text: ours.text.replacen("\"done\"", "\"dune\"", 1) };
        let v = verdict(&src, &planted, &theirs);
        assert!(v.as_ref().is_some_and(|s| !s.starts_with("expected:")), "plant hidden as {v:?}");
    }
    // The fuzz's program (fuzz_rlua SWEEP1_SEED=10013, plain twin): C Lua leaves the loop
    // after one pass, silently.
    let src = FUZZ_10013;
    let (ours, theirs) = (harness::remedy(src.as_bytes(), "=t"), harness::oracle(src.as_bytes()));
    assert_eq!(
        verdict(src, &ours, &theirs).as_deref(),
        Some("expected:c-lua-close-goto-bug"),
        "{}\n{}",
        ours.text,
        theirs.text
    );
    let planted = Outcome { class: ours.class, text: ours.text.replacen("seg 1 ok", "seg 1 okay", 1) };
    assert!(verdict(src, &planted, &theirs).is_some_and(|s| !s.starts_with("expected:")));
    // Shapes C Lua gets right are not the shape.
    for decl in ["local v <const> = 1", "local v <close> = nil", ""] {
        assert!(!harness::close_goto_shape(prog(decl).as_bytes()), "{decl:?}");
    }
    let no_close = "for i = 0, 1 do\n  do\n    local c = {}\n    if true then goto cont end\n  end\n  local v = {}\n  ::cont::\nend";
    assert!(!harness::close_goto_shape(no_close.as_bytes()));
    let not_last = "for i = 0, 1 do\n  do\n    local c <close> = nil\n    if true then goto cont end\n  end\n  local v = {}\n  ::cont::\n  v = 1\nend";
    assert!(!harness::close_goto_shape(not_last.as_bytes()));
}

/// LR-25 (`fuzz_mutated` SWEEP1_SEED=10027): C Lua 5.4.7 fails on a store to an upvalue
/// table under a comparison key; the difference that rewriting the key away removes in
/// both engines is that bug, and a real difference beside it stays a finding.
#[test]
fn c_lua_compare_key_bug_is_classified() {
    for key in ["#t ~= 1", "1 < 2", "n == 0"] {
        let src = format!("local t, n = {{}}, 0 local function f() t[{key}] = 1 end f() __emit(t[true])");
        let (ours, theirs) = (harness::remedy(src.as_bytes(), "=t"), harness::oracle(src.as_bytes()));
        assert!(theirs.text.contains("attempt to index a"), "{key}: {}", theirs.text);
        let v = |o: &Outcome| {
            compare(o, &theirs)
                .map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), o, &theirs, s))
        };
        assert_eq!(v(&ours).as_deref(), Some("expected:c-lua-compare-key-bug"), "{key}: {}", ours.text);
        let planted = Outcome { class: ours.class, text: ours.text.replacen('1', "2", 1) };
        assert!(v(&planted).is_some_and(|s| !s.starts_with("expected:")), "{key}: plant hidden");
    }
    assert_eq!(harness::compare_keys_wrapped(b"t[a] = 1 x = t[a < b]"), None);
}

/// Parity's fuzz round r8: an address the program took apart past `normalize` is C Lua's
/// address noise (LR-31), and C Lua spending its time between instructions (a vararg tail
/// call copying a growing argument list) stops at the wall-time cut, inconclusive, well
/// inside the driver's timeout (LR-30).
#[test]
fn oracle_address_split_and_wall_cut() {
    let src = r#"__emit((tostring(setmetatable({}, {})):gsub("", ": ADDR"))) __emit(1)"#;
    let (ours, theirs) = (harness::remedy(src.as_bytes(), "=t"), harness::oracle(src.as_bytes()));
    let v = |o: &Outcome| {
        compare(o, &theirs).map(|s| harness::classify(src.as_bytes(), "=t", src.as_bytes(), o, &theirs, s))
    };
    assert_eq!(v(&ours).as_deref(), Some("expected:c-address"), "{}\n{}", ours.text, theirs.text);
    let planted = Outcome { class: ours.class, text: ours.text.replacen("\n1", "\n2", 1) };
    assert!(v(&planted).is_some_and(|s| !s.starts_with("expected:")), "plant hidden");
    let t = std::time::Instant::now();
    let o = harness::oracle(b"local function f(...) return f('#', ...) end return f(1, 2, 3)");
    assert_eq!(o.class, Class::Fuel, "{}", o.text);
    assert!(t.elapsed() < harness::ORACLE_WALL * 2, "{:?}", t.elapsed());
}

/// The child of `harness::oracle` for programs C Lua may crash on: the program on stdin
/// (not a test on its own).
#[test]
#[ignore = "run by harness::oracle"]
fn oracle_child() {
    let mut src = Vec::new();
    std::io::Read::read_to_end(&mut std::io::stdin(), &mut src).unwrap();
    let r = match harness::oracle_raw(&src) {
        None => "T".into(),
        Some(Ok(t)) => format!("O{t}"),
        Some(Err(e)) => format!("E{e}"),
    };
    print!("{}{r}{}", harness::CHILD_OPEN, harness::CHILD_CLOSE);
}

/// C Lua 5.4.7 segfaults on this program (LR-19's shape, cut from `parity_26`): the oracle runs it
/// in a child, so the driver lives on and the crash is that bug; a crash elsewhere stays
/// a finding.
#[test]
fn oracle_crash_does_not_end_the_driver() {
    let src = r####"
local ev = {}
local function closer(name) return setmetatable({}, {__close = function(_, e) ev[#ev + 1] = name .. (e and ":err" or "") end}) end
for i = 0, 1 do
  do local c <close> = closer("c" .. i) if true then goto cont end end
  local v = {}
  ::cont::
end
local n = 0
while n < 2 do
  n = n + 1
  do
    local a <close> = closer("a" .. n)
    local b <close> = closer("b" .. n)
    if n > 0 then goto next end
  end
  local w = setmetatable({}, {})
  ::next::
end
"####;
    let (ours, theirs) = (harness::remedy(src.as_bytes(), "=t"), harness::oracle(src.as_bytes()));
    assert!(theirs.text.starts_with("<host> <crash>"), "{}", theirs.text);
    let sig = compare(&ours, &theirs).expect("a crash is a difference");
    assert_eq!(
        harness::classify(src.as_bytes(), "=t", src.as_bytes(), &ours, &theirs, sig.clone()),
        "expected:c-lua-close-goto-bug"
    );
    assert_eq!(harness::classify(b"return 1", "=t", b"return 1", &ours, &theirs, sig), "oracle-crash");
}

/// The memory cut point accepts a catchable `stack overflow` (frames filling a smaller
/// budget), and nothing else besides the memory limit.
/// C Lua's string-hash seed changes with every state, as it does across processes.
/// Stock C Lua seeds from the time and addresses, which repeat within a process, so the
/// harness's reruns of a program all walked tables in one order and never showed the
/// orders it gives elsewhere: `next(t)`'s first key of `{b = 2, c = 3, d = 4}` came out
/// a "major" difference when RemedyLua's first key was another one (fuzz long4, mut-002).
#[test]
fn oracle_seed_changes_per_state() {
    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..40 {
        seen.insert(remedy_lua_oracle::run("return (next({b = 2, c = 3, d = 4}))", 1_000_000));
    }
    let all: std::collections::BTreeSet<_> = ["b", "c", "d"].iter().map(|k| Ok(k.to_string())).collect();
    assert_eq!(seen, all);
}

/// The oracle's step limit counts the instructions of every coroutine. C Lua's count hook
/// is inherited by each new coroutine with a fresh count, so with the whole budget as the
/// hook's period, a program whose work is spread over many short coroutines was never
/// stopped: a fuzzed chain of `coroutine.wrap` calls ran on past the harness's timeout,
/// whose abandoned threads kept spinning (fuzz_rlua, seed 10000, during minimization).
#[test]
fn oracle_step_limit_counts_every_coroutine() {
    // 20 x 150 coroutines of ~1,500 instructions each: about 4.5 million in all, of
    // which the main thread runs a few hundred.
    let src = "local function f(n) for j = 1, 500 do end if n > 0 then return coroutine.wrap(f)(n - 1) end return 'done' end \
               for i = 1, 20 do f(150) end return 'finished'";
    let r = remedy_lua_oracle::run(src, 1_000_000);
    assert!(r.as_ref().is_err_and(|e| e.contains("step limit")), "{r:?}");
    // With room enough, the same program finishes.
    assert_eq!(remedy_lua_oracle::run(src, 100_000_000), Ok("finished".to_string()));
}

/// The oracle's memory ceiling. C Lua has no memory limit, so a program doubling a string
/// by concatenation (which the wrapper's `string.rep` cap does not see) once took a fuzz
/// driver to 13 GiB and the kernel's OOM killer (fuzz long4, `fuzz_generated`). C Lua now
/// stops at the cap, and a `pcall` that catches its "not enough memory" does not turn that
/// into a verdict the harness would compare with RemedyLua's.
#[test]
fn oracle_memory_is_capped() {
    let cap = 64 << 20;
    let grow = "local s = 'x' while true do s = s .. s end";
    assert_eq!(
        remedy_lua_oracle::run_capped(grow, u64::MAX, cap),
        Err(remedy_lua_oracle::MEMORY_LIMIT.to_string())
    );
    let caught = "local s = 'x' pcall(function() while true do s = s .. s end end) s = nil return 'caught'";
    assert_eq!(
        remedy_lua_oracle::run_capped(caught, u64::MAX, cap),
        Err(remedy_lua_oracle::MEMORY_LIMIT.to_string())
    );
    // Below the cap nothing changes.
    let small = "local s = 'x' for _ = 1, 20 do s = s .. s end return #s";
    assert_eq!(remedy_lua_oracle::run_capped(small, u64::MAX, cap), Ok("1048576".to_string()));
    // (The harness maps that error to `Class::Host`, which `compare` takes as no verdict.)
}

/// A memory cut gives the full run's result or the memory limit (fuzz round r8, gen-001,
/// seed 909001's program at 391,302 bytes): a table store failing while the thread's small
/// stack was a quarter of a tiny budget was made "stack overflow", which C Lua (no budget)
/// would never raise there; caught, it left the wrapper's output with a hole. Only the
/// stack's own growth counts the budget so; anywhere, C Lua's own limits do.
#[test]
fn memory_cut_keeps_ordinary_allocations_the_memory_limit() {
    let src = include_str!("lua/parity/memory_cut_gen.lua");
    let (full, _) = harness::remedy_raw(src.as_bytes(), "=t", harness::remedy_limits());
    for m in [391_302usize, 300_000, 500_000] {
        let (o, _) = harness::remedy_raw(src.as_bytes(), "=t", harness::remedy_limits().memory(m));
        assert!(
            o.class == Class::Memory || o.text == full.text || overflow_from_memory(&full.text, &o.text),
            "memory {m}: {}",
            o.text
        );
    }
}

#[test]
fn memory_cut_accepts_only_a_stack_overflow() {
    let full = "seg 1 ok\nprint 1\n\u{1}=> OK 2";
    assert!(overflow_from_memory(
        full,
        "\u{2}c-limit\nseg 1 error \"t:3: stack overflow\"\nprint 1\n\u{1}=> OK 2"
    ));
    assert!(overflow_from_memory(full, "seg 1 error \"t:3: stack overflow\"\nprint 1\n\u{1}=> OK 2"));
    assert!(overflow_from_memory(
        full,
        "\u{2}c-limit\n\u{2}c-limit\nseg 1 error \"t:3: stack overflow\"\n\u{1}=> OK 2"
    ));
    assert!(!overflow_from_memory(full, "\u{2}c-limit\nseg 1 error \"t:3: attempt to call\"\n\u{1}=> OK 2"));
    assert!(!overflow_from_memory(full, "seg 1 ok\nprint 2\n\u{1}=> OK 2"));
    assert!(!overflow_from_memory(
        full,
        "seg 1 error \"t:3: attempt to index a nil value\"\nprint 1\n\u{1}=> OK 2"
    ));
    // A `pcall` caught the overflow and outer frames printed on before it reached the
    // segment (fuzz_generated SWEEP1_SEED=10003, memory 338711).
    let rec = "print 1\nprint 1\nprint 1\nseg 4 ok\n\u{1}=> OK 2";
    assert!(overflow_from_memory(
        rec,
        "print 1\n\u{2}c-limit\nprint 1\n\u{2}c-limit\nseg 4 error \"t:32: stack overflow\"\n\u{1}=> OK 2"
    ));
    // ... but not with no overflow anywhere, nor when the full run overflows too.
    assert!(!overflow_from_memory(rec, "print 1\n\u{2}c-limit\nprint 1\nseg 4 ok\n\u{1}=> OK 2"));
    let both = "print 1\nprint 1\nprint 1\nseg 4 error \"t:32: stack overflow\"\n\u{1}=> OK 2";
    assert!(!overflow_from_memory(
        both,
        "print 1\n\u{2}c-limit\nprint 1\nseg 4 error \"t:32: stack overflow\"\n\u{1}=> OK 2"
    ));
    let deep = "seg 1 error \"t:3: stack overflow\"\n\u{1}=> OK 2";
    assert!(!overflow_from_memory(deep, "seg 1 error \"t:3: stack overflow\"\n\u{1}=> OK 3"));
}

/// FX2-2 (`fuzz_mutated` seed 900001, fast loop on: a segfault in glibc's `pthread_detach`,
/// not reproducible): `with_timeout` dropped its thread's handle, detaching a thread that
/// could be exiting at that moment. Now a thread that finished is joined before its result
/// is returned, so it has fully exited (its thread-local destructors have run), and one
/// that ran over is kept, joinable. Without the fix the destructor below (it sleeps first)
/// has not run when `with_timeout` returns, and the timed-out thread is not kept.
#[test]
fn fx2_2_with_timeout_never_detaches() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static EXITED: AtomicUsize = AtomicUsize::new(0);
    struct Exit;
    impl Drop for Exit {
        fn drop(&mut self) {
            std::thread::sleep(std::time::Duration::from_millis(100));
            EXITED.fetch_add(1, Ordering::SeqCst);
        }
    }
    std::thread_local!(static ON_EXIT: Exit = const { Exit });
    for i in 0..3 {
        let r = harness::with_timeout(64 << 20, move || {
            ON_EXIT.with(|_| ());
            i
        });
        assert_eq!(r, Some(i));
        assert_eq!(EXITED.load(Ordering::SeqCst), i + 1, "a finished thread was not joined");
    }
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let s2 = stop.clone();
    let r = crate::common::with_deadline(1 << 20, std::time::Duration::from_millis(20), move || {
        while !s2.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    assert_eq!(r, None);
    // (Still running, so no later call can have reaped it.)
    assert!(crate::common::strays() >= 1, "the thread that ran over was not kept");
    stop.store(true, Ordering::SeqCst);
}
