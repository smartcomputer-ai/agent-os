use std::{collections::HashMap, path::Path};

use aos_air_types::{validate, AirNode, Manifest, NamedRef};
use aos_cbor::Hash;

use crate::{io_error, Store, StoreError, StoreResult};

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub hash: Hash,
    pub node: AirNode,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub manifest: Manifest,
    pub nodes: HashMap<String, CatalogEntry>,
}

#[derive(Clone, Copy)]
enum NodeKind {
    Schema,
    Module,
    Plan,
    Cap,
    Policy,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            NodeKind::Schema => "defschema",
            NodeKind::Module => "defmodule",
            NodeKind::Plan => "defplan",
            NodeKind::Cap => "defcap",
            NodeKind::Policy => "defpolicy",
        }
    }

    fn matches(self, node: &AirNode) -> bool {
        matches!(
            (self, node),
            (NodeKind::Schema, AirNode::Defschema(_))
                | (NodeKind::Module, AirNode::Defmodule(_))
                | (NodeKind::Plan, AirNode::Defplan(_))
                | (NodeKind::Cap, AirNode::Defcap(_))
                | (NodeKind::Policy, AirNode::Defpolicy(_))
        )
    }
}

pub fn load_manifest_from_path<S: Store>(store: &S, path: impl AsRef<Path>) -> StoreResult<Catalog> {
    let path_ref = path.as_ref();
    let bytes = std::fs::read(path_ref).map_err(|e| io_error(path_ref, e))?;
    load_manifest_from_bytes(store, &bytes)
}

pub fn load_manifest_from_bytes<S: Store>(store: &S, bytes: &[u8]) -> StoreResult<Catalog> {
    let manifest: Manifest = serde_cbor::from_slice(bytes)?;
    let mut nodes = HashMap::new();

    load_refs(store, &manifest.schemas, NodeKind::Schema, &mut nodes)?;
    load_refs(store, &manifest.modules, NodeKind::Module, &mut nodes)?;
    load_refs(store, &manifest.plans, NodeKind::Plan, &mut nodes)?;
    load_refs(store, &manifest.caps, NodeKind::Cap, &mut nodes)?;
    load_refs(store, &manifest.policies, NodeKind::Policy, &mut nodes)?;

    validate_plans(&manifest, &nodes)?;

    Ok(Catalog { manifest, nodes })
}

fn load_refs<S: Store>(
    store: &S,
    refs: &[NamedRef],
    kind: NodeKind,
    nodes: &mut HashMap<String, CatalogEntry>,
) -> StoreResult<()> {
    for reference in refs {
        let hash = parse_hash_str(reference.hash.as_str())?;
        let node: AirNode = store.get_node(hash)?;
        if !kind.matches(&node) {
            return Err(StoreError::NodeKindMismatch {
                name: reference.name.clone(),
                expected: kind.label(),
            });
        }
        nodes.insert(
            reference.name.clone(),
            CatalogEntry {
                hash,
                node,
            },
        );
    }
    Ok(())
}

fn parse_hash_str(value: &str) -> StoreResult<Hash> {
    Hash::from_hex_str(value).map_err(|source| StoreError::InvalidHashString {
        value: value.to_string(),
        source,
    })
}

