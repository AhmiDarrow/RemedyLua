//! A rough Lua tokenizer (each token keeps the whitespace/comments before it, so joining
//! the tokens gives the source back) and token/line mutations over it.

use super::rng::Rng;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Name,
    Num,
    Str,
    Op,
    Other,
}

#[derive(Clone, Debug)]
pub struct Tok {
    /// Whitespace and comments before the token.
    pub pre: String,
    pub text: String,
    pub kind: Kind,
}

pub fn join(ts: &[Tok]) -> String {
    let mut s = String::new();
    for t in ts {
        s.push_str(&t.pre);
        s.push_str(&t.text);
    }
    s
}

fn long_bracket(b: &[u8], i: usize) -> Option<usize> {
    // `[` `=`* `[`: returns the level.
    if b.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'=') {
        j += 1;
    }
    (b.get(j) == Some(&b'[')).then_some(j - i - 1)
}

fn long_end(b: &[u8], from: usize, lvl: usize) -> usize {
    let close: Vec<u8> = std::iter::once(b']').chain(std::iter::repeat_n(b'=', lvl)).chain(std::iter::once(b']')).collect();
    b[from..].windows(close.len()).position(|w| w == close.as_slice()).map_or(b.len(), |p| from + p + close.len())
}

pub fn tokenize(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut out = vec![];
    let mut i = 0;
    let mut pre_start = 0;
    let ops3 = ["...", "//=", "..="];
    let ops2 = ["..", "==", "~=", "<=", ">=", "<<", ">>", "//", "::", "+=", "-=", "*=", "/=", "%="];
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c == b'-' && b.get(i + 1) == Some(&b'-') {
            if let Some(l) = long_bracket(b, i + 2) {
                i = long_end(b, i + 2, l);
            } else {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            continue;
        }
        let start = i;
        let kind;
        if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            kind = Kind::Name;
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)) {
            while i < b.len() {
                let d = b[i];
                let exp = matches!(d, b'e' | b'E' | b'p' | b'P');
                if d.is_ascii_alphanumeric() || d == b'.' {
                    i += 1;
                    if exp && matches!(b.get(i), Some(b'+' | b'-')) {
                        i += 1;
                    }
                } else {
                    break;
                }
            }
            kind = Kind::Num;
        } else if c == b'"' || c == b'\'' || c == b'`' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            kind = Kind::Str;
        } else if let Some(l) = long_bracket(b, i) {
            i = long_end(b, i, l);
            kind = Kind::Str;
        } else if let Some(op) = ops3.iter().chain(ops2.iter()).find(|o| b[i..].starts_with(o.as_bytes())) {
            i += op.len();
            kind = Kind::Op;
        } else {
            // One character (a whole UTF-8 sequence is covered by the name branch).
            i += 1;
            kind = if b"+-*/%^#&~|<>=(){}[];:,.".contains(&c) { Kind::Op } else { Kind::Other };
        }
        let i2 = i.min(b.len());
        out.push(Tok {
            pre: String::from_utf8_lossy(&b[pre_start..start]).into_owned(),
            text: String::from_utf8_lossy(&b[start..i2]).into_owned(),
            kind,
        });
        pre_start = i2;
        i = i2;
    }
    if pre_start < b.len() {
        out.push(Tok { pre: String::from_utf8_lossy(&b[pre_start..]).into_owned(), text: String::new(), kind: Kind::Other });
    }
    out
}

