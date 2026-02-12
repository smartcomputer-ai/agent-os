//! Unified LLM client library for Forge.
//!
//! This crate follows the specification in `spec/01-unified-llm-spec.md` and
//! is organized into the four-layer architecture described there.

pub mod anthropic;
pub mod catalog;
pub mod client;
pub mod errors;
pub mod high_level;
pub mod openai;
pub mod provider;
pub mod stream;
pub mod types;
pub mod utils;

#[allow(unused_imports)]
pub use anthropic::*;
#[allow(unused_imports)]
pub use catalog::*;
#[allow(unused_imports)]
pub use client::*;
#[allow(unused_imports)]
pub use errors::*;
#[allow(unused_imports)]
pub use high_level::*;
#[allow(unused_imports)]
pub use openai::*;
#[allow(unused_imports)]
pub use provider::*;
#[allow(unused_imports)]
pub use stream::*;
#[allow(unused_imports)]
pub use types::*;
