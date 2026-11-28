//! `Serialize` / `Deserialize` for [`Data`] (feature `serde`).
//!
//! A table whose keys are exactly the integers `1..=n` (in any order; `n` may be 0)
//! serializes as a sequence, in key order; any other table as a map, in its entry order.
//! `nil` serializes as a unit (`null` in JSON). Floats that are not finite, and map keys
//! the format cannot take, are the format's errors.
//!
//! Deserializing makes a sequence a table with keys `1..=n` and a map a table of its
//! entries. Absent values (`null`) are left out of tables, as a Lua table cannot hold `nil`:
//! `[1, null, 3]` has keys 1 and 3. An unsigned integer above `i64::MAX` becomes a float.
//! Nesting deeper than [`MAX_SERDE_DEPTH`] is an error, whatever the format allows, so a
//! hostile payload cannot exhaust the host's stack. Size is bounded by the format and by
//! [`Lua::from_data_with`](crate::Lua::from_data_with), which still applies when the result
//! goes into a state.

use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::Data;

/// The deepest nesting [`Data`]'s `Deserialize` follows (as deep as
/// [`Lua::from_data`](crate::Lua::from_data) builds).
pub const MAX_SERDE_DEPTH: usize = 64;

/// The table's values in key order when its keys are exactly `1..=n`.
fn as_sequence(pairs: &[(Data, Data)]) -> Option<Vec<&Data>> {
    let n = pairs.len();
    let mut slots: Vec<Option<&Data>> = vec![None; n];
    for (k, v) in pairs {
        let Data::Int(i) = k else { return None };
        let i = usize::try_from(*i).ok()?.checked_sub(1)?;
        let slot = slots.get_mut(i)?;
        if slot.replace(v).is_some() {
            return None;
        }
    }
    slots.into_iter().collect()
}

impl Serialize for Data {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Data::Nil => s.serialize_unit(),
            Data::Bool(b) => s.serialize_bool(*b),
            Data::Int(i) => s.serialize_i64(*i),
            Data::Num(f) => s.serialize_f64(*f),
            Data::Str(t) => s.serialize_str(t),
            Data::Table(pairs) => {
                if let Some(items) = as_sequence(pairs) {
                    let mut seq = s.serialize_seq(Some(items.len()))?;
                    for v in items {
                        seq.serialize_element(v)?;
                    }
                    seq.end()
                } else {
                    let mut map = s.serialize_map(Some(pairs.len()))?;
                    for (k, v) in pairs {
                        map.serialize_entry(k, v)?;
                    }
                    map.end()
                }
            }
        }
    }
}

/// `Data` at nesting `depth`.
struct At(usize);