const KEYWORDS: &[&str] = &[
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto", "if", "in", "local", "nil", "not", "or",
    "repeat", "return", "then", "true", "until", "while",
];
const OPS: &[&str] = &[
    "+", "-", "*", "/", "//", "%", "^", "#", "&", "~", "|", "<<", ">>", "==", "~=", "<=", ">=", "<", ">", "=", "(", ")", "{", "}",
    "[", "]", "::", ";", ":", ",", ".", "..", "...",
];
const NUMS: &[&str] = &[
    "0", "1", "-1", "2", "255", "0x7fffffffffffffff", "0xffffffffffffffff", "9223372036854775807", "9223372036854775808",
    "-9223372036854775808", "1e308", "1e309", "0.0", "-0.0", "0x1p-1074", "1e-320", "0x", "1e", "3..2", "0x.p1", "08", "1.5e+",
    "4294967296", "65536", "200", "1000000", "0/0", "2^63", "math.mininteger", "math.huge",
];
const STRS: &[&str] = &[
    "\"\"", "\"%\"", "\"%d\"", "\"\\0\"", "\"\\xff\\xfe\"", "\"\\u{7FFFFFFF}\"", "\"[\"", "\"%b\"", "\"%f\"", "\"(()\"", "'%1'",
    "\"\\z\"", "\"\\300\"", "\"\\x4\"", "\"\\u{110000000}\"", "\"\\q\"", "[[]]", "[==[]=]]==]", "\"unterminated", "`x{1}`",
    "\"10\"", "\"0x10\"", "\"1e1\"", "\"abc\"", "\"é\"",
];
const NOISE: &[&str] = &[
    "<const>", "<close>", "<toclose>", "goto", "::l::", "...", "local", "function", "end", "return", "break", "--[[", "]]", "\n",
    "\0", "\u{FEFF}", "#!", "\r", "\t", "@", "$", "!", "?", "\\", "`", "continue", "+=", "..=", "self", "_ENV", "_G",
];

fn rand_tok(rng: &mut Rng) -> String {
    match rng.below(5) {
        0 => rng.pick(KEYWORDS).to_string(),
        1 => rng.pick(OPS).to_string(),
        2 => rng.pick(NUMS).to_string(),
        3 => rng.pick(STRS).to_string(),
        _ => rng.pick(NOISE).to_string(),
    }
}

/// Applies 1..=k random mutations. `corpus` supplies lines to splice in.
pub fn mutate(src: &str, rng: &mut Rng, corpus: &[String]) -> String {
    let mut s = src.to_owned();
    let n = if rng.pct(70) { 1 } else { 2 + rng.below(2) };
    for _ in 0..n {
        s = mutate_once(&s, rng, corpus);
    }
    s
}

