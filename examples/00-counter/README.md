# Example 00 — CounterSM

A minimal counter state machine with no micro-effects.

## Structure

- `air/` — AIR JSON definitions (schemas, module, manifest)
- `reducer/` — WASM reducer crate

## Running

```bash
# Via example runner (with replay verification)
cargo run -p aos-examples -- counter

# Via CLI
aos world step examples/00-counter --reset-journal
aos world step examples/00-counter --event demo/CounterEvent@1 --value '{"Start": {"target": 3}}'
aos world step examples/00-counter --event demo/CounterEvent@1 --value '"Tick"'
```