impl<'de> DeserializeSeed<'de> for At {
    type Value = Data;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Data, D::Error> {
        if self.0 > MAX_SERDE_DEPTH {
            return Err(de::Error::custom("data nested too deeply"));
        }
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for At {
    type Value = Data;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("nil, a boolean, a number, a string, a sequence or a map")
    }

    fn visit_bool<E: de::Error>(self, b: bool) -> Result<Data, E> {
        Ok(Data::Bool(b))
    }
    fn visit_i64<E: de::Error>(self, i: i64) -> Result<Data, E> {
        Ok(Data::Int(i))
    }
    fn visit_u64<E: de::Error>(self, u: u64) -> Result<Data, E> {
        Ok(i64::try_from(u).map_or(Data::Num(u as f64), Data::Int))
    }
    fn visit_i128<E: de::Error>(self, i: i128) -> Result<Data, E> {
        Ok(i64::try_from(i).map_or(Data::Num(i as f64), Data::Int))
    }
    fn visit_u128<E: de::Error>(self, u: u128) -> Result<Data, E> {
        Ok(i64::try_from(u).map_or(Data::Num(u as f64), Data::Int))
    }
    fn visit_f64<E: de::Error>(self, f: f64) -> Result<Data, E> {
        Ok(Data::Num(f))
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Data, E> {
        Ok(Data::Str(s.to_owned()))
    }
    fn visit_string<E: de::Error>(self, s: String) -> Result<Data, E> {
        Ok(Data::Str(s))
    }
    fn visit_bytes<E: de::Error>(self, b: &[u8]) -> Result<Data, E> {
        Ok(Data::Str(String::from_utf8_lossy(b).into_owned()))
    }
    fn visit_unit<E: de::Error>(self) -> Result<Data, E> {
        Ok(Data::Nil)
    }
    fn visit_none<E: de::Error>(self) -> Result<Data, E> {
        Ok(Data::Nil)
    }
    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Data, D::Error> {
        At(self.0).deserialize(d)
    }
    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<Data, D::Error> {
        At(self.0).deserialize(d)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Data, A::Error> {
        let mut out = Vec::new();
        let mut i: i64 = 0;
        while let Some(v) = seq.next_element_seed(At(self.0 + 1))? {
            i = i.checked_add(1).ok_or_else(|| de::Error::custom("sequence too long"))?;
            if v != Data::Nil {
                out.push((Data::Int(i), v));
            }
        }
        Ok(Data::Table(out))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Data, A::Error> {
        let mut out = Vec::new();
        while let Some(k) = map.next_key_seed(At(self.0 + 1))? {
            let v = map.next_value_seed(At(self.0 + 1))?;
            if v != Data::Nil {
                out.push((k, v));
            }
        }
        Ok(Data::Table(out))
    }
}

impl<'de> Deserialize<'de> for Data {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Data, D::Error> {
        At(0).deserialize(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_detection() {
        let t = |ks: &[i64]| ks.iter().map(|&k| (Data::Int(k), Data::Int(k * 10))).collect::<Vec<_>>();
        assert_eq!(as_sequence(&t(&[])).map(|v| v.len()), Some(0));
        let shuffled = t(&[2, 3, 1]);
        let seq = as_sequence(&shuffled).map(|v| v.into_iter().cloned().collect::<Vec<_>>());
        assert_eq!(seq, Some(vec![Data::Int(10), Data::Int(20), Data::Int(30)]));
        assert!(as_sequence(&t(&[1, 3])).is_none(), "gap");
        assert!(as_sequence(&t(&[0, 1])).is_none(), "zero");
        assert!(as_sequence(&t(&[1, 1])).is_none(), "duplicate");
        assert!(as_sequence(&t(&[-1])).is_none(), "negative");
        assert!(as_sequence(&[(Data::Str("1".into()), Data::Nil)]).is_none(), "string key");
    }

    fn json(d: &Data) -> String {
        serde_json::to_string(d).unwrap()
    }

    fn parse(s: &str) -> Result<Data, serde_json::Error> {
        serde_json::from_str(s)
    }

    #[test]
    fn scalars_and_shapes() {
        assert_eq!(parse("null").unwrap(), Data::Nil);
        assert_eq!(parse("true").unwrap(), Data::Bool(true));
        assert_eq!(parse("-7").unwrap(), Data::Int(-7));
        assert_eq!(parse("2.5").unwrap(), Data::Num(2.5));
        assert_eq!(parse("\"hi\"").unwrap(), Data::Str("hi".into()));
        assert_eq!(
            parse("18446744073709551615").unwrap(),
            Data::Num(u64::MAX as f64),
            "above i64 is a float"
        );
        assert_eq!(parse("[]").unwrap(), Data::Table(vec![]));
        assert_eq!(json(&Data::Table(vec![])), "[]");
        assert_eq!(
            parse("[1, null, 3]").unwrap(),
            Data::Table(vec![(Data::Int(1), Data::Int(1)), (Data::Int(3), Data::Int(3))]),
            "null leaves a hole, as in Lua"
        );
        assert_eq!(
            parse(r#"{"a": null, "b": 1}"#).unwrap(),
            Data::Table(vec![(Data::Str("b".into()), Data::Int(1))])
        );
        assert_eq!(json(&Data::Num(f64::NAN)), "null", "serde_json writes non-finite floats as null");
    }

    #[test]
    fn json_round_trips_through_a_script() {
        let src = r#"{"name":"probe","tags":["a","b"],"pos":{"x":1,"y":2.5},"on":true}"#;
        let d = parse(src).unwrap();
        assert_eq!(json(&d), src, "entry order and sequences survive");

        let mut lua = crate::Lua::new(crate::Limits::default(), crate::Libs::STANDARD);
        let v = lua.from_data(&d).unwrap();
        lua.set_global("cfg", v).unwrap();
        let out = lua
            .exec("cfg.pos.x = cfg.pos.x + 41; cfg.tags[#cfg.tags + 1] = cfg.name; return cfg", "t")
            .unwrap();
        let back = lua.to_data(out.into_iter().next().unwrap(), crate::DataLimits::default()).unwrap();
        let back: serde_json::Value = serde_json::from_str(&json(&back)).unwrap();
        assert_eq!(back["pos"]["x"], 42);
        assert_eq!(back["pos"]["y"], 2.5);
        assert_eq!(back["tags"], serde_json::json!(["a", "b", "probe"]));
        assert_eq!(back["on"], true);
    }

    #[test]
    fn hostile_nesting_is_an_error_not_a_stack_overflow() {
        let ok = format!("{}{}", "[".repeat(MAX_SERDE_DEPTH + 1), "]".repeat(MAX_SERDE_DEPTH + 1));
        assert!(parse(&ok).is_ok(), "the limit itself is accepted");
        let deep = format!("{}{}", "[".repeat(MAX_SERDE_DEPTH + 2), "]".repeat(MAX_SERDE_DEPTH + 2));
        let e = parse(&deep).unwrap_err().to_string();
        assert!(e.contains("nested too deeply"), "{e}");
        // Far past serde_json's own recursion limit too: our error or its, never a crash.
        let bomb = "[".repeat(100_000);
        assert!(parse(&bomb).is_err());
        let mut m = "1".to_owned();
        for _ in 0..(MAX_SERDE_DEPTH + 5) {
            m = format!(r#"{{"k":{m}}}"#);
        }
        assert!(parse(&m).is_err());
    }

    #[test]
    fn deserialized_data_fits_from_data() {
        // Whatever Deserialize accepts, from_data builds (its depth bound is the same).
        let s = format!("{}1{}", "[".repeat(MAX_SERDE_DEPTH), "]".repeat(MAX_SERDE_DEPTH));
        let d = parse(&s).unwrap();
        let mut lua = crate::Lua::new(crate::Limits::default(), crate::Libs::NONE);
        lua.from_data(&d).unwrap();
    }

    #[test]
    fn non_sequence_tables_are_maps() {
        let d = Data::Table(vec![(Data::Int(1), Data::Bool(true)), (Data::Int(3), Data::Bool(false))]);
        assert_eq!(json(&d), r#"{"1":true,"3":false}"#);
        let bad = Data::Table(vec![(Data::Table(vec![]), Data::Int(1))]);
        assert!(serde_json::to_string(&bad).is_err(), "a table key is the format's error");
    }
}
