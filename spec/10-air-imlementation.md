# AIR v1 Implementation Guide

This guide describes how to implement AIR v1 and its execution inside AgentOS. It reflects the current spec (spec/03-air.md) and the JSON Schemas under spec/schemas. The aim is a small, deterministic control‑plane executor — not a general language VM — that one engineer can build and evolve.

Scope and non‑goals
- Control‑plane only: schemas, modules, plans, capabilities, policies, manifest.
- Deterministic, single‑threaded world core; effects at the edges with signed receipts.
- Plans are finite DAGs; expressions are total and side‑effect‑free.
- v1 avoids the WASM Component Model and complex policy engines. Keep seams to add them later.

Recommended crate layout
- aos-air-types: AIR data types (serde), JSON Schema bundling, Expr AST
- aos-air-validate: structural + semantic validation (shape, references, DAG checks)
- aos-cbor: canonical CBOR encode/decode + hashing
- aos-store: content-addressed store (nodes/blobs), manifest loader
- aos-wasm: deterministic Wasm runner (reducer ABI)
- aos-effects: effect intent/receipt schema and adapters interface
- aos-kernel: plan executor, capability/policy gates, journal/snapshots, world loop
- aos-cli: commands to init world, propose/shadow/apply, run, tail logs, etc.

Core dependencies
- serde, serde_json
- serde_cbor (use canonical serializer)
- sha2 (SHA-256), hex, rand (seeded RNG)
- thiserror, anyhow/color-eyre
- ed25519-dalek (or HMAC from ring) for receipt signing
- wasmtime for core Wasm
- petgraph for DAG checks
- globset for policy matching
- indexmap for deterministic maps
- tokio (optional) for adapters; core kernel remains single-threaded

1) Canonical CBOR and hashing

Ensure every persisted object (AIR node, reducer state, manifest, receipt) has a single canonical byte representation.

```rust
// aos-cbor/src/lib.rs
use serde::Serialize;
use sha2::{Digest, Sha256};

pub fn to_canonical_cbor<T: Serialize>(value: &T) -> Result<Vec<u8>, serde_cbor::Error> {
    // serde_cbor supports canonical mode
    let mut buf = Vec::with_capacity(256);
    let mut ser = serde_cbor::ser::Serializer::new(&mut buf);
    ser.self_describe();           // optional but useful
    ser.canonical();               // RFC 8949 deterministic encoding
    value.serialize(&mut ser)?;
    Ok(buf)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hash([u8; 32]);

impl Hash {
    pub fn of_cbor<T: Serialize>(v: &T) -> Self {
        let bytes = to_canonical_cbor(v).expect("canonical CBOR");
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let out = hasher.finalize();
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&out);
        Hash(arr)
    }
    pub fn to_hex(&self) -> String { format!("sha256:{:x}", hex::encode(self.0)) }
}
```

2) AIR types (serde models)

Mirror the JSON Schemas you defined so you can parse/validate and then operate on strong types.

```rust
// aos-air-types/src/types.rs
use serde::{Deserialize, Serialize};
use indexmap::IndexMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", tag = "$kind")]
pub enum AirNode {
    DefSchema(DefSchema),
    DefModule(DefModule),
    DefPlan(DefPlan),
    DefCap(DefCap),
    DefPolicy(DefPolicy),
    Manifest(Manifest),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schemas: Vec<NamedRef>,
    pub modules: Vec<NamedRef>,
    pub plans:   Vec<NamedRef>,
    pub caps:    Vec<NamedRef>,
    pub policies:Vec<NamedRef>,
    #[serde(default)]
    pub defaults: Option<Defaults>,
    #[serde(default)]
    pub module_bindings: IndexMap<String, ModuleBinding>,
    #[serde(default)]
    pub routing: Option<Routing>,
    #[serde(default)]
    pub triggers: Vec<Trigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Trigger { pub event: String, pub plan: String, #[serde(default)] pub correlate_by: Option<String> }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedRef { pub name: String, pub hash: String }

// … DefSchema/TypeExpr per schema; DefModule/DefPlan per schema …
```

Expressions and step unions:

```rust
// aos-air-types/src/expr.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Expr {
    #[serde(rename="ref")]
    Ref { r#ref: String },
    Bool { bool: bool },
    Int { int: i64 },
    Nat { nat: u64 },
    Dec128 { dec128: String },          // v1 use string; convert at runtime
    Text { text: String },
    BytesB64 { bytes_b64: String },
    TimeNs { time_ns: u64 },
    DurationNs { duration_ns: i64 },
    Hash { hash: String },
    Uuid { uuid: String },
    Op { op: Op, args: Vec<Expr> },
    Record { record: indexmap::IndexMap<String, Expr> },
    List { list: Vec<Expr> },
    Set { set: Vec<Expr> },
    Map { map: Vec<(Expr, Expr)> },
    Variant { variant: VariantExpr },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantExpr { pub tag: String, pub value: Option<Expr> }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Len, Get, Has, Eq, Ne, Lt, Le, Gt, Ge,
    And, Or, Not, Concat, Add, Sub, Mul, Div, Mod,
    StartsWith, EndsWith, Contains,
}
```

