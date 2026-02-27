//! Agent SDK contracts and helpers.
//!
//! This crate provides P2.1 scaffolding for `aos.agent/*` session lifecycle
//! contracts and deterministic control helpers.

#![no_std]

extern crate alloc;

pub mod contracts;
pub mod helpers;

pub use contracts::*;
pub use helpers::*;
