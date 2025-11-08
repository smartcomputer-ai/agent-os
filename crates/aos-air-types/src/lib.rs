//! AIR v1 core data model and semantic validation (informed by spec/03-air.md).

pub mod builtins;
mod model;
mod refs;
pub mod schemas;
pub mod validate;

pub use model::*;
pub use refs::{HashRef, RefError, SchemaRef};