3) Store and manifest loader

File-backed CAS with canonical CBOR ensures hashes are stable.

```rust
// aos-store/src/lib.rs
use std::{fs, path::{Path, PathBuf}};
use aos_cbor::{to_canonical_cbor, Hash as CHash};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

pub struct FsStore { root: PathBuf }

impl FsStore {
    pub fn open(root: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(root.join(".store/nodes"))?;
        fs::create_dir_all(root.join(".store/blobs"))?;
        Ok(FsStore { root })
    }
    pub fn put_node<T: Serialize>(&self, node: &T) -> anyhow::Result<CHash> {
        let bytes = to_canonical_cbor(node)?;
        let h = CHash::of_cbor(node);
        let p = self.root.join(".store/nodes").join(h.to_hex().replace(':', "_"));
        if !p.exists() { fs::write(p, bytes)?; }
        Ok(h)
    }
    pub fn get_node<T: DeserializeOwned>(&self, h: &CHash) -> anyhow::Result<T> {
        let p = self.root.join(".store/nodes").join(h.to_hex().replace(':', "_"));
        let bytes = fs::read(p)?;
        Ok(serde_cbor::from_slice(&bytes)?)
    }
    pub fn put_blob(&self, bytes: &[u8]) -> anyhow::Result<CHash> {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let out = hasher.finalize();
        let mut arr = [0u8;32];
        arr.copy_from_slice(&out);
        let h = CHash(arr);
        let p = self.root.join(".store/blobs").join(h.to_hex().replace(':', "_"));
        if !p.exists() { fs::write(p, bytes)?; }
        Ok(h)
    }
    pub fn get_blob(&self, h: &CHash) -> anyhow::Result<Vec<u8>> {
        let p = self.root.join(".store/blobs").join(h.to_hex().replace(':', "_"));
        Ok(fs::read(p)?)
    }
}
```

Manifest loader that resolves Name→Hash mappings and caches parsed nodes:

```rust
// aos-store/src/manifest.rs
use aos_air_types::{AirNode, Manifest, NamedRef};
use aos_cbor::Hash as CHash;

pub struct Catalog {
    pub manifest: Manifest,
    pub nodes: std::collections::HashMap<String, (CHash, AirNode)>, // name -> (hash,node)
}

impl super::FsStore {
    pub fn load_manifest(&self) -> anyhow::Result<Catalog> {
        let path = self.root.join("manifest.air.cbor");
        let bytes = std::fs::read(&path)?;
        let manifest: Manifest = serde_cbor::from_slice(&bytes)?;
        let mut nodes = std::collections::HashMap::new();

        let mut load_refs = |refs: &[NamedRef]| -> anyhow::Result<()> {
            for r in refs {
                let hash = parse_hash(&r.hash)?; // implement parse_hash("sha256:…") -> CHash
                let node: AirNode = self.get_node(&hash)?;
                nodes.insert(r.name.clone(), (hash, node));
            }
            Ok(())
        };

        load_refs(&manifest.schemas)?;
        load_refs(&manifest.modules)?;
        load_refs(&manifest.plans)?;
        load_refs(&manifest.caps)?;
        load_refs(&manifest.policies)?;
        Ok(Catalog { manifest, nodes })
    }
}
```

4) Semantic validator

Validate beyond JSON Schema: name resolution, DAG checks, type compatibility, capability/policy references.

```rust
// aos-air-validate/src/lib.rs
use aos_air_types::*;
use petgraph::{graphmap::DiGraphMap, Direction};

pub fn validate_plan_semantics(plan: &DefPlan) -> anyhow::Result<()> {
    // build graph and check acyclicity
    let mut g = DiGraphMap::<&str, ()>::new();
    for s in &plan.steps { g.add_node(&s.id()); }
    for e in &plan.edges { g.add_edge(e.from.as_str(), e.to.as_str(), ()); }
    if petgraph::algo::is_cyclic_directed(&g) {
        anyhow::bail!("plan {} has cycles", plan.name);
    }
    // check await_receipt refs earlier steps, etc.
    // … more checks …
    Ok(())
}
```

5) Expression evaluator

Side-effect-free, deterministic, operating on a small Value type. Use your Value model, not serde_cbor::Value, to keep control.

