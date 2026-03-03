#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PatchOpCounts {
    pub(crate) add: u64,
    pub(crate) update: u64,
    pub(crate) delete: u64,
    pub(crate) move_count: u64,
}

impl core::fmt::Display for PatchOpCounts {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "add={} update={} delete={} move={}",
            self.add, self.update, self.delete, self.move_count
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedPatch {
    pub(crate) operations: Vec<PatchOperation>,
    pub(crate) counts: PatchOpCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatchOperation {
    AddFile {
        path: String,
        lines: Vec<String>,
    },
    DeleteFile {
        path: String,
    },
    UpdateFile {
        path: String,
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PatchHunk {
    pub(crate) header: String,
    pub(crate) lines: Vec<PatchHunkLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PatchHunkLine {
    Context(String),
    Delete(String),
    Add(String),
    EndOfFile,
}