fn mutate_once(src: &str, rng: &mut Rng, corpus: &[String]) -> String {
    let mut ts = tokenize(src);
    if ts.is_empty() {
        return rand_tok(rng);
    }
    let n = ts.len();
    let i = rng.below(n);
    match rng.below(14) {
        0 => {
            ts.remove(i);
        }
        1 => {
            let t = ts[i].clone();
            ts.insert(i, Tok { pre: " ".into(), ..t });
        }
        2 if n > 1 => {
            let j = (i + 1) % n;
            let (a, b) = (ts[i].text.clone(), ts[j].text.clone());
            ts[i].text = b;
            ts[j].text = a;
        }
        3 => ts[i].text = rand_tok(rng),
        4 => {
            ts.insert(i, Tok { pre: " ".into(), text: rand_tok(rng), kind: Kind::Other });
        }
        5 => {
            // Same-kind replacement keeps programs mostly parseable.
            let k = ts[i].kind;
            let cands: Vec<usize> = (0..n).filter(|&j| ts[j].kind == k).collect();
            let j = *rng.pick(&cands);
            ts[i].text = match k {
                Kind::Num => rng.pick(NUMS).to_string(),
                Kind::Str => rng.pick(STRS).to_string(),
                Kind::Op => rng.pick(OPS).to_string(),
                _ => ts[j].text.clone(),
            };
        }
        6 => {
            // Numbers: a nearby or extreme value.
            let nums: Vec<usize> = (0..n).filter(|&j| ts[j].kind == Kind::Num).collect();
            if !nums.is_empty() {
                let j = *rng.pick(&nums);
                ts[j].text = rng.pick(NUMS).to_string();
            }
        }
        7 => {
            // Truncate.
            ts.truncate(i.max(1));
        }
        8 | 9 => {
            let mut lines: Vec<String> = join(&ts).split('\n').map(str::to_owned).collect();
            let a = rng.below(lines.len());
            let b = rng.below(lines.len());
            match rng.below(4) {
                0 => {
                    lines.remove(a);
                }
                1 => {
                    let l = lines[a].clone();
                    lines.insert(b, l);
                }
                2 => lines.swap(a, b),
                _ => {
                    if let Some(c) = corpus.get(rng.below(corpus.len().max(1))) {
                        let cl: Vec<&str> = c.split('\n').collect();
                        let l = cl[rng.below(cl.len())].to_owned();
                        lines.insert(a, l);
                    }
                }
            }
            return lines.join("\n");
        }
        10 => {
            // Duplicate a run of tokens elsewhere (subtree-ish).
            let len = 1 + rng.below(8);
            let run: Vec<Tok> = ts[i..(i + len).min(n)].to_vec();
            let at = rng.below(n);
            for (k, t) in run.into_iter().enumerate() {
                ts.insert(at + k, t);
            }
        }
        11 => {
            // Delete a run of tokens.
            let len = 1 + rng.below(6);
            ts.drain(i..(i + len).min(n));
        }
        12 => {
            // Wrap a token run in parens / a do-block.
            let len = 1 + rng.below(5);
            let j = (i + len).min(n);
            let (open, close) = *rng.pick(&[("(", ")"), ("do ", " end"), ("{", "}"), ("[", "]"), ("function() ", " end")]);
            ts.insert(j, Tok { pre: String::new(), text: close.into(), kind: Kind::Op });
            ts.insert(i, Tok { pre: " ".into(), text: open.into(), kind: Kind::Op });
        }
        _ => {
            // Whitespace: join tokens (`a.b` → `a .b`, `1 ..2` → `1..2`).
            ts[i].pre = rng.pick(&["", " ", "\n", "\r\n", "\t", "--x\n", "--[==[\n]==]"]).to_string();
        }
    }
    join(&ts)
}

/// Delta debugging (ddmin) over units: the smallest subsequence (by removal) for which
/// `keep` still holds, within a budget of checks.
pub fn ddmin<T: Clone>(units: Vec<T>, budget: &mut usize, keep: &mut dyn FnMut(&[T]) -> bool) -> Vec<T> {
    ddmin_until(units, budget, Instant::now() + MIN_TIME, keep)
}

/// The most one `ddmin` may spend: candidates that run to their fuel take seconds each,
/// so 500 of them held a driver for over an hour past its `SWEEP1_SECS` (long-run
/// finding). What is left unreduced is still a valid reproduction.
pub const MIN_TIME: Duration = Duration::from_secs(60);

/// `ddmin`, stopping (with the smallest set found so far) at `deadline`.
pub fn ddmin_until<T: Clone>(mut units: Vec<T>, budget: &mut usize, deadline: Instant, keep: &mut dyn FnMut(&[T]) -> bool) -> Vec<T> {
    let mut n = 2usize;
    while units.len() >= 2 && *budget > 0 {
        let chunk = units.len().div_ceil(n);
        let mut reduced = false;
        let mut start = 0;
        while start < units.len() && *budget > 0 {
            let end = (start + chunk).min(units.len());
            let mut cand = units[..start].to_vec();
            cand.extend_from_slice(&units[end..]);
            if Instant::now() >= deadline {
                *budget = 0;
                break;
            }
            *budget -= 1;
            if !cand.is_empty() && keep(&cand) {
                units = cand;
                reduced = true;
                n = (n - 1).max(2);
                break;
            }
            start = end;
        }
        if !reduced {
            if n >= units.len() {
                break;
            }
            n = (n * 2).min(units.len());
        }
    }
    units
}