```rust
// aos-air-exec/src/expr_eval.rs
use aos_air_types::{expr::*, value::*};
use thiserror::Error;
use indexmap::IndexMap;

#[derive(Default)]
pub struct Env {
    pub plan_input: Value,
    pub vars: IndexMap<String, Value>,
    pub steps: IndexMap<String, Value>,
}

#[derive(Error, Debug)]
pub enum EvalError {
    #[error("missing ref {0}")] MissingRef(String),
    #[error("type error: {0}")] TypeError(&'static str),
    #[error("op error: {0}")] OpError(String),
}

pub fn eval(expr: &Expr, env: &Env) -> Result<Value, EvalError> {
    match expr {
        Expr::Ref { r#ref: path } => resolve_ref(path, env),
        Expr::Bool { bool: b } => Ok(Value::Bool(*b)),
        Expr::Int { int } => Ok(Value::Int(*int)),
        Expr::Nat { nat } => Ok(Value::Nat(*nat)),
        Expr::Dec128 { dec128 } => Ok(Value::Dec128(dec128.clone())),
        Expr::Text { text } => Ok(Value::Text(text.clone())),
        Expr::BytesB64 { bytes_b64 } => Ok(Value::Bytes(base64::decode(bytes_b64).unwrap_or_default())),
        Expr::TimeNs { time_ns } => Ok(Value::TimeNs(*time_ns)),
        Expr::DurationNs { duration_ns } => Ok(Value::DurationNs(*duration_ns)),
        Expr::Hash { hash } => Ok(Value::Hash(hash.clone())),
        Expr::Uuid { uuid } => Ok(Value::Uuid(uuid.clone())),
        Expr::Record { record } => {
            let mut out = IndexMap::new();
            for (k, v) in record {
                out.insert(k.clone(), eval(v, env)?);
            }
            Ok(Value::Record(out))
        }
        Expr::List { list } => Ok(Value::List(list.iter().map(|e| eval(e, env)).collect::<Result<Vec<_>, _>>()?)),
        Expr::Set { set } => {
            let mut s = std::collections::BTreeSet::new();
            for e in set { s.insert(to_key(&eval(e, env)?)?); }
            Ok(Value::Set(s))
        }
        Expr::Map { map } => {
            let mut m = std::collections::BTreeMap::new();
            for (k, v) in map {
                m.insert(to_key(&eval(k, env)?)?, eval(v, env)?);
            }
            Ok(Value::Map(m))
        }
        Expr::Variant { variant } => Ok(Value::Record(IndexMap::from_iter([
            ("$tag".into(), Value::Text(variant.tag.clone())),
            ("$value".into(), variant.value.as_ref().map(|v| eval(v, env)).transpose()?.unwrap_or(Value::Unit)),
        ]))),
        Expr::Op { op, args } => eval_op(op, args, env),
    }
}

fn resolve_ref(path: &str, env: &Env) -> Result<Value, EvalError> {
    if let Some(rest) = path.strip_prefix("@plan.input") {
        return get_path(&env.plan_input, rest);
    }
    if let Some(var) = path.strip_prefix("@var:") {
        return env.vars.get(var).cloned().ok_or_else(|| EvalError::MissingRef(path.to_string()));
    }
    if let Some(step) = path.strip_prefix("@step:") {
        let (id, rest) = step.split_once('.').unwrap_or((step, ""));
        let val = env.steps.get(id).cloned().ok_or_else(|| EvalError::MissingRef(path.to_string()))?;
        return get_path(&val, if rest.is_empty() { "" } else { &format!(".{rest}") });
    }
    Err(EvalError::MissingRef(path.to_string()))
}

fn get_path(root: &Value, path: &str) -> Result<Value, EvalError> {
    if path.is_empty() { return Ok(root.clone()); }
    let mut cur = root;
    for seg in path.trim_start_matches('.').split('.') {
        match cur {
            Value::Record(map) => { cur = map.get(seg).ok_or_else(|| EvalError::MissingRef(seg.into()))?; }
            _ => return Err(EvalError::TypeError("path on non-record")),
        }
    }
    Ok(cur.clone())
}

fn to_key(v: &Value) -> Result<ValueKey, EvalError> {
    match v {
        Value::Int(i) => Ok(ValueKey::Int(*i)),
        Value::Nat(n) => Ok(ValueKey::Nat(*n)),
        Value::Text(s) => Ok(ValueKey::Text(s.clone())),
        Value::Uuid(s) => Ok(ValueKey::Uuid(s.clone())),
        Value::Hash(s) => Ok(ValueKey::Hash(s.clone())),
        _ => Err(EvalError::TypeError("invalid map/set key type")),
    }
}

fn eval_op(op: &Op, args: &[Expr], env: &Env) -> Result<Value, EvalError> {
    use Op::*;
    let vals: Vec<Value> = args.iter().map(|e| eval(e, env)).collect::<Result<_, _>>()?;
    match op {
        Len => Ok(Value::Nat(match &vals[0] {
            Value::List(v) => v.len() as u64,
            Value::Record(m) => m.len() as u64,
            _ => return Err(EvalError::TypeError("len: list or record")),
        })),
        Eq => Ok(Value::Bool(vals.len() == 2 && equal(&vals[0], &vals[1]))),
        And => Ok(Value::Bool(as_bool(&vals[0])? && as_bool(&vals[1])?)),
        Or  => Ok(Value::Bool(as_bool(&vals[0])? || as_bool(&vals[1])?)),
        Not => Ok(Value::Bool(!as_bool(&vals[0])?)),
        Concat => Ok(Value::Text(format!("{}{}", as_text(&vals[0])?, as_text(&vals[1])?))),
        Add => match (&vals[0], &vals[1]) {
            (Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
            (Value::Nat(a), Value::Nat(b)) => Ok(Value::Nat(a + b)),
            _ => Err(EvalError::TypeError("add: int|nat")),
        },
        _ => Err(EvalError::OpError(format!("op {:?} not yet implemented", op))),
    }
}

fn as_bool(v: &Value) -> Result<bool, EvalError> { if let Value::Bool(b)=v {Ok(*b)} else {Err(EvalError::TypeError("bool expected"))}}
fn as_text(v: &Value) -> Result<&str, EvalError> { if let Value::Text(s)=v {Ok(s)} else {Err(EvalError::TypeError("text expected"))}}
fn equal(a:&Value,b:&Value)->bool{ std::mem::discriminant(a)==std::mem::discriminant(b) && to_cbor(a)==to_cbor(b) }
fn to_cbor(v:&Value)->Vec<u8>{ serde_cbor::to_vec(v).unwrap_or_default() }
```

