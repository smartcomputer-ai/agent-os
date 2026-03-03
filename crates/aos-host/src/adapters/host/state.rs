use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Default)]
pub(crate) struct HostState {
    pub(crate) next_session_id: u64,
    pub(crate) sessions: HashMap<String, SessionRecord>,
}

#[derive(Clone)]
pub(crate) struct SessionRecord {
    pub(crate) workdir: PathBuf,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) expires_at_ns: Option<u64>,
    pub(crate) closed: bool,
    pub(crate) ended_at_ns: Option<u64>,
    pub(crate) last_exit_code: Option<i32>,
}
