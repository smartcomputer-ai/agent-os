//! AIR expression evaluation engine plus deterministic value model.

mod expr;
mod value;

pub use expr::{Env, EvalError, EvalResult, eval_expr};
pub use value::{Value, ValueKey, ValueMap, ValueSet};