6) Deterministic WASM runner

Single exported function per module, no WASI, no threads. Call step/run with CBOR payloads.

```rust
// aos-wasm/src/lib.rs
use wasmtime::{Config, Engine, Module, Store, Instance, Linker, TypedFunc};
use anyhow::Context;

pub struct WasmRuntime { engine: Engine }

impl WasmRuntime {
    pub fn new() -> anyhow::Result<Self> {
        let mut cfg = Config::new();
        cfg.wasm_multi_value(true);
        cfg.wasm_threads(false);
        cfg.consume_fuel(false);
        cfg.debug_info(false);
        let engine = Engine::new(&cfg)?;
        Ok(Self { engine })
    }

    pub fn call_reducer_step(&self, wasm_bytes: &[u8], input_cbor: &[u8]) -> anyhow::Result<Vec<u8>> {
        let module = Module::new(&self.engine, wasm_bytes)?;
        let mut store = Store::new(&self.engine, ());
        let mut linker = Linker::new(&self.engine);
        let instance = linker.instantiate(&mut store, &module)?;
        let func = instance.get_typed_func::<(i32,i32), (i32,i32), _>(&mut store, "step")
            .context("export `step` not found")?;
        let memory = instance.get_memory(&mut store, "memory").context("no memory")?;
        let ptr = 0x10000; // naive; replace with bump allocator in prod
        memory.write(&mut store, ptr as usize, input_cbor)?;
        let len = input_cbor.len() as i32;
        let (out_ptr, out_len) = func.call(&mut store, (ptr, len))?;
        let mut out = vec![0u8; out_len as usize];
        memory.read(&mut store, out_ptr as usize, &mut out)?;
        Ok(out)
    }

}
```

Reducer call envelope (v1 and v1.1)

Use a single reducer export `step` with a canonical CBOR envelope that carries optional key and a mode flag. This lets the same reducer binary run in v1 (monolithic state) and v1.1 (cells).

```rust
#[derive(serde::Deserialize)]
struct InEnvelope {
    version: u8, // =1
    #[serde(with="serde_bytes")]
    state: Option<Vec<u8>>,                  // None on first call (cell mode)
    event: EventEnvelope,
    ctx: CallCtx,
}

#[derive(serde::Deserialize)]
struct EventEnvelope { schema: String, #[serde(with="serde_bytes")] value: Vec<u8> }

#[derive(serde::Deserialize)]
struct CallCtx { #[serde(with="serde_bytes")] key: Option<Vec<u8>>, cell_mode: bool }

#[derive(serde::Serialize)]
struct OutEnvelope {
    #[serde(with="serde_bytes")] state: Option<Vec<u8>>, // None => delete cell (cell mode)
    #[serde(default)] domain_events: Vec<EventEnvelope>,
    #[serde(default)] effects: Vec<EffectIntent>,
    #[serde(default, with="serde_bytes")] ann: Option<Vec<u8>>,
}
```

Kernel constructs this envelope from reducer state and routing metadata (including manifest.routing.events[].key_field and cell_mode flag). See spec/05-cells.md for details.

7) Effects, capabilities, policies

Define effect intents, receipts, the gate traits, and an adapter registry. The kernel enqueues intents; adapters execute out-of-band and append receipts.

```rust
// aos-effects/src/types.rs
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EffectIntent {
    pub kind: String,
    pub params_cbor: Vec<u8>,   // canonical CBOR of params
    pub cap_name: String,       // which grant to use
    pub idempotency_key: [u8; 32],
    pub intent_hash: [u8; 32],  // sha256(kind, params, cap, idemp)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub intent_hash: [u8; 32],
    pub adapter_id: String,
    pub status: ReceiptStatus,
    pub payload_cbor: Vec<u8>,
    pub cost_cents: Option<u64>,
    pub sig: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus { Ok, Error }
```

