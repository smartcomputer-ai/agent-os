//! Shared effect intent/receipt types and helpers.

pub mod builtins;
pub mod effect_ops;
pub mod normalize;

mod intent;
mod receipt;
mod stream;

pub use intent::{EffectIntent, EffectSource, IdempotencyKey, IntentBuilder, IntentEncodeError};
pub use normalize::{NormalizeError, normalize_effect_op_params, normalize_effect_params};
pub use receipt::{EffectReceipt, ReceiptDecodeError, ReceiptStatus};
pub use stream::EffectStreamFrame;
