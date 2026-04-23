//! Agent contracts for `aos.agent/*`.
//!
//! Public API is contract-first: domain-event/state/config types in `contracts`.
//! Helper reducers/mappers remain available under `helpers` for internal/runtime use.

#![no_std]

extern crate alloc;

pub mod contracts;
#[doc(hidden)]
pub mod helpers;
#[doc(hidden)]
pub mod tools;
mod workflow;
mod world;

pub use contracts::*;
pub use workflow::SessionWorkflow;
pub use world::aos_air_nodes;
