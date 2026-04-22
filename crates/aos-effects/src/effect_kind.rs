use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct EffectKind(String);

impl EffectKind {
    pub const HTTP_REQUEST: &'static str = "http.request";
    pub const BLOB_PUT: &'static str = "blob.put";
    pub const BLOB_GET: &'static str = "blob.get";
    pub const TIMER_SET: &'static str = "timer.set";
    pub const PORTAL_SEND: &'static str = "portal.send";
    pub const HOST_SESSION_OPEN: &'static str = "host.session.open";
    pub const HOST_EXEC: &'static str = "host.exec";
    pub const HOST_SESSION_SIGNAL: &'static str = "host.session.signal";
    pub const HOST_FS_READ_FILE: &'static str = "host.fs.read_file";
    pub const HOST_FS_WRITE_FILE: &'static str = "host.fs.write_file";
    pub const HOST_FS_EDIT_FILE: &'static str = "host.fs.edit_file";
    pub const HOST_FS_APPLY_PATCH: &'static str = "host.fs.apply_patch";
    pub const HOST_FS_GREP: &'static str = "host.fs.grep";
    pub const HOST_FS_GLOB: &'static str = "host.fs.glob";
    pub const HOST_FS_STAT: &'static str = "host.fs.stat";
    pub const HOST_FS_EXISTS: &'static str = "host.fs.exists";
    pub const HOST_FS_LIST_DIR: &'static str = "host.fs.list_dir";
    pub const LLM_GENERATE: &'static str = "llm.generate";
    pub const VAULT_PUT: &'static str = "vault.put";
    pub const VAULT_ROTATE: &'static str = "vault.rotate";
    pub const INTROSPECT_MANIFEST: &'static str = "introspect.manifest";
    pub const INTROSPECT_WORKFLOW_STATE: &'static str = "introspect.workflow_state";
    pub const INTROSPECT_JOURNAL_HEAD: &'static str = "introspect.journal_head";
    pub const INTROSPECT_LIST_CELLS: &'static str = "introspect.list_cells";
    pub const WORKSPACE_RESOLVE: &'static str = "workspace.resolve";
    pub const WORKSPACE_EMPTY_ROOT: &'static str = "workspace.empty_root";
    pub const WORKSPACE_LIST: &'static str = "workspace.list";
    pub const WORKSPACE_READ_REF: &'static str = "workspace.read_ref";
    pub const WORKSPACE_READ_BYTES: &'static str = "workspace.read_bytes";
    pub const WORKSPACE_WRITE_BYTES: &'static str = "workspace.write_bytes";
    pub const WORKSPACE_WRITE_REF: &'static str = "workspace.write_ref";
    pub const WORKSPACE_REMOVE: &'static str = "workspace.remove";
    pub const WORKSPACE_DIFF: &'static str = "workspace.diff";
    pub const WORKSPACE_ANNOTATIONS_GET: &'static str = "workspace.annotations_get";
    pub const WORKSPACE_ANNOTATIONS_SET: &'static str = "workspace.annotations_set";

    pub fn new(kind: impl Into<String>) -> Self {
        Self(kind.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn http_request() -> Self {
        Self::new(Self::HTTP_REQUEST)
    }

    pub fn blob_put() -> Self {
        Self::new(Self::BLOB_PUT)
    }

    pub fn blob_get() -> Self {
        Self::new(Self::BLOB_GET)
    }

    pub fn timer_set() -> Self {
        Self::new(Self::TIMER_SET)
    }

    pub fn llm_generate() -> Self {
        Self::new(Self::LLM_GENERATE)
    }

    pub fn host_session_open() -> Self {
        Self::new(Self::HOST_SESSION_OPEN)
    }

    pub fn host_exec() -> Self {
        Self::new(Self::HOST_EXEC)
    }

    pub fn host_session_signal() -> Self {
        Self::new(Self::HOST_SESSION_SIGNAL)
    }

    pub fn workspace_resolve() -> Self {
        Self::new(Self::WORKSPACE_RESOLVE)
    }

    pub fn workspace_empty_root() -> Self {
        Self::new(Self::WORKSPACE_EMPTY_ROOT)
    }

    pub fn workspace_list() -> Self {
        Self::new(Self::WORKSPACE_LIST)
    }

    pub fn workspace_read_ref() -> Self {
        Self::new(Self::WORKSPACE_READ_REF)
    }

    pub fn workspace_read_bytes() -> Self {
        Self::new(Self::WORKSPACE_READ_BYTES)
    }

    pub fn workspace_write_bytes() -> Self {
        Self::new(Self::WORKSPACE_WRITE_BYTES)
    }

    pub fn workspace_write_ref() -> Self {
        Self::new(Self::WORKSPACE_WRITE_REF)
    }

    pub fn workspace_remove() -> Self {
        Self::new(Self::WORKSPACE_REMOVE)
    }

    pub fn workspace_diff() -> Self {
        Self::new(Self::WORKSPACE_DIFF)
    }

    pub fn workspace_annotations_get() -> Self {
        Self::new(Self::WORKSPACE_ANNOTATIONS_GET)
    }

    pub fn workspace_annotations_set() -> Self {
        Self::new(Self::WORKSPACE_ANNOTATIONS_SET)
    }

    pub fn introspect_manifest() -> Self {
        Self::new(Self::INTROSPECT_MANIFEST)
    }

    pub fn introspect_workflow_state() -> Self {
        Self::new(Self::INTROSPECT_WORKFLOW_STATE)
    }

    pub fn introspect_journal_head() -> Self {
        Self::new(Self::INTROSPECT_JOURNAL_HEAD)
    }

    pub fn introspect_list_cells() -> Self {
        Self::new(Self::INTROSPECT_LIST_CELLS)
    }
}

impl<S: Into<String>> From<S> for EffectKind {
    fn from(value: S) -> Self {
        EffectKind::new(value)
    }
}

impl std::fmt::Display for EffectKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EffectKind {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(s.to_owned()))
    }
}

impl AsRef<str> for EffectKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}
