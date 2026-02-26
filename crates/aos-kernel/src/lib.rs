//! Deterministic kernel entry points: load manifests, run workflows, emit intents.

pub mod cap_enforcer;
pub mod capability;
pub mod cell_index;
pub mod effects;
pub mod error;
pub mod event;
pub mod governance;
pub mod governance_effects;
pub mod governance_utils;
pub mod internal_effects;
pub mod journal;
pub mod manifest;
pub mod patch_doc;
pub mod policy;
pub mod pure;
pub mod query;
pub mod receipts;
pub mod workflow;
pub mod schema_value;
pub mod secret;
pub mod shadow;
pub mod snapshot;
pub mod world;

pub use effects::{EffectManager, EffectQueue};
pub use error::KernelError;
pub use event::{KernelEvent, WorkflowEvent};
pub use manifest::{LoadedManifest, ManifestLoader};
pub use pure::PureRegistry;
pub use query::{Consistency, ReadMeta, StateRead, StateReader};
pub use workflow::WorkflowRegistry;
pub use secret::{
    MapSecretResolver, PlaceholderSecretResolver, ResolvedSecret, SecretResolver,
    SecretResolverError, SharedSecretResolver,
};
pub use shadow::{ShadowConfig, ShadowExecutor, ShadowSummary};
pub use world::{
    DefListing, Kernel, KernelBuilder, KernelConfig, KernelHeights, TailEntry, TailIntent,
    TailReceipt, TailScan,
};
