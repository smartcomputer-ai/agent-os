# Example 00 — CounterSM

A minimal counter state machine with no micro-effects.

## Structure

- `air/` — AIR JSON definitions (schemas, module, manifest)
- `workflow/` — WASM workflow crate

## Running

```bash
# Via example runner (with replay verification)
cargo run -p aos-smoke -- counter

# Via CLI
aos world step crates/aos-smoke/fixtures/00-counter --reset-journal
aos world step crates/aos-smoke/fixtures/00-counter --event demo/CounterEvent@1 --value '{"Start": {"target": 3}}'
aos world step crates/aos-smoke/fixtures/00-counter --event demo/CounterEvent@1 --value '{"Tick": null}'
```
