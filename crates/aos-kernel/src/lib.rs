//! Deterministic kernel entry points: load manifests, run reducers, emit intents.

pub mod capability;
pub mod cap_ledger;
pub mod cap_enforcer;
pub mod cell_index;
pub mod effects;
pub mod error;
pub mod event;
pub mod governance;
pub mod internal_effects;
pub mod journal;
pub mod manifest;
pub mod patch_doc;
pub mod plan;
pub mod policy;
pub mod pure;
pub mod query;
pub mod receipts;
pub mod reducer;
pub mod scheduler;
pub mod schema_value;
pub mod secret;
pub mod shadow;
pub mod snapshot;
pub mod world;

pub use effects::{EffectManager, EffectQueue};
pub use error::KernelError;
pub use event::{KernelEvent, ReducerEvent};
pub use manifest::{LoadedManifest, ManifestLoader};
pub use pure::PureRegistry;
pub use query::{Consistency, ReadMeta, StateRead, StateReader};
pub use reducer::ReducerRegistry;
pub use secret::{
    MapSecretResolver, PlaceholderSecretResolver, ResolvedSecret, SecretResolver,
    SecretResolverError, SharedSecretResolver,
};
pub use shadow::{ShadowConfig, ShadowExecutor, ShadowSummary};
pub use world::{
    DefListing, Kernel, KernelBuilder, KernelConfig, KernelHeights, PlanResultEntry, TailIntent,
    TailReceipt, TailScan,
};
