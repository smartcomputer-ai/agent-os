Legacy hosted worker code that is not on the compiled worker path.

The active hosted center is defined by `worker/mod.rs` and currently compiles:

- `checkpoint.rs`
- `commands.rs`
- `core.rs`
- `domains.rs`
- `journal.rs`
- `layers.rs`
- `projections.rs`
- `runtime.rs`
- `scheduler.rs`
- `supervisor.rs`
- `types.rs`
- `util.rs`
- `worlds.rs`

Files in this directory are kept only as transitional reference material while the
roadmap cutover finishes. They should not be used as the authoritative hosted runtime or as the
source of truth for current hosted command-state semantics.
