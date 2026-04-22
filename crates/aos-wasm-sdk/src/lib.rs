#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

mod shared_effects;
mod workflow_effects;
mod workflows;

pub use shared_effects::*;
pub use workflow_effects::*;
pub use workflows::*;

#[cfg(feature = "air-macros")]
pub use aos_air_macros::AirSchema;
pub use serde_cbor::Value;

/// Metadata emitted by Rust-authored AIR schema derives.
///
/// The derive macro only emits static metadata. Host-side authoring tools are responsible for
/// materializing this JSON under `air/generated/` and feeding it through the normal AIR loader.
pub trait AirSchemaExport {
    const AIR_SCHEMA_JSON: &'static str;
}

/// Metadata emitted by Rust-authored AIR workflow attributes.
///
/// The workflow macro exports the AIR module and workflow definitions as static JSON fragments.
/// Host-side authoring tools are responsible for replacing the placeholder WASM artifact hash
/// during normal generated AIR materialization.
pub trait AirWorkflowExport {
    const AIR_MODULE_JSON: &'static str;
    const AIR_WORKFLOW_JSON: &'static str;
}

/// Collect Rust-authored AIR metadata exported by this crate.
///
/// Package-local export binaries or host-side authoring glue can pass the generated constant to
/// `aos_authoring::write_generated_air_nodes`.
///
/// ```
/// # use aos_wasm_sdk::AirSchemaExport;
/// # struct TaskSubmitted;
/// # impl AirSchemaExport for TaskSubmitted {
/// #     const AIR_SCHEMA_JSON: &'static str =
/// #         r#"{"$kind":"defschema","name":"demo/TaskSubmitted@1","type":{"record":{}}}"#;
/// # }
/// aos_wasm_sdk::aos_air_exports! {
///     pub const AOS_AIR_NODES_JSON = [TaskSubmitted];
/// }
///
/// assert_eq!(AOS_AIR_NODES_JSON.len(), 1);
/// let export_payload = aos_wasm_sdk::air_exports_json(AOS_AIR_NODES_JSON);
/// assert!(export_payload.contains("demo/TaskSubmitted@1"));
/// ```
#[macro_export]
macro_rules! aos_air_exports {
    (
        $vis:vis const $name:ident = {
            schemas: [ $($schema:ty),* $(,)? ],
            workflows: [ $($workflow:ty),* $(,)? ] $(,)?
        };
    ) => {
        $vis const $name: &[&str] = &[
            $(<$schema as $crate::AirSchemaExport>::AIR_SCHEMA_JSON,)*
            $(
                <$workflow as $crate::AirWorkflowExport>::AIR_MODULE_JSON,
                <$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_JSON,
            )*
        ];
    };
    ($vis:vis const $name:ident = [ $($ty:ty),* $(,)? ];) => {
        $vis const $name: &[&str] = &[
            $(<$ty as $crate::AirSchemaExport>::AIR_SCHEMA_JSON),*
        ];
    };
}

use alloc::alloc::{Layout, alloc};
use alloc::string::String;
use core::slice;

/// Encode collected AIR JSON fragments as a JSON string array.
///
/// This is the package-local export binary contract. A Rust-authored AIR package can print this
/// payload, and host-side authoring code can feed it to
/// `aos_authoring::write_generated_air_export_json`.
pub fn air_exports_json(exports: &[&str]) -> String {
    let mut out = String::from("[");
    for (idx, export) in exports.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        push_json_string(&mut out, export);
    }
    out.push(']');
    out
}

fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{00}'..='\u{1f}' => {
                out.push_str("\\u00");
                push_hex_byte(out, ch as u8);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
}

fn push_hex_byte(out: &mut String, value: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(value >> 4) as usize] as char);
    out.push(HEX[(value & 0x0f) as usize] as char);
}

#[inline]
fn guest_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return core::ptr::null_mut();
    }
    let layout = Layout::from_size_align(len, 8).expect("layout");
    unsafe { alloc(layout) }
}

unsafe fn read_input<'a>(ptr: i32, len: i32) -> &'a [u8] {
    if ptr == 0 || len <= 0 {
        return &[];
    }
    unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) }
}

fn write_back(bytes: &[u8]) -> (i32, i32) {
    let len = bytes.len();
    if len == 0 {
        return (0, 0);
    }
    let ptr = guest_alloc(len);
    unsafe {
        let out = slice::from_raw_parts_mut(ptr as *mut u8, len);
        out.copy_from_slice(bytes);
    }
    (ptr as i32, len as i32)
}

/// Exported allocator body (used by the macro-generated `alloc`).
pub fn exported_alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    guest_alloc(len as usize) as i32
}

#[cfg(test)]
mod tests {
    use super::air_exports_json;

    #[test]
    fn air_exports_json_encodes_string_array_payload() {
        let payload = air_exports_json(&[
            r#"{"$kind":"defschema","name":"demo/Task@1","type":{"record":{}}}"#,
            "line\nbreak",
        ]);
        let decoded: Vec<String> = serde_json::from_str(&payload).expect("parse export payload");
        assert_eq!(decoded.len(), 2);
        assert_eq!(
            decoded[0],
            r#"{"$kind":"defschema","name":"demo/Task@1","type":{"record":{}}}"#
        );
        assert_eq!(decoded[1], "line\nbreak");
    }
}
