//! Deterministic kernel entry points: load manifests, run reducers, emit intents.

pub mod capability;
pub mod effects;
pub mod error;
pub mod event;
pub mod journal;
pub mod manifest;
pub mod plan;
pub mod policy;
pub mod receipts;
pub mod reducer;
pub mod snapshot;
pub mod scheduler;
pub mod world;

pub use effects::{EffectManager, EffectQueue};
pub use error::KernelError;
pub use event::{KernelEvent, ReducerEvent};
pub use manifest::{LoadedManifest, ManifestLoader};
pub use reducer::ReducerRegistry;
pub use world::{Kernel, KernelBuilder};
