use std::collections::{BTreeMap, HashMap};

use aos_cbor::Hash;
use once_cell::sync::Lazy;
use serde_json;

use crate::{AirNode, DefEffect, DefModule, DefSchema, DefWorkflow, HashRef};

static BUILTIN_SCHEMAS_RAW: &str = include_str!("../../../spec/defs/builtin-schemas.air.json");
static BUILTIN_SCHEMAS_SDK_RAW: &str =
    include_str!("../../../spec/defs/builtin-schemas-sdk.air.json");
static BUILTIN_SCHEMAS_HOST_RAW: &str =
    include_str!("../../../spec/defs/builtin-schemas-host.air.json");
static BUILTIN_MODULES_RAW: &str = include_str!("../../../spec/defs/builtin-modules.air.json");
static BUILTIN_WORKFLOW_EFFECTS_RAW: &str = include_str!("../../../spec/defs/builtin-ops.air.json");

#[derive(Debug)]
pub struct BuiltinSchema {
    pub schema: DefSchema,
    pub hash: Hash,
    pub hash_ref: HashRef,
}

#[derive(Debug, Clone)]
pub struct BuiltinWorkflow {
    pub workflow: DefWorkflow,
    pub hash: Hash,
    pub hash_ref: HashRef,
}

#[derive(Debug, Clone)]
pub struct BuiltinEffect {
    pub effect: DefEffect,
    pub hash: Hash,
    pub hash_ref: HashRef,
}

#[derive(Debug, Clone)]
pub struct BuiltinModule {
    pub module: DefModule,
    pub hash: Hash,
    pub hash_ref: HashRef,
}

static BUILTIN_SCHEMAS: Lazy<Vec<BuiltinSchema>> = Lazy::new(|| {
    let defs: Vec<DefSchema> = serde_json::from_str(BUILTIN_SCHEMAS_RAW)
        .expect("spec/defs/builtin-schemas.air.json must parse");
    let sdk_defs: Vec<DefSchema> = serde_json::from_str(BUILTIN_SCHEMAS_SDK_RAW)
        .expect("spec/defs/builtin-schemas-sdk.air.json must parse");
    let host_defs: Vec<DefSchema> = serde_json::from_str(BUILTIN_SCHEMAS_HOST_RAW)
        .expect("spec/defs/builtin-schemas-host.air.json must parse");
    let mut merged: BTreeMap<String, DefSchema> = BTreeMap::new();
    for schema in defs
        .into_iter()
        .chain(sdk_defs.into_iter())
        .chain(host_defs.into_iter())
    {
        merged.insert(schema.name.clone(), schema);
    }
    merged
        .into_values()
        .map(|schema| {
            let hash = Hash::of_cbor(&schema).expect("canonical hash");
            let hash_ref = HashRef::new(hash.to_hex()).expect("valid hash");
            BuiltinSchema {
                schema,
                hash,
                hash_ref,
            }
        })
        .collect()
});

static BUILTIN_WORKFLOWS: Lazy<Vec<BuiltinWorkflow>> = Lazy::new(|| {
    let defs: Vec<AirNode> = serde_json::from_str(BUILTIN_WORKFLOW_EFFECTS_RAW)
        .expect("spec/defs/builtin-ops.air.json must parse");
    defs.into_iter()
        .filter_map(|node| match node {
            AirNode::Defworkflow(workflow) => Some(workflow),
            _ => None,
        })
        .map(|workflow| {
            let hash = Hash::of_cbor(&workflow).expect("canonical hash");
            let hash_ref = HashRef::new(hash.to_hex()).expect("valid hash");
            BuiltinWorkflow {
                workflow,
                hash,
                hash_ref,
            }
        })
        .collect()
});

static BUILTIN_EFFECTS: Lazy<Vec<BuiltinEffect>> = Lazy::new(|| {
    let defs: Vec<AirNode> = serde_json::from_str(BUILTIN_WORKFLOW_EFFECTS_RAW)
        .expect("spec/defs/builtin-ops.air.json must parse");
    defs.into_iter()
        .filter_map(|node| match node {
            AirNode::Defeffect(effect) => Some(effect),
            _ => None,
        })
        .map(|effect| {
            let hash = Hash::of_cbor(&effect).expect("canonical hash");
            let hash_ref = HashRef::new(hash.to_hex()).expect("valid hash");
            BuiltinEffect {
                effect,
                hash,
                hash_ref,
            }
        })
        .collect()
});

