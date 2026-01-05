# Tutorial: CLI workflow for a simple world

This walkthrough builds a minimal "Notes" world: one reducer that counts notes
and stores the last note text. It shows the core loop: edit AIR + reducer,
run the world, send events, and then export/import for upgrades.

If you do not have a standalone `aos` binary, replace `aos` with
`cargo run -p aos-cli --` in the commands below.

## 1) Initialize a new world
```
aos init ./notes-world
cd ./notes-world
```

## 2) Define AIR (schemas + module + routing)
Create a single defs bundle:
```
cat > air/defs.air.json <<'JSON'
[
  {
    "$kind": "defschema",
    "name": "demo/NoteState@1",
    "type": {
      "record": {
        "count": { "nat": {} },
        "last": { "text": {} }
      }
    }
  },
  {
    "$kind": "defschema",
    "name": "demo/NoteEvent@1",
    "type": {
      "record": {
        "text": { "text": {} }
      }
    }
  },
  {
    "$kind": "defmodule",
    "name": "demo/Notes@1",
    "module_kind": "reducer",
    "abi": {
      "reducer": {
        "state": "demo/NoteState@1",
        "event": "demo/NoteEvent@1",
        "effects_emitted": [],
        "cap_slots": {}
      }
    }
  }
]
JSON
```

Update `air/manifest.air.json` to reference the defs and route events:
```
cat > air/manifest.air.json <<'JSON'
{
  "$kind": "manifest",
  "air_version": "1",
  "schemas": [
    { "name": "demo/NoteState@1" },
    { "name": "demo/NoteEvent@1" }
  ],
  "modules": [
    { "name": "demo/Notes@1" }
  ],
  "plans": [],
  "effects": [],
  "caps": [],
  "policies": [],
  "secrets": [],
  "routing": {
    "events": [
      { "event": "demo/NoteEvent@1", "reducer": "demo/Notes@1" }
    ],
    "inboxes": []
  },
  "triggers": []
}
JSON
```

## 3) Write the reducer
Create `reducer/Cargo.toml`:
```
cat > reducer/Cargo.toml <<'TOML'
[workspace]

[package]
name = "notes-reducer"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
aos-wasm-sdk = { path = "../../crates/aos-wasm-sdk" }
serde = { version = "1", features = ["derive"] }
TOML
```
Note: the `aos-wasm-sdk` path assumes `notes-world` lives in the repo root.
Adjust the path if your world lives elsewhere.

Create `reducer/src/lib.rs`:
```
cat > reducer/src/lib.rs <<'RS'
#![allow(improper_ctypes_definitions)]
#![no_std]

extern crate alloc;

use alloc::string::String;
use aos_wasm_sdk::{aos_reducer, ReduceError, Reducer, ReducerCtx};
use serde::{Deserialize, Serialize};

aos_reducer!(NotesSm);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct NoteState {
    count: u64,
    last: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NoteEvent {
    text: String,
}

#[derive(Default)]
struct NotesSm;

impl Reducer for NotesSm {
    type State = NoteState;
    type Event = NoteEvent;
    type Ann = ();

    fn reduce(
        &mut self,
        event: Self::Event,
        ctx: &mut ReducerCtx<Self::State, ()>,
    ) -> Result<(), ReduceError> {
        ctx.state.count = ctx.state.count.saturating_add(1);
        ctx.state.last = event.text;
        Ok(())
    }
}
RS
```

## 4) Run and exercise the world
Start the daemon (in one terminal):
```
aos run
```

Send an event (in another terminal):
```
aos event send demo/NoteEvent@1 '{"text":"first note"}'
```

Query state:
```
aos state get demo/Notes@1
```
Note: record events use the schema-defined fields directly.

## 5) Export/edit/import (upgrade flow)
Export the current world layout for editing:
```
aos export --out /tmp/notes-checkout --defs-bundle --with-sys
```

Edit `/tmp/notes-checkout/air/*` (e.g., add a new schema field or new event).

Preview the patch document:
```
aos import --air /tmp/notes-checkout/air --import-mode patch --air-only --dry-run
```

Apply via governance (daemon required):
```
aos import --air /tmp/notes-checkout/air --import-mode patch --air-only \
  --propose --shadow --approve --apply
```
If you change `air/` or `reducer/`, restart `aos run` so the daemon reloads the assets.

## 6) Optional: workspace sync (deferred)
Source sync is handled by workspaces; see `roadmap/v0.7-workspaces/p7-fs-sync.md`.