Policy and capability gates:

```rust
// aos-kernel/src/gates.rs
use aos_effects::types::EffectIntent;

pub trait CapabilityGate {
    fn resolve(&self, cap_name: &str, effect_kind: &str) -> anyhow::Result<ResolvedCap>;
}

pub trait PolicyGate {
    fn decide(&self, intent: &EffectIntent, cap: &ResolvedCap, source: EffectSource) -> Decision;
}

pub enum Decision { Allow, Deny, RequireApproval }

pub enum EffectSource { Reducer(String), Plan(String) }

pub struct ResolvedCap {
    pub cap_type: String, // e.g., "http.out"
    pub params_cbor: Vec<u8>,
    pub budget: Budget,
}

pub struct Budget { pub tokens: Option<u64>, pub bytes: Option<u64>, pub cents: Option<u64> }
```

Reducer effect guardrails (v1)

Enforce the architectural boundary between reducers and plans:

```rust
// aos-kernel/src/reducer_guards.rs
const MICRO_EFFECTS: &[&str] = &["fs.blob.put", "fs.blob.get", "timer.set"];

pub fn validate_reducer_effects(
    module_name: &str,
    effects: &[ReducerEffect],
    declared_effects: &[String]
) -> anyhow::Result<()> {
    // Rule 1: At most one effect per step
    if effects.len() > 1 {
        anyhow::bail!(
            "Reducer {} emitted {} effects; reducers may emit at most 1 per step. \
             Lift complex orchestration to a plan.",
            module_name, effects.len()
        );
    }

    // Rule 2: Only micro-effects allowed
    for eff in effects {
        if !MICRO_EFFECTS.contains(&eff.kind.as_str()) {
            anyhow::bail!(
                "Reducer {} attempted to emit effect '{}'. \
                 Reducers may only emit micro-effects (fs.blob.put, fs.blob.get, timer.set). \
                 Network effects (http, llm, email, payment) must go through plans.",
                module_name, eff.kind
            );
        }

        // Rule 3: Must be in declared effects_emitted
        if !declared_effects.contains(&eff.kind) {
            anyhow::bail!(
                "Reducer {} emitted effect '{}' not in declared effects_emitted",
                module_name, eff.kind
            );
        }
    }

    Ok(())
}
```

Policy gate should also check effect source:

```rust
impl PolicyGate for DefaultPolicy {
    fn decide(&self, intent: &EffectIntent, cap: &ResolvedCap, source: EffectSource) -> Decision {
        // If effect came from a reducer and is not a micro-effect, deny
        if matches!(source, EffectSource::Reducer(_)) && !is_micro_effect(&intent.kind) {
            return Decision::Deny;
        }
        // ... rest of policy logic
    }
}

pub enum EffectSource { Reducer(String), Plan(String) }

fn is_micro_effect(kind: &str) -> bool {
    matches!(kind, "fs.blob.put" | "fs.blob.get" | "timer.set")
}
```

This ensures the intent→plan→effect flow is respected and reducers stay focused on domain logic.

8) Plan executor

Single-threaded DAG scheduler. Produces outbox intents, consumes receipts, raises events, emits effects, awaits receipts/events, binds variables, checks invariants.

