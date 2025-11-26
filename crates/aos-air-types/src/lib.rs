//! AIR v1 core data model and semantic validation (informed by spec/03-air.md).

pub mod builtins;
pub mod catalog;
mod model;
pub mod plan_literals;
mod refs;
pub mod schemas;
pub mod typecheck;
pub mod validate;

pub use model::*;
pub use refs::{HashRef, RefError, SchemaRef};
pub use typecheck::{ValueTypeError, validate_value_literal};

#[cfg(test)]
mod tests;
