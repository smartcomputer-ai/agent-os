//! AIR v1 core data model and semantic validation (informed by spec/03-air.md).

mod model;
pub mod validate;
pub mod schemas;
mod refs;

pub use model::*;
pub use refs::{HashRef, SchemaRef, RefError};