```rust
// aos-kernel/src/plan_exec.rs
use aos_air_types::{DefPlan, expr::Expr};
use aos_air_exec::expr_eval::{Env, eval};
use aos_cbor::to_canonical_cbor;
use aos_wasm::WasmRuntime;
use aos_effects::types::{EffectIntent, Receipt};
use indexmap::IndexMap;
use thiserror::Error;

#[derive(Debug, Clone)]
pub enum StepState { Pending, Ready, WaitingReceipt { effect_id: [u8;32] }, Done, Failed(String) }

pub struct PlanInstance {
    pub name: String,
    pub env: Env,
    pub steps: IndexMap<String, StepRuntime>,
    pub edges: Vec<(String, String, Option<Expr>)>,
    pub output: Option<Expr>,
    pub completed: bool,
}

pub struct StepRuntime { pub spec: StepSpec, pub state: StepState }

#[derive(Clone)]
pub enum StepSpec {
    RaiseEvent { reducer: String, key: Option<Expr>, event: Expr },
    EmitEffect { kind: String, params: Expr, cap: String, bind: String },
    AwaitReceipt { effect_ref: Expr, bind: String },
    AwaitEvent { event_schema: String, predicate: Option<Expr>, bind: String },
    Assign { expr: Expr, bind: String },
    End { result: Option<Expr> },
}

#[derive(Error, Debug)]
pub enum ExecError { #[error("validation: {0}")] Validation(String), #[error("runtime: {0}")] Runtime(String) }

impl PlanInstance {
    pub fn tick(
        &mut self,
        rt: &WasmRuntime,
        world: &mut WorldCtx
    ) -> Result<(), ExecError> {
        if self.completed { return Ok(()); }

        let ready_ids = compute_ready(&self.steps, &self.edges, &self.env)?;
        if ready_ids.is_empty() { return Ok(()); }
        let id = &ready_ids[0];
        let step = self.steps.get_mut(id).unwrap();

        match &step.spec {
            StepSpec::RaiseEvent { reducer, key, event } => {
                let val = eval(event, &self.env).map_err(|e| ExecError::Runtime(format!("{e:?}")))?;
                let key_val = match key {
                    Some(kexpr) => Some(eval(kexpr, &self.env).map_err(|e| ExecError::Runtime(format!("{e:?}")))?),
                    None => None,
                };
                world.raise_event(reducer, key_val, val)?;
                step.state = StepState::Done;
            }
            StepSpec::EmitEffect { kind, params, cap, bind } => {
                let params_val = eval(params, &self.env).map_err(|e| ExecError::Runtime(format!("{e:?}")))?;
                let intent = world.make_intent(kind.clone(), params_val, cap)?;
                world.enqueue_intent(&intent, EffectSource::Plan(self.name.clone()))?;
                self.env.vars.insert(bind.clone(), aos_air_types::value::Value::Hash(format!("sha256:{}", hex::encode(intent.intent_hash))));
                step.state = StepState::Done;
            }
            StepSpec::AwaitReceipt { effect_ref, bind } => {
                let idv = eval(effect_ref, &self.env).map_err(|e| ExecError::Runtime(format!("{e:?}")))?;
                let effect_id = effect_id_from_value(&idv)?;
                if let Some(rec) = world.try_get_receipt(&effect_id) {
                    let val: aos_air_types::value::Value = serde_cbor::from_slice(&rec.payload_cbor).unwrap();
                    self.env.vars.insert(bind.clone(), val);
                    step.state = StepState::Done;
                } else {
                    step.state = StepState::WaitingReceipt { effect_id };
                }
            }
            StepSpec::AwaitEvent { event_schema, predicate, bind } => {
                if let Some(ev) = world.try_get_event(event_schema, predicate.as_ref(), &self.env)? {
                    self.env.vars.insert(bind.clone(), ev);
                    step.state = StepState::Done;
                } else {
                    // remain pending
                }
            }
            StepSpec::Assign { expr, bind } => {
                let v = eval(expr, &self.env).map_err(|e| ExecError::Runtime(format!("{e:?}")))?;
                self.env.vars.insert(bind.clone(), v);
                step.state = StepState::Done;
            }
            StepSpec::End { result } => {
                if let Some(r) = result {
                    let v = eval(r, &self.env).map_err(|e| ExecError::Runtime(format!("{e:?}")))?;
                    world.complete_plan(Some(v));
                } else {
                    world.complete_plan(None);
                }
                self.completed = true;
                step.state = StepState::Done;
            }
        }

        Ok(())
    }
}

fn compute_ready(
    steps: &IndexMap<String, StepRuntime>,
    edges: &Vec<(String,String,Option<Expr>)>,
    env: &Env
) -> Result<Vec<String>, ExecError> {
    use StepState::*;
    let mut preds = std::collections::HashMap::<&str, Vec<&(String,String,Option<Expr>)>>::new();
    for e in edges { preds.entry(&e.1).or_default().push(e); }
    let mut ready = vec![];
    for (id, st) in steps.iter() {
        if !matches!(st.state, Pending) { continue; }
        let all_pred_done = preds.get(id.as_str()).map(|vv| {
            vv.iter().all(|(from,_,guard)| {
                let done = matches!(steps.get(from.as_str()).map(|s| &s.state), Some(Done));
                let pass = guard.as_ref().map(|g| matches!(eval(g, env), Ok(aos_air_types::value::Value::Bool(true)))).unwrap_or(true);
                done && pass
            })
        }).unwrap_or(true);
        if all_pred_done { ready.push(id.clone()); }
    }
    ready.sort();
    Ok(ready)
}

#[derive(serde::Deserialize)]
struct ReducerOutput {
    state: Vec<u8>,
    #[serde(default)]
    effects: Vec<ReducerEffect>,
    #[serde(default)]
    ann: Option<aos_air_types::value::Value>,
}
#[derive(serde::Deserialize)]
struct ReducerEffect {
    kind: String,
    params: aos_air_types::value::Value,
    #[serde(default)]
    cap_slot: Option<String>,
}
```

World context passes capability/policy gates and queues intents/receipts:

```rust
// aos-kernel/src/world.rs
use aos_effects::types::{EffectIntent, Receipt};
use aos_cbor::{to_canonical_cbor, Hash as CHash};
use rand::{RngCore, SeedableRng};

pub struct WorldCtx<'a> {
    pub caps: &'a dyn CapabilityGate,
    pub policy: &'a dyn PolicyGate,
    pub outbox: Vec<EffectIntent>,
    pub receipts: std::collections::HashMap<[u8;32], Receipt>,
    pub events: Vec<aos_air_types::value::Value>, // recent domain events for await_event
    pub modules: ModuleRepo<'a>,
    pub reducer_states: std::collections::HashMap<String, Vec<u8>>,
    pub rng: rand_chacha::ChaCha20Rng,
}

impl<'a> WorldCtx<'a> {
    pub fn reducer_state(&self, module: &str) -> Vec<u8> { self.reducer_states.get(module).cloned().unwrap_or_default() }
    pub fn set_reducer_state(&mut self, module: &str, state: Vec<u8>) { self.reducer_states.insert(module.to_string(), state); }
    pub fn resolve_slot(&self, module: &str, slot: Option<&str>, overrides: &indexmap::IndexMap<String,String>) -> anyhow::Result<String> {
        if let Some(s) = slot {
            if let Some(g) = overrides.get(s) { return Ok(g.clone()); }
            if let Some(g) = self.modules.binding(module, s) { return Ok(g.to_string()); }
            anyhow::bail!("unbound cap slot {} for module {}", s, module);
        }
        anyhow::bail!("missing cap slot");
    }
    pub fn make_intent(&mut self, kind: String, params: aos_air_types::value::Value, cap: &str) -> anyhow::Result<EffectIntent> {
        let params_cbor = to_canonical_cbor(&params)?;
        let mut idk = [0u8; 32]; self.rng.fill_bytes(&mut idk);
        #[derive(serde::Serialize)]
        struct IntentHash<'a> { kind:&'a str, #[serde(with="serde_bytes")] params:&'a [u8], cap:&'a str, idk:&'a [u8;32] }
        let ih = aos_cbor::Hash::of_cbor(&IntentHash{ kind: &kind, params: &params_cbor, cap, idk: &idk });
        Ok(EffectIntent{ kind, params_cbor, cap_name: cap.to_string(), idempotency_key: idk, intent_hash: ih.0 })
    }
    pub fn enqueue_intent(&mut self, intent: &EffectIntent, source: EffectSource) -> anyhow::Result<()> {
        let cap = self.caps.resolve(&intent.cap_name, &intent.kind)?;
        match self.policy.decide(intent, &cap, source) {
            Decision::Allow => { self.outbox.push(intent.clone()) }
            Decision::Deny => anyhow::bail!("policy denied"),
            Decision::RequireApproval => anyhow::bail!("approval required (v1)"),
        }
        Ok(())
    }
    pub fn raise_event(&mut self, reducer: &str, key: Option<aos_air_types::value::Value>, event: aos_air_types::value::Value) -> anyhow::Result<()> {
        // Append DomainEvent to journal targeting `reducer` (and keyed cell if provided); simplified stub here
        // In a full implementation, use routing.events to find key_field and attach key to the entry
        let _ = (reducer, key); // suppress unused warnings in the stub
        self.events.push(event);
        Ok(())
    }
    pub fn try_get_event(&mut self, schema: &str, pred: Option<&Expr>, env: &Env) -> anyhow::Result<Option<aos_air_types::value::Value>> {
        // Very simplified: match by a $schema field in the value record and optional predicate
        if let Some(pos) = self.events.iter().position(|v| matches_schema(v, schema) && pred.map(|p| matches_pred(v, p, env)).unwrap_or(true)) {
            return Ok(Some(self.events.remove(pos)));
        }
        Ok(None)
    }
    pub fn try_get_receipt(&self, effect_id: &[u8;32]) -> Option<&Receipt> { self.receipts.get(effect_id) }
    pub fn complete_plan(&mut self, _result: Option<aos_air_types::value::Value>) { /* write journal … */ }
}

pub struct ModuleRepo<'a> {
    pub load: Box<dyn Fn(&str) -> anyhow::Result<Vec<u8>> + 'a>,
    pub bindings: std::collections::HashMap<(String,String), String>, // (module, slot)->cap
}
impl<'a> ModuleRepo<'a> {
    pub fn load_wasm(&self, name: &str) -> anyhow::Result<Vec<u8>> { (self.load)(name) }
    pub fn binding(&self, module: &str, slot: &str) -> Option<&str> { self.bindings.get(&(module.to_string(), slot.to_string())).map(|s| s.as_str()) }
}
```

9) Journal and snapshots

Persist PlanStarted/EffectQueued/PolicyDecision/ReceiptAppended/PlanEnded etc. All entries encoded with canonical CBOR; snapshots store reducer states + control-plane state. Keep the world stepper strictly single-threaded: apply one journal entry at a time.

