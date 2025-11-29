# AgentOS v1 Pre-Ship Work Items

This folder contains work items that should be completed before AgentOS v1 ships. Each major item has its own file so different engineers can work in parallel.

## Priority Levels

- **P0**: Must fix before v1 ships. These are spec/schema inconsistencies that will cause confusion and bugs.
- **P1**: Strongly recommended for v1. These improve the design and reduce future debt.
- **P2**: Nice to have (polish). Can be done post-v1 if needed.

## Work Items

### P0: Foundational (Must Fix)

| File | Summary | Effort |
|------|---------|--------|
| [p0-schema-prose-alignment.md](p0-schema-prose-alignment.md) | Fix mismatches between `builtin-schemas.air.json` and spec prose | Small |
| [p0-governance-event-naming.md](p0-governance-event-naming.md) | Standardize governance event names across docs and code | Small |
| [p0-null-option-expr-value.md](p0-null-option-expr-value.md) | Fix confusing `{"const": {"null": {}}}` prose vs schema | Small |

### P1: Medium Changes (Strongly Recommended)

| File | Summary | Effort |
|------|---------|--------|
| [p1-derived-caps-effects.md](p1-derived-caps-effects.md) | Make `required_caps`/`allowed_effects` derived from steps | Medium |
| [p1-defer-invariants.md](p1-defer-invariants.md) | Remove underspecified `invariants` field from v1 | Small |
| [p1-defer-await-event.md](p1-defer-await-event.md) | Remove underspecified `await_event` step from v1 | Small-Medium |
| [p1-defsecret.md](p1-defsecret.md) | Introduce `defsecret` defkind for consistent manifest model | Medium |
| [p1-defeffect.md](p1-defeffect.md) | Introduce `defeffect` defkind for data-driven effect catalog | Medium-Large |

### P2: Polish

| File | Summary | Effort |
|------|---------|--------|
| [polish.md](polish.md) | 10 small improvements (can be done incrementally) | Small each |

## Suggested Order

1. **Start with P0 items** - these are quick wins that fix inconsistencies
2. **Then P1 deferrals** (`invariants`, `await_event`) - removes surface area
3. **Then `defsecret`** - medium effort but high value for manifest consistency
4. **Then `defeffect`** - medium-large effort, enables future effect extensibility
5. **Then derived caps/effects** - medium effort, good ergonomics win
6. **Polish items** - as time permits

## Dependencies

- `p1-defsecret` should be done before updating examples that use secrets
- `p1-defeffect` should be done after `p0-schema-prose-alignment` (schemas must be correct first)
- `p1-defeffect` pairs well with `p1-defsecret` (both add defkinds, similar patterns)
- `p0-schema-prose-alignment` should be done before `p1-derived-caps-effects` (cleaner baseline)
- All P0 items are independent and can be done in parallel

## Questions?

If anything is unclear, discuss in the relevant todo file or create an issue.