static BUILTIN_MODULES: Lazy<Vec<BuiltinModule>> = Lazy::new(|| {
    let defs: Vec<DefModule> = serde_json::from_str(BUILTIN_MODULES_RAW)
        .expect("spec/defs/builtin-modules.air.json must parse");
    defs.into_iter()
        .map(|module| {
            let hash = Hash::of_cbor(&module).expect("canonical hash");
            let hash_ref = HashRef::new(hash.to_hex()).expect("valid hash");
            BuiltinModule {
                module,
                hash,
                hash_ref,
            }
        })
        .collect()
});

static BUILTIN_SCHEMA_INDEX: Lazy<HashMap<String, usize>> = Lazy::new(|| {
    BUILTIN_SCHEMAS
        .iter()
        .enumerate()
        .map(|(idx, schema)| (schema.schema.name.clone(), idx))
        .collect()
});

static BUILTIN_WORKFLOW_INDEX: Lazy<HashMap<String, usize>> = Lazy::new(|| {
    BUILTIN_WORKFLOWS
        .iter()
        .enumerate()
        .map(|(idx, workflow)| (workflow.workflow.name.clone(), idx))
        .collect()
});

static BUILTIN_EFFECT_INDEX: Lazy<HashMap<String, usize>> = Lazy::new(|| {
    BUILTIN_EFFECTS
        .iter()
        .enumerate()
        .map(|(idx, effect)| (effect.effect.name.clone(), idx))
        .collect()
});

static BUILTIN_MODULE_INDEX: Lazy<HashMap<String, usize>> = Lazy::new(|| {
    BUILTIN_MODULES
        .iter()
        .enumerate()
        .map(|(idx, module)| (module.module.name.clone(), idx))
        .collect()
});

/// Returns the parsed list of built-in `defschema` nodes (timer/blob params, receipts, and events).
pub fn builtin_schemas() -> &'static [BuiltinSchema] {
    &BUILTIN_SCHEMAS
}

/// Returns the parsed list of built-in `defworkflow` nodes.
pub fn builtin_workflows() -> &'static [BuiltinWorkflow] {
    &BUILTIN_WORKFLOWS
}

/// Returns the parsed list of built-in `defeffect` nodes.
pub fn builtin_effects() -> &'static [BuiltinEffect] {
    &BUILTIN_EFFECTS
}

/// Returns the parsed list of built-in `defmodule` nodes.
pub fn builtin_modules() -> &'static [BuiltinModule] {
    &BUILTIN_MODULES
}

/// Finds a built-in schema definition by name (e.g., `sys/TimerFired@1`).
pub fn find_builtin_schema(name: &str) -> Option<&'static BuiltinSchema> {
    BUILTIN_SCHEMA_INDEX
        .get(name)
        .and_then(|idx| BUILTIN_SCHEMAS.get(*idx))
}

/// Finds a built-in workflow definition by name (e.g., `sys/Workspace@1`).
pub fn find_builtin_workflow(name: &str) -> Option<&'static BuiltinWorkflow> {
    BUILTIN_WORKFLOW_INDEX
        .get(name)
        .and_then(|idx| BUILTIN_WORKFLOWS.get(*idx))
}

/// Finds a built-in effect definition by name (e.g., `sys/http.request@1`).
pub fn find_builtin_effect(name: &str) -> Option<&'static BuiltinEffect> {
    BUILTIN_EFFECT_INDEX
        .get(name)
        .and_then(|idx| BUILTIN_EFFECTS.get(*idx))
}

