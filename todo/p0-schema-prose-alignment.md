# P0: Align Built-in Schemas with Spec Prose

**Priority**: P0 (must fix before v1 ships)
**Effort**: Small
**Risk if deferred**: High - spec/schema drift causes implementation bugs and agent confusion

## Summary

The built-in effect schemas in `spec/defs/builtin-schemas.air.json` disagree with the textual descriptions in `spec/03-air.md` §7. These mismatches must be resolved before v1 ships, as they affect what agents and tooling expect.

## Rationale

When schemas and prose disagree:
- Agents authoring AIR produce invalid payloads
- Tooling validates against the wrong contract
- Runtime behavior surprises users
- Future changes become harder (which is "correct"?)

Fixing this now is cheap. Fixing it after adoption is expensive.

## Current State: Specific Mismatches

### 1. `timer.set` key field

| Source | Says |
|--------|------|
| Prose (§7) | `key?:text` (optional) |
| Schema `sys/TimerSetParams@1` | `key: { "text": {} }` (required) |
| Schema `sys/TimerSetReceipt@1` | `key: { "text": {} }` (required) |

**Decision needed**: Is `key` required or optional?

**Recommendation**: Make `key` **optional** (`option<text>`). It's a correlation hint for routing receipts back to the right context, but simple timers (e.g., "wake me in 5 seconds") don't need it.

### 2. `cost_cents` in receipt events

| Source | Says |
|--------|------|
| Prose (§7, reducer receipt events table) | `cost_cents?:nat` (optional) |
| Schema `sys/TimerFired@1` | `cost_cents: { "nat": {} }` (required) |
| Schema `sys/BlobPutResult@1` | `cost_cents: { "nat": {} }` (required) |
| Schema `sys/BlobGetResult@1` | `cost_cents: { "nat": {} }` (required) |

**Recommendation**: Make `cost_cents` **optional** (`option<nat>`). Not all deployments track costs. Adapters that don't know the cost shouldn't have to invent a value.

### 3. LLM params (`tools` and `api_key`)

| Source | Says |
|--------|------|
| Prose (§7) | `tools?:list<text>` only |
| Schema `sys/LlmGenerateParams@1` | Has both `tools: option<list<text>>` AND `api_key: option<TextOrSecretRef>` |

**Recommendation**: Update prose to include `api_key?: TextOrSecretRef`. The schema is richer and correct; prose just needs to catch up.

### 4. `TimerFired.reducer` type

| Source | Says |
|--------|------|
| Prose | `reducer: Name` |
| Schema `sys/TimerFired@1` | `reducer: { "text": {} }` |

**Recommendation**: Use `text` in the schema (as it is), but document that "this field contains a Name-formatted string." The schema doesn't have a `Name` primitive type, so `text` is correct. Update prose to say `reducer: text` (Name format).

### 5. Secret version minimum

| Source | Says |
|--------|------|
| Manifest schema | `version` has `minimum: 1` |
| `sys/SecretRef@1` | `version: { "nat": {} }` (allows 0) |

**Recommendation**: Add semantic validation that secret versions must be ≥1. The schema type is `nat` (correct), but the validator should reject version 0.

## Implementation Plan

### Step 1: Update `spec/defs/builtin-schemas.air.json`

```diff
// sys/TimerSetParams@1
- "key": { "text": {} }
+ "key": { "option": { "text": {} } }

// sys/TimerSetReceipt@1
- "key": { "text": {} }
+ "key": { "option": { "text": {} } }

// sys/TimerFired@1, sys/BlobPutResult@1, sys/BlobGetResult@1
- "cost_cents": { "nat": {} }
+ "cost_cents": { "option": { "nat": {} } }
```

### Step 2: Update `spec/03-air.md` §7

- Add `api_key?: TextOrSecretRef` to `llm.generate` params
- Change `reducer: Name` to `reducer: text` (Name format) in receipt events table
- Verify all other fields match

### Step 3: Update Rust types and adapters

- `aos-air-types`: Update any structs that model these schemas
- `aos-kernel`: Update receipt handling to handle optional fields
- Adapters: Update to produce optional cost_cents

### Step 4: Update tests

- Golden hash tests in `aos-cbor` will need new expected hashes
- Add tests for optional key/cost_cents handling

## Acceptance Criteria

- [x] Every field in `builtin-schemas.air.json` matches `03-air.md` §7 exactly
- [x] Prose and schema agree on required vs optional for all fields
- [x] Golden hash tests pass with updated schemas
- [x] Example 01-hello-timer works with optional key
- [x] Adapters correctly handle optional cost_cents