```rust
// aos-kernel/src/journal.rs
#[derive(serde::Serialize, serde::Deserialize)]
pub enum JournalEntry {
    PlanStarted { plan: String, instance: String, input_ref: aos_cbor::Hash },
    EffectQueued { instance: String, intent_hash: [u8;32] },
    PolicyDecision { intent_hash: [u8;32], decision: String },
    ReceiptAppended { intent_hash: [u8;32], receipt_ref: aos_cbor::Hash },
    PlanEnded { instance: String, status: String },
    SnapshotMade { height: u64, snapshot_ref: aos_cbor::Hash },
}
```

10) Shadow-run

Clone the manifest and plan inputs; evaluate steps until the first emit_effect/await_receipt; collect predicted intents and state diffs; do not execute any effects.

```rust
pub fn shadow_run(plan: &DefPlan, input: Value, world: &WorldCtx) -> ShadowReport {
    // Execute tick loop but intercept EmitEffect and do not enqueue;
    // Instead, record predicted intents (kind + params + cap) with hashes
    // Return a report with step order, guards, predicted effects, and schema diffs
    unimplemented!()
}
```

11) Unit tests and replay

- “Replay-or-die”: after each integration test, serialize snapshot bytes, then replay journal + receipts from genesis and assert byte-identical snapshot/state hashes.
- Fuzz the Expr evaluator with arbitrary JSON to catch panics and type errors.
- Golden tests for Wasm reducers: feed inputs, compare outputs.

12) Minimal reducer SDK (Rust)

Help reducer authors build CBOR IO and reducer outputs.

```rust
// aos-wasm-sdk/src/lib.rs (to be compiled to wasm32-unknown-unknown for reducers)
use serde::{Serialize, Deserialize};
#[derive(Serialize, Deserialize)]
pub struct StepInput<S,E> { pub state: S, pub event: E }
#[derive(Serialize, Deserialize)]
pub struct StepOutput<S,A> {
    pub state: S,
    #[serde(default)]
    pub effects: Vec<EffectIntent<A>>,
    #[serde(default)]
    pub ann: Option<serde_cbor::Value>,
}
#[derive(Serialize, Deserialize)]
pub struct EffectIntent<A> { pub kind: String, pub params: A, pub cap_slot: Option<String> }

#[no_mangle]
pub extern "C" fn step(ptr: i32, len: i32) -> (i32, i32) {
    // read input from linear memory; call user_step(); write output; return pointer/len
    // Provide a tiny bump allocator or rely on wee_alloc; omitted for brevity
    unimplemented!()
}
```

Putting it together: execution loop

- World loop:
  - Read next journal command (start plan, append receipt, approve patch, etc.)
  - If PlanStarted: instantiate PlanInstance with env input, set steps Pending.
  - For active plan instances, call tick() until no Ready steps remain (or after 1 step per loop if you prefer strict pacing).
  - EffectQueued: write to adapter queue (out of core).
  - ReceiptAppended: store in receipts map; awaken any WaitingReceipt steps whose effect_id matches.

Example: run one step per tick deterministically

```rust
// aos-kernel/src/stepper.rs
pub fn step_world(world: &mut World) -> anyhow::Result<()> {
    // 1) apply any new receipts from adapter inbox -> journal ReceiptAppended
    world.drain_adapter_inbox()?;

    // 2) pick next plan instance with READY step (lex order)
    if let Some((pi_id, step_id)) = world.next_ready_step() {
        world.exec_step(&pi_id, &step_id)?;
        world.journal(JournalEntry::StepExecuted { pi_id, step_id });
    } else {
        // idle; optionally sleep/poll timers
    }
    Ok(())
}
```

Advice on libraries and details

- decimal128: if you truly need IEEE 754 decimal128 now, use bson::Decimal128 for internal representation and convert at the AIR boundary; otherwise, keep decimals as strings in v1 and add proper decimal later.
- canonical CBOR: serde_cbor::ser::Serializer::canonical() is sufficient for v1; if you later need DAG-CBOR, switch to libipld and its dag-cbor codec (but you’ll have to drop tags).
- Wasmtime determinism: keep to wasm32-unknown-unknown, avoid WASI, threads, random, time; compile with opt-level=z or s for size; pin Wasmtime version in Cargo.lock.
- Policy and capability: keep gates simple now (glob host/path, verbs, budgets). Add OPA/CEL later behind the PolicyGate trait.

Final conclusions

- Implement AIR evaluation in Rust as a small control-plane engine: canonicalize-and-hash all AIR nodes; load and semantically validate manifests; evaluate expressions with a total, deterministic interpreter; schedule plan steps through a single-threaded executor; call deterministic WASM reducers via a tiny CBOR ABI; create effect intents and gate them by capability and policy; reconcile via signed receipts; and persist everything to a canonical journal and snapshots.
- The code skeletons above give you the key types, traits, and function boundaries. Start with CBOR+hashing+store, then loader/validator, then expr eval, then Wasm runner, then the plan executor and gates, then effects/receipts and journal, then shadow-run. Keep tests “replay-or-die.” This will get you to an end-to-end deterministic PoC without building a full DSL.