fn validate_plans(manifest: &Manifest, nodes: &HashMap<String, CatalogEntry>) -> StoreResult<()> {
    for plan_ref in &manifest.plans {
        if let Some(entry) = nodes.get(&plan_ref.name) {
            if let AirNode::Defplan(plan) = &entry.node {
                validate::validate_plan(plan).map_err(|source| StoreError::PlanValidation {
                    name: plan.name.clone(),
                    source,
                })?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemStore;
    use aos_air_types::{
        AirNode, DefPlan, EffectKind, EmptyObject, Expr, ExprConst, ExprOp, ExprOpCode, ExprRef,
        HashRef, NamedRef, PlanBind, PlanBindEffect, PlanEdge, PlanStep, PlanStepAssign,
        PlanStepAwaitReceipt, PlanStepEmitEffect, PlanStepEnd, PlanStepKind, SchemaRef, TypeExpr,
        TypePrimitive, TypePrimitiveText,
    };
    use indexmap::IndexMap;

    fn sample_plan() -> DefPlan {
        let expr = Expr::Const(ExprConst::Text { text: "payload".into() });
        DefPlan {
            name: "com.acme/plan@1".into(),
            input: SchemaRef::new("com.acme/Input@1").unwrap(),
            output: None,
            locals: IndexMap::new(),
            steps: vec![
                PlanStep {
                    id: "emit".into(),
                    kind: PlanStepKind::EmitEffect(PlanStepEmitEffect {
                        kind: EffectKind::HttpRequest,
                        params: expr.clone(),
                        cap: "http_cap".into(),
                        bind: PlanBindEffect { effect_id_as: "req".into() },
                    }),
                },
                PlanStep {
                    id: "await".into(),
                    kind: PlanStepKind::AwaitReceipt(PlanStepAwaitReceipt {
                        for_expr: Expr::Ref(ExprRef {
                            reference: "@var:req".into(),
                        }),
                        bind: PlanBind { var: "rcpt".into() },
                    }),
                },
                PlanStep {
                    id: "assign".into(),
                    kind: PlanStepKind::Assign(PlanStepAssign {
                        expr: Expr::Op(ExprOp {
                            op: ExprOpCode::Concat,
                            args: vec![expr.clone(), Expr::Const(ExprConst::Text { text: "!".into() })],
                        }),
                        bind: PlanBind { var: "result".into() },
                    }),
                },
                PlanStep {
                    id: "end".into(),
                    kind: PlanStepKind::End(PlanStepEnd { result: None }),
                },
            ],
            edges: vec![
                PlanEdge { from: "emit".into(), to: "await".into(), when: None },
                PlanEdge { from: "await".into(), to: "assign".into(), when: None },
                PlanEdge { from: "assign".into(), to: "end".into(), when: None },
            ],
            required_caps: vec!["http_cap".into()],
            allowed_effects: vec![EffectKind::HttpRequest],
            invariants: vec![],
        }
    }

    fn manifest_with_plan(plan_ref: NamedRef) -> Manifest {
        Manifest {
            schemas: vec![],
            modules: vec![],
            plans: vec![plan_ref],
            caps: vec![],
            policies: vec![],
            defaults: None,
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        }
    }

    #[test]
    fn load_manifest_success() {
        let store = MemStore::default();
        let plan = sample_plan();
        let plan_hash = store
            .put_node(&AirNode::Defplan(plan.clone()))
            .expect("store plan");
        let manifest = manifest_with_plan(NamedRef {
            name: plan.name.clone(),
            hash: HashRef::new(plan_hash.to_hex()).unwrap(),
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let catalog = load_manifest_from_bytes(&store, &manifest_bytes).expect("load");
        assert!(catalog.nodes.contains_key(&plan.name));
    }

    #[test]
    fn detects_node_kind_mismatch() {
        let store = MemStore::default();
        let schema_node = AirNode::Defschema(aos_air_types::DefSchema {
            name: "com.acme/Schema@1".into(),
            ty: TypeExpr::Primitive(TypePrimitive::Text(TypePrimitiveText {
                text: EmptyObject::default(),
            })),
        });
        let hash = store.put_node(&schema_node).expect("store schema");
        let manifest = manifest_with_plan(NamedRef {
            name: "com.acme/plan@1".into(),
            hash: HashRef::new(hash.to_hex()).unwrap(),
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::NodeKindMismatch { .. }));
    }

    #[test]
    fn plan_validation_failure_propagates() {
        let store = MemStore::default();
        let mut plan = sample_plan();
        plan.steps.push(plan.steps[0].clone()); // duplicate id triggers validation error
        let hash = store
            .put_node(&AirNode::Defplan(plan.clone()))
            .expect("store plan");
        let manifest = manifest_with_plan(NamedRef {
            name: plan.name.clone(),
            hash: HashRef::new(hash.to_hex()).unwrap(),
        });
        let manifest_bytes = serde_cbor::to_vec(&AirNode::Manifest(manifest)).unwrap();
        let err = load_manifest_from_bytes(&store, &manifest_bytes).unwrap_err();
        assert!(matches!(err, StoreError::PlanValidation { .. }));
    }
}
