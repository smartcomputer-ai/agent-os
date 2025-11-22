//! Shared effect intent/receipt/capability types and helpers.

pub mod builtins;
pub mod normalize;

mod capability;
mod intent;
mod kinds;
mod receipt;
pub mod traits;

pub use capability::{
    CapabilityBudget, CapabilityEncodeError, CapabilityGrant, CapabilityGrantBuilder,
};
pub use intent::{EffectIntent, EffectSource, IdempotencyKey, IntentBuilder, IntentEncodeError};
pub use kinds::EffectKind;
pub use receipt::{EffectReceipt, ReceiptDecodeError, ReceiptStatus};
pub use normalize::{normalize_effect_params, NormalizeError};