/// Finds a built-in module definition by name (e.g., `sys/Workspace@1`).
pub fn find_builtin_module(name: &str) -> Option<&'static BuiltinModule> {
    BUILTIN_MODULE_INDEX
        .get(name)
        .and_then(|idx| BUILTIN_MODULES.get(*idx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_expected_schema_names() {
        let names: Vec<_> = builtin_schemas()
            .iter()
            .map(|s| s.schema.name.as_str())
            .collect();
        // Timer/Blob
        assert!(names.contains(&"sys/TimerSetParams@1"));
        assert!(names.contains(&"sys/TimerSetReceipt@1"));
        assert!(names.contains(&"sys/PortalSendParams@1"));
        assert!(names.contains(&"sys/PortalSendReceipt@1"));
        assert!(names.contains(&"sys/TimerFired@1"));
        assert!(names.contains(&"sys/HostMount@1"));
        assert!(names.contains(&"sys/HostLocalTarget@1"));
        assert!(names.contains(&"sys/HostSandboxTarget@1"));
        assert!(names.contains(&"sys/HostTarget@1"));
        assert!(names.contains(&"sys/HostSessionOpenParams@1"));
        assert!(names.contains(&"sys/HostSessionOpenReceipt@1"));
        assert!(names.contains(&"sys/HostOutput@1"));
        assert!(names.contains(&"sys/HostExecParams@1"));
        assert!(names.contains(&"sys/HostExecReceipt@1"));
        assert!(names.contains(&"sys/HostExecProgressFrame@1"));
        assert!(names.contains(&"sys/HostSessionSignalParams@1"));
        assert!(names.contains(&"sys/HostSessionSignalReceipt@1"));
        assert!(names.contains(&"sys/HostBlobRefInput@1"));
        assert!(names.contains(&"sys/HostFileContentInput@1"));
        assert!(names.contains(&"sys/HostFsReadFileParams@1"));
        assert!(names.contains(&"sys/HostFsReadFileReceipt@1"));
        assert!(names.contains(&"sys/HostFsWriteFileParams@1"));
        assert!(names.contains(&"sys/HostFsWriteFileReceipt@1"));
        assert!(names.contains(&"sys/HostFsEditFileParams@1"));
        assert!(names.contains(&"sys/HostFsEditFileReceipt@1"));
        assert!(names.contains(&"sys/HostPatchInput@1"));
        assert!(names.contains(&"sys/HostPatchOpsSummary@1"));
        assert!(names.contains(&"sys/HostFsApplyPatchParams@1"));
        assert!(names.contains(&"sys/HostFsApplyPatchReceipt@1"));
        assert!(names.contains(&"sys/HostTextOutput@1"));
        assert!(names.contains(&"sys/HostFsGrepParams@1"));
        assert!(names.contains(&"sys/HostFsGrepReceipt@1"));
        assert!(names.contains(&"sys/HostFsGlobParams@1"));
        assert!(names.contains(&"sys/HostFsGlobReceipt@1"));
        assert!(names.contains(&"sys/HostFsStatParams@1"));
        assert!(names.contains(&"sys/HostFsStatReceipt@1"));
        assert!(names.contains(&"sys/HostFsExistsParams@1"));
        assert!(names.contains(&"sys/HostFsExistsReceipt@1"));
        assert!(names.contains(&"sys/HostFsListDirParams@1"));
        assert!(names.contains(&"sys/HostFsListDirReceipt@1"));
        assert!(names.contains(&"sys/BlobPutParams@1"));
        assert!(names.contains(&"sys/BlobEdge@1"));
        assert!(names.contains(&"sys/BlobPutReceipt@1"));
        assert!(names.contains(&"sys/BlobPutResult@1"));
        assert!(names.contains(&"sys/BlobGetParams@1"));
        assert!(names.contains(&"sys/BlobGetReceipt@1"));
        assert!(names.contains(&"sys/BlobGetResult@1"));
        assert!(names.contains(&"sys/WorkflowContext@1"));
        assert!(names.contains(&"sys/PureContext@1"));
        assert!(names.contains(&"sys/PendingEffect@1"));
        assert!(names.contains(&"sys/EffectReceiptEnvelope@1"));
        assert!(names.contains(&"sys/EffectStreamFrame@1"));
        assert!(names.contains(&"sys/EffectReceiptRejected@1"));
        assert!(names.contains(&"sys/PendingEffectSetText@1"));
        assert!(names.contains(&"sys/PendingBatchGroupText@1"));
        assert!(names.contains(&"sys/PendingBatchText@1"));
        // HTTP/LLM
        assert!(names.contains(&"sys/HttpRequestParams@1"));
        assert!(names.contains(&"sys/HttpRequestReceipt@1"));
        assert!(names.contains(&"sys/LlmGenerateParams@1"));
        assert!(names.contains(&"sys/LlmGenerateReceipt@1"));
        // Secrets
        assert!(names.contains(&"sys/SecretRef@1"));
        assert!(names.contains(&"sys/TextOrSecretRef@1"));
        assert!(names.contains(&"sys/BytesOrSecretRef@1"));
        assert!(names.contains(&"sys/VaultPutParams@1"));
        assert!(names.contains(&"sys/VaultPutReceipt@1"));
        assert!(names.contains(&"sys/VaultRotateParams@1"));
        assert!(names.contains(&"sys/VaultRotateReceipt@1"));
        // Governance
        assert!(names.contains(&"sys/GovPatchInput@1"));
        assert!(names.contains(&"sys/GovPatchSummary@1"));
        assert!(names.contains(&"sys/GovProposeParams@1"));
        assert!(names.contains(&"sys/GovProposeReceipt@1"));
        assert!(names.contains(&"sys/GovShadowParams@1"));
        assert!(names.contains(&"sys/GovShadowReceipt@1"));
        assert!(names.contains(&"sys/GovApproveParams@1"));
        assert!(names.contains(&"sys/GovApproveReceipt@1"));
        assert!(names.contains(&"sys/GovApplyParams@1"));
        assert!(names.contains(&"sys/GovApplyReceipt@1"));
        assert!(names.contains(&"sys/GovActionRequested@1"));
        // Workspace
        assert!(names.contains(&"sys/WorkspaceName@1"));
        assert!(names.contains(&"sys/WorkspaceRef@1"));
        assert!(names.contains(&"sys/HttpPublishRule@1"));
        assert!(names.contains(&"sys/HttpPublishRegistry@1"));
        assert!(names.contains(&"sys/HttpPublishSet@1"));
        assert!(names.contains(&"sys/WorkspaceCommitMeta@1"));
        assert!(names.contains(&"sys/WorkspaceHistory@1"));
        assert!(names.contains(&"sys/WorkspaceCommit@1"));
        assert!(names.contains(&"sys/WorkspaceEntry@1"));
        assert!(names.contains(&"sys/WorkspaceTree@1"));
        assert!(names.contains(&"sys/WorkspaceEntry@2"));
        assert!(names.contains(&"sys/WorkspaceTree@2"));
        assert!(names.contains(&"sys/WorkspaceAnnotations@1"));
        assert!(names.contains(&"sys/WorkspaceAnnotationsPatch@1"));
        assert!(names.contains(&"sys/WorkspaceResolveParams@1"));
        assert!(names.contains(&"sys/WorkspaceResolveReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceListParams@1"));
        assert!(names.contains(&"sys/WorkspaceListEntry@1"));
        assert!(names.contains(&"sys/WorkspaceListReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceReadRefParams@1"));
        assert!(names.contains(&"sys/WorkspaceRefEntry@1"));
        assert!(names.contains(&"sys/WorkspaceReadRefReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceReadBytesParams@1"));
        assert!(names.contains(&"sys/WorkspaceReadBytesReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceWriteBytesParams@1"));
        assert!(names.contains(&"sys/WorkspaceWriteBytesReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceWriteRefParams@1"));
        assert!(names.contains(&"sys/WorkspaceWriteRefReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceRemoveParams@1"));
        assert!(names.contains(&"sys/WorkspaceRemoveReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceDiffParams@1"));
        assert!(names.contains(&"sys/WorkspaceDiffChange@1"));
        assert!(names.contains(&"sys/WorkspaceDiffReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceAnnotationsGetParams@1"));
        assert!(names.contains(&"sys/WorkspaceAnnotationsGetReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceAnnotationsSetParams@1"));
        assert!(names.contains(&"sys/WorkspaceAnnotationsSetReceipt@1"));
        assert!(names.contains(&"sys/WorkspaceEmptyRootParams@1"));
        assert!(names.contains(&"sys/WorkspaceEmptyRootReceipt@1"));
        // Introspection
        assert!(names.contains(&"sys/ReadMeta@1"));
        assert!(names.contains(&"sys/IntrospectManifestParams@1"));
        assert!(names.contains(&"sys/IntrospectManifestReceipt@1"));
        assert!(names.contains(&"sys/IntrospectWorkflowStateParams@1"));
        assert!(names.contains(&"sys/IntrospectWorkflowStateReceipt@1"));
        assert!(names.contains(&"sys/IntrospectJournalHeadParams@1"));
        assert!(names.contains(&"sys/IntrospectJournalHeadReceipt@1"));
        assert!(names.contains(&"sys/IntrospectListCellsParams@1"));
        assert!(names.contains(&"sys/IntrospectListCellsReceipt@1"));
    }

    #[test]
    fn lookup_returns_same_instance() {
        let timer = find_builtin_schema("sys/TimerSetParams@1").expect("timer params");
        assert_eq!(timer.schema.name.as_str(), "sys/TimerSetParams@1");
    }

    #[test]
    fn exposes_expected_modules() {
        let names: Vec<_> = builtin_modules()
            .iter()
            .map(|m| m.module.name.as_str())
            .collect();
        for name in [
            "sys/builtin_effects@1",
            "sys/workspace_wasm@1",
            "sys/http_publish_wasm@1",
        ] {
            assert!(names.contains(&name));
        }
    }

    #[test]
    fn exposes_expected_workflows_and_effects() {
        let workflow_names: Vec<_> = builtin_workflows()
            .iter()
            .map(|workflow| workflow.workflow.name.as_str())
            .collect();
        for name in ["sys/Workspace@1", "sys/HttpPublish@1"] {
            assert!(workflow_names.contains(&name));
        }

        let effect_names: Vec<_> = builtin_effects()
            .iter()
            .map(|effect| effect.effect.name.as_str())
            .collect();
        for name in ["sys/timer.set@1"] {
            assert!(effect_names.contains(&name));
        }
    }
}
