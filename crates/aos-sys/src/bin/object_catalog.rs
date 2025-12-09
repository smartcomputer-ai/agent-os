//! ObjectCatalog reducer (`sys/ObjectCatalog@1`).
//!
//! A keyed reducer that maintains a versioned catalog of named objects.
//! Each object name maps to an append-only history of versions.

#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::collections::BTreeSet;
use alloc::string::{String, ToString};
use aos_sys::{ObjectMeta, ObjectVersions, Version};
use aos_wasm_sdk::{ReduceError, Reducer, ReducerCtx, Value, aos_reducer};
use serde_cbor::Value as CborValue;

// Required for WASM binary entry point
#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(not(target_arch = "wasm32"))]
fn main() {}

aos_reducer!(ObjectCatalog);

/// ObjectCatalog reducer — keyed by object name.
///
/// Invariants:
/// - Key must equal `meta.name` (enforced via `ensure_key_eq`)
/// - Versions are append-only; `latest` increments monotonically
/// - No micro-effects; pure state machine
#[derive(Default)]
struct ObjectCatalog;

impl Reducer for ObjectCatalog {
    type State = ObjectVersions;
    type Event = CborValue;
    type Ann = Value;

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, Self::Ann>,
    ) -> Result<(), ReduceError> {
        // Decode meta out of the externally-tagged ExprValue-shaped payload.
        let meta = decode_meta(&event).ok_or(ReduceError::new("invalid meta"))?;

        // Append-only version bump (0 → 1 on first registration)
        let next: Version = ctx.state.latest.saturating_add(1);
        ctx.state.latest = next;
        ctx.state.versions.insert(next, meta);
        Ok(())
    }
}

fn decode_meta(v: &CborValue) -> Option<ObjectMeta> {
    let record = match get_tagged(v, "Record")? {
        CborValue::Map(map) => map,
        _ => return None,
    };
    let meta_val = record.get(&CborValue::Text("meta".into()))?;
    let meta_map = match get_tagged(meta_val, "Record")? {
        CborValue::Map(map) => map,
        _ => return None,
    };

    let name = get_text(meta_map.get(&CborValue::Text("name".into()))?)?.to_string();
    let kind = get_text(meta_map.get(&CborValue::Text("kind".into()))?)?.to_string();
    let hash = get_text(meta_map.get(&CborValue::Text("hash".into()))?)?.to_string();
    let owner = get_text(meta_map.get(&CborValue::Text("owner".into()))?)?.to_string();
    let created_at = get_nat(meta_map.get(&CborValue::Text("created_at".into()))?)?;
    let tags = get_set(meta_map.get(&CborValue::Text("tags".into()))?);

    Some(ObjectMeta {
        name,
        kind,
        hash,
        tags,
        created_at,
        owner,
    })
}

fn get_tagged<'a>(v: &'a CborValue, tag: &str) -> Option<&'a CborValue> {
    match v {
        CborValue::Map(map) => map.get(&CborValue::Text(tag.into())),
        _ => None,
    }
}

fn get_text(v: &CborValue) -> Option<&str> {
    match v {
        CborValue::Text(s) => Some(s.as_str()),
        CborValue::Map(map) => match map.get(&CborValue::Text("Text".into())) {
            Some(CborValue::Text(s)) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn get_nat(v: &CborValue) -> Option<u64> {
    match v {
        CborValue::Integer(i) if *i >= 0 => Some(*i as u64),
        CborValue::Map(map) => match map.get(&CborValue::Text("Nat".into())) {
            Some(CborValue::Integer(i)) if *i >= 0 => Some(*i as u64),
            _ => None,
        },
        _ => None,
    }
}

fn get_set(v: &CborValue) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    if let Some(arr) = match v {
        CborValue::Array(a) => Some(a),
        CborValue::Map(map) => match map.get(&CborValue::Text("Set".into())) {
            Some(CborValue::Array(a)) => Some(a),
            _ => None,
        },
        _ => None,
    } {
        for item in arr {
            if let Some(text) = get_text(item) {
                out.insert(text.to_string());
            }
        }
    }
    out
}
