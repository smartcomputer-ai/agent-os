//! AIR v1 core data model and semantic validation (informed by spec/03-air.md).

pub mod builtins;
pub mod catalog;
mod model;
mod refs;
pub mod schema_index;
pub mod schemas;
pub mod typecheck;
pub mod validate;
pub mod value_normalize;

pub use model::*;
pub use refs::{HashRef, RefError, SchemaRef};
pub use typecheck::{ValueTypeError, validate_value_literal};
pub use validate::validate_manifest;

#[cfg(test)]
mod tests;
