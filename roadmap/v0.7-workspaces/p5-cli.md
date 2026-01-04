update the cli to work with the new workspace system instead of the old object registry.

scope:
- replace `aos obj` with `aos ws` (ls/log/cat/edit or equivalents)
- wire CLI reads to `workspace.resolve` + tree ops (`list`, `read_bytes`, `read_ref`)
- wire CLI writes to `workspace.write_bytes` / `workspace.remove`
- keep world_io/import/export changes out of this doc (see p7)
