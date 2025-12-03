//! Re-export manifest_loader from aos-host.
//!
//! The canonical implementation now lives in aos-host. This module re-exports
//! for backwards compatibility with existing example code.

pub use aos_host::manifest_loader::*;
