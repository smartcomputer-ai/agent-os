use std::collections::{BTreeMap, BTreeSet};

use aos_air_types::HashRef;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Deterministic value representation used by the AIR expression evaluator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Unit,
    Null,
    Bool(bool),
    Int(i64),
    Nat(u64),
    Dec128(String),
    #[serde(with = "serde_bytes")]
    Bytes(Vec<u8>),
    Text(String),
    TimeNs(u64),
    DurationNs(i64),
    Hash(HashRef),
    Uuid(String),
    List(Vec<Value>),
    Set(ValueSet),
    Map(ValueMap),
    Record(IndexMap<String, Value>),
}

impl Default for Value {
    fn default() -> Self {
        Self::Unit
    }
}

impl Value {
    /// Human-readable kind string used in error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            Value::Unit => "unit",
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Nat(_) => "nat",
            Value::Dec128(_) => "dec128",
            Value::Bytes(_) => "bytes",
            Value::Text(_) => "text",
            Value::TimeNs(_) => "time_ns",
            Value::DurationNs(_) => "duration_ns",
            Value::Hash(_) => "hash",
            Value::Uuid(_) => "uuid",
            Value::List(_) => "list",
            Value::Set(_) => "set",
            Value::Map(_) => "map",
            Value::Record(_) => "record",
        }
    }

    /// Convenience helper to build a record from field/value pairs.
    pub fn record(fields: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Self {
        let mut map = IndexMap::new();
        for (key, value) in fields.into_iter() {
            map.insert(key.into(), value);
        }
        Value::Record(map)
    }
}

impl From<()> for Value {
    fn from(_: ()) -> Self {
        Value::Unit
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Value::Nat(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::Text(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::Text(value.to_owned())
    }
}

/// Key type for maps/sets (limited to schemas' comparable primitives).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ValueKey {
    Int(i64),
    Nat(u64),
    Text(String),
    Hash(String),
    Uuid(String),
}

pub type ValueSet = BTreeSet<ValueKey>;
pub type ValueMap = BTreeMap<ValueKey, Value>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_reports_variant_name() {
        assert_eq!(Value::Text("hello".into()).kind(), "text");
        assert_eq!(Value::Nat(5).kind(), "nat");
    }

    #[test]
    fn record_helper_builds_indexmap() {
        let value = Value::record([("first", Value::Int(1)), ("second", Value::Bool(true))]);
        match value {
            Value::Record(map) => {
                assert_eq!(map.get("first"), Some(&Value::Int(1)));
                assert_eq!(map.get("second"), Some(&Value::Bool(true)));
            }
            other => panic!("expected record, got {other:?}"),
        }
    }

    #[test]
    fn value_keys_order_deterministically() {
        let mut set = ValueSet::new();
        set.insert(ValueKey::Text("beta".into()));
        set.insert(ValueKey::Text("alpha".into()));
        let ordered: Vec<_> = set.into_iter().collect();
        assert_eq!(
            ordered,
            vec![
                ValueKey::Text("alpha".into()),
                ValueKey::Text("beta".into()),
            ]
        );
    }
}
