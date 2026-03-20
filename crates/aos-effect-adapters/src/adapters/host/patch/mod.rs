mod apply;
mod edit;
mod matching;
mod parser;
mod types;

pub(crate) use apply::apply_update_hunks;
pub(crate) use edit::{EditMatchError, apply_edit};
pub(crate) use parser::parse_patch_v4a;
pub(crate) use types::{ParsedPatch, PatchOpCounts, PatchOperation};
