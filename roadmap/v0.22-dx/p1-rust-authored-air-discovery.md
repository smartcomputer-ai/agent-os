# P1: Rust-Authored AIR And Package Discovery

Status: planned.

## Goal

Add a Rust-first authoring lane for AIR definitions without removing the hand-authored AIR directory
lane.

Primary outcome:

1. Rust workflow crates can generate `defschema`, `defmodule`, `defworkflow`, routing, and manifest
   refs from Rust code and attributes.
2. `aos-authoring` can discover AIR from both authored JSON directories and generated Rust package
   metadata.
3. Cargo packages such as `aos-agent` can export reusable AIR without every consumer hand-writing
   `aos.sync.json` import entries.

The kernel should continue to consume only canonical AIR. Rust authoring is sugar that produces the
same `AirNode`s the current loader already knows how to store, hash, validate, bundle, and upload.

## Design Stance

### 0) Choose procedural macros for the primary Rust DX

The primary Rust authoring surface should use procedural macros.

That gives the intended developer experience:

```text
write normal Rust structs/enums/workflows
add focused AOS attributes where AIR needs explicit identity or semantics
let the compiler generate AIR metadata
```

This deliberately accepts one small proc-macro crate as the cost of avoiding duplicated schema
field lists and hand-maintained Rust/AIR drift.

### 1) Generated AIR is a peer source, not a hidden runtime path

The active authoring model becomes:

```text
AIR source =
  hand-authored AIR directory
  generated Rust-authored package/world
```

Both sources produce `AirNode`s. After that point, the existing path remains authoritative:

```text
AirNode -> CAS -> manifest ref patching -> semantic validation -> loaded manifest -> kernel
```

Generated AIR should be inspectable. The CLI should be able to print or materialize generated AIR
for debugging and CI diffs.

### 2) Rust annotations should be explicit where AIR and serde differ

Serde shape is useful, but AIR has stronger identity and typing rules. The derive/macro surface
should require explicit annotations for cases that cannot be inferred safely:

1. schema names,
2. external schema refs,
3. `time`, `duration`, `hash`, and `uuid` semantics,
4. newtype schema representation,
5. variant tagging,
6. workflow key schema,
7. emitted effects,
8. routing key fields.

Unsupported Rust shapes should fail during generation with actionable diagnostics instead of
silently producing loose AIR.

### 3) Package discovery should be metadata-driven

Reusable crates should advertise AIR exports through Cargo metadata, not through each downstream
world's sync file.

Illustrative shape:

```toml
[package.metadata.aos]
air = "generated"
air_dir = ".aos/generated/air"
exports = true
```

The exact fields can change during implementation, but the metadata should answer:

1. does this package export AIR,
2. where the generated or checked-in AIR can be found,
3. which build target provides workflow WASM modules,
4. what lock identity protects imported defs.

## Non-Goals

- Do not remove hand-authored AIR support.
- Do not migrate every smoke fixture.
- Do not redesign AIR v2 root forms.
- Do not add Python workflow or Python effect execution.
- Do not implement custom Rust/WASI effects in this phase.
- Do not make the kernel understand Rust metadata directly.

## Phase 1A: Rust Schema Generation

Add a Rust AIR schema generation surface using procedural macros.

Rust requires derive and attribute procedural macros to live in a `proc-macro` crate, so P1 should
add exactly one new crate for this purpose. The crate should stay thin: parse Rust syntax, validate
the supported authoring shape, and emit metadata/glue code. It should not become the authoring
loader or AIR model owner.

Crate placement:

```text
crates/aos-air-macros       derive/attribute procedural macros; compile-time only
crates/aos-wasm-sdk         re-export macros and own no_std workflow ABI helpers
crates/aos-authoring        generated metadata discovery, AIR assembly, Cargo discovery
```

Avoid making `aos-air-types` depend on proc-macro infrastructure. It should remain the AIR data
model and validation crate, not the authoring frontend.

Do not add a second support crate in P1. If shared helper code becomes necessary, prefer existing
crates first:

1. pure AIR data types stay in `aos-air-types`,
2. workflow/runtime-side helpers stay in `aos-wasm-sdk`,
3. std-side generation and loading helpers stay in `aos-authoring`.

Initial derive surface:

```rust
#[derive(Serialize, Deserialize, AirSchema)]
#[aos(schema = "demo/TaskSubmitted@1")]
pub struct TaskSubmitted {
    #[aos(ref = "aos.agent/SessionId@1")]
    pub task_id: SessionId,
    #[aos(type = "time")]
    pub observed_at_ns: u64,
    pub task: String,
}
```

Supported first subset:

1. structs with named fields -> `record`,
2. serde-tagged enums -> `variant`,
3. `String` -> `text`,
4. `bool` -> `bool`,
5. `u64` -> `nat`,
6. `i64` -> `int`,
7. `Vec<T>` -> `list<T>`,
8. `Option<T>` -> `option<T>`,
9. `BTreeMap<String, T>` -> `map<text, T>`,
10. explicit `#[aos(ref = "...")]` for external or reused schemas.

Required checks:

1. every generated schema has a stable AIR name,
2. duplicate schema names with different generated types fail,
3. unsupported field types fail,
4. generated schemas round-trip through `aos-air-types` JSON and canonical CBOR.

## Phase 1B: Rust Workflow Metadata

Extend the workflow macro surface so the same Rust declaration that exports the WASM ABI also emits
AIR workflow metadata.

Illustrative shape:

```rust
#[aos_workflow(
    name = "demo/Counter@1",
    module = "demo/Counter_wasm@1",
    entrypoint = "step",
    state = CounterState,
    event = CounterEvent,
    context = "sys/WorkflowContext@1",
    key = "demo/CounterId@1",
    effects = ["sys/timer.set@1"],
    routes = [
        { event = "demo/CounterEvent@1", key_field = "counter_id" }
    ]
)]
#[derive(Default)]
struct Counter;
```

Generated definitions:

1. `defmodule` for the WASM artifact with placeholder hash,
2. `defworkflow` with state/event/context/key/effect allowlist,
3. manifest refs for local schemas/modules/workflows/effects/secrets,
4. routing subscriptions for domain ingress.

The existing `aos_workflow!(Ty)` macro can remain as a low-level ABI-only escape hatch. The new
surface should be the Rust-authored AIR path.

## Phase 1C: Generated AIR Materialization And Loader Integration

Add an `AirSource` layer inside `aos-authoring` so callers no longer assume that all local defs live
under one `air/` directory.

Target shape:

```text
AirSource::Directory(path)
AirSource::GeneratedRustPackage(package metadata)
```

Implementation should:

1. collect hand-authored AIR nodes from directories exactly as today,
2. invoke or read generated Rust AIR for packages that expose it,
3. merge all defs before manifest ref patching,
4. preserve the current duplicate-name behavior,
5. keep imported manifests ignored when they come from import roots,
6. produce the same `WorldBundle` shape as the current build path.

Generated AIR can first be materialized to:

```text
.aos/generated/air/
```

or:

```text
target/aos/air/
```

The final location matters less than having a stable loader contract and an inspectable output.

## Phase 1D: Cargo Package Discovery

Teach `aos-authoring` to discover AOS-exporting packages through `cargo metadata`.

Current consumer shape:

```json
{
  "air": {
    "imports": [
      {
        "cargo": {
          "package": "aos-agent",
          "air_dir": "air"
        }
      }
    ]
  }
}
```

Target consumer shape:

```text
depend on aos-agent in Cargo.toml
run aos build/push
authoring discovers aos-agent AIR exports automatically
```

Discovery rules:

1. inspect direct dependencies of the local workflow package first,
2. only import packages that opt in through `package.metadata.aos`,
3. support both checked-in AIR directories and generated AIR outputs,
4. compute a defs hash from imported definitions,
5. compare that hash with a lock identity in strict mode,
6. produce warnings locally and hard errors in CI, matching current import lock behavior.

The existing explicit `aos.sync.json` imports should remain valid. They are useful for smoke tests,
non-Cargo import roots, and migration debugging.

## Phase 1E: Developer Commands And CI Hooks

Add CLI surfaces that make Rust-generated AIR visible.

Recommended commands:

```text
aos air generate --manifest-path <Cargo.toml> --package <pkg>
aos air print --world <world-root>
aos air check --world <world-root>
```

Required behavior:

1. generation is deterministic,
2. `check` fails if checked-in generated AIR is stale,
3. diagnostics point to Rust source annotations where practical,
4. output can be diffed as ordinary AIR JSON.

## Testing

Add focused tests before migrating large worlds:

1. derive one record schema,
2. derive one variant schema with refs,
3. generate one workflow/module/manifest/routing set,
4. merge generated AIR with hand-authored AIR,
5. auto-discover one Cargo dependency export,
6. preserve explicit `aos.sync.json` import behavior,
7. reject duplicate generated/hand-authored definitions with different hashes.

The first green target should be:

```text
cargo test -p aos-air-types
cargo test -p aos-authoring
cargo test -p aos-wasm-sdk
```

Add CLI tests once the command surface exists.

## Exit Criteria

P1 is complete when:

1. a small Rust-authored fixture can build a valid world bundle with no local `air/*.json`,
2. a package can export AIR through Cargo metadata,
3. another workflow crate can discover that package's AIR without manual sync import wiring,
4. hand-authored smoke fixtures still build through the existing AIR directory path,
5. generated AIR can be printed or materialized for inspection.
