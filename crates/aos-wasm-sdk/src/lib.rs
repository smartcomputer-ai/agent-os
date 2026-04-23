#![cfg_attr(not(any(test, feature = "std")), no_std)]

extern crate alloc;

mod shared_effects;
mod workflow_effects;
mod workflows;

pub use aos_wasm_abi::WorkflowContext;
pub use shared_effects::*;
pub use workflow_effects::*;
pub use workflows::*;

#[cfg(feature = "air-macros")]
pub use aos_air_macros::{AirSchema, aos_workflow as air_workflow};
pub use serde_cbor::Value;

/// Metadata emitted by Rust-authored AIR schema derives.
///
/// The derive macro only emits static metadata. Host-side authoring tools are responsible for
/// materializing this JSON under `air/generated/` and feeding it through the normal AIR loader.
pub trait AirSchemaRef {
    const AIR_SCHEMA_NAME: &'static str;
}

pub trait AirSchemaExport: AirSchemaRef {
    const AIR_SCHEMA_JSON: &'static str;

    fn air_schema_json() -> String {
        String::from(Self::AIR_SCHEMA_JSON)
    }

    fn air_schema_type_json() -> String {
        String::new()
    }
}

/// Metadata emitted by Rust-authored AIR workflow attributes.
///
/// The workflow macro exports the AIR module and workflow definitions as static JSON fragments.
/// Host-side authoring tools are responsible for replacing the placeholder WASM artifact hash
/// during normal generated AIR materialization.
pub trait AirWorkflowExport {
    const AIR_MODULE_NAME: &'static str;
    const AIR_WORKFLOW_NAME: &'static str;
    const AIR_MODULE_JSON: &'static str;
    const AIR_WORKFLOW_JSON: &'static str;

    fn air_module_json() -> String {
        String::from(Self::AIR_MODULE_JSON)
    }

    fn air_workflow_json() -> String {
        String::from(Self::AIR_WORKFLOW_JSON)
    }
}

/// Metadata for an AIR effect reference.
///
/// The built-in implementations map parameter payload types to their effect definition names.
pub trait AirEffectRef {
    const AIR_EFFECT_NAME: &'static str;
}

/// Metadata emitted by Rust-authored AIR secret declarations.
pub trait AirSecretExport {
    const AIR_SECRET_NAME: &'static str;
    const AIR_SECRET_JSON: &'static str;
}

/// Metadata emitted by Rust-authored AIR manifest declarations.
pub trait AirManifestExport {
    const AIR_MANIFEST_JSON: &'static str;
}

#[derive(Clone, Copy, Debug)]
pub struct AirRoute {
    pub event: &'static str,
    pub workflow: &'static str,
    pub key_field: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct AirManifestParts<'a> {
    pub air_version: &'static str,
    pub schemas: &'a [&'static str],
    pub modules: &'a [&'static str],
    pub workflows: &'a [&'static str],
    pub secrets: &'a [&'static str],
    pub effects: &'a [&'static str],
    pub routes: &'a [AirRoute],
}

pub fn air_manifest_json(parts: AirManifestParts<'_>) -> String {
    let mut out = String::from(r#"{"$kind":"manifest","air_version":"#);
    push_json_string(&mut out, parts.air_version);
    out.push(',');
    push_named_ref_array(&mut out, "schemas", parts.schemas);
    out.push(',');
    push_named_ref_array(&mut out, "modules", parts.modules);
    out.push(',');
    push_named_ref_array(&mut out, "workflows", parts.workflows);
    out.push(',');
    push_named_ref_array(&mut out, "secrets", parts.secrets);
    out.push(',');
    push_named_ref_array(&mut out, "effects", parts.effects);
    out.push_str(r#","routing":{"subscriptions":["#);
    for (idx, route) in parts.routes.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(r#"{"event":"#);
        push_json_string(&mut out, route.event);
        out.push_str(r#","workflow":"#);
        push_json_string(&mut out, route.workflow);
        out.push_str(r#","key_field":"#);
        push_json_string(&mut out, route.key_field);
        out.push('}');
    }
    out.push_str("]}}");
    out
}

fn push_named_ref_array(out: &mut String, key: &str, names: &[&str]) {
    push_json_string(out, key);
    out.push_str(":[");
    for (idx, name) in names.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(r#"{"name":"#);
        push_json_string(out, name);
        out.push('}');
    }
    out.push(']');
}

#[doc(hidden)]
pub fn push_air_json_string(out: &mut String, value: &str) {
    push_json_string(out, value);
}

#[doc(hidden)]
pub mod __aos_export {
    pub use alloc::string::String;
    pub use alloc::vec::Vec;
}

macro_rules! impl_air_schema_ref {
    ($ty:ty => $name:literal) => {
        impl $crate::AirSchemaRef for $ty {
            const AIR_SCHEMA_NAME: &'static str = $name;
        }
    };
}

macro_rules! impl_air_effect_ref {
    ($ty:ty => $name:literal) => {
        impl $crate::AirEffectRef for $ty {
            const AIR_EFFECT_NAME: &'static str = $name;
        }
    };
}

impl_air_schema_ref!(aos_wasm_abi::WorkflowContext => "sys/WorkflowContext@1");
impl_air_schema_ref!(aos_wasm_abi::PureContext => "sys/PureContext@1");
impl_air_schema_ref!(crate::PendingEffect => "sys/PendingEffect@1");
impl_air_schema_ref!(crate::PendingEffectSet<alloc::string::String> => "sys/PendingEffectSetText@1");
impl_air_schema_ref!(crate::PendingBatch<alloc::string::String> => "sys/PendingBatchText@1");
impl_air_schema_ref!(crate::EffectReceiptEnvelope => "sys/EffectReceiptEnvelope@1");
impl_air_schema_ref!(crate::EffectReceiptRejected => "sys/EffectReceiptRejected@1");
impl_air_schema_ref!(crate::EffectStreamFrameEnvelope => "sys/EffectStreamFrame@1");

impl_air_schema_ref!(aos_effect_types::TimerSetParams => "sys/TimerSetParams@1");
impl_air_schema_ref!(aos_effect_types::TimerSetReceipt => "sys/TimerSetReceipt@1");
impl_air_schema_ref!(aos_effect_types::PortalSendParams => "sys/PortalSendParams@1");
impl_air_schema_ref!(aos_effect_types::PortalSendReceipt => "sys/PortalSendReceipt@1");
impl_air_schema_ref!(aos_effect_types::BlobPutParams => "sys/BlobPutParams@1");
impl_air_schema_ref!(aos_effect_types::BlobEdge => "sys/BlobEdge@1");
impl_air_schema_ref!(aos_effect_types::BlobPutReceipt => "sys/BlobPutReceipt@1");
impl_air_schema_ref!(aos_effect_types::BlobGetParams => "sys/BlobGetParams@1");
impl_air_schema_ref!(aos_effect_types::BlobGetReceipt => "sys/BlobGetReceipt@1");
impl_air_schema_ref!(aos_effect_types::HttpRequestParams => "sys/HttpRequestParams@1");
impl_air_schema_ref!(aos_effect_types::HttpRequestReceipt => "sys/HttpRequestReceipt@1");
impl_air_schema_ref!(aos_effect_types::SecretRef => "sys/SecretRef@1");
impl_air_schema_ref!(aos_effect_types::TextOrSecretRef => "sys/TextOrSecretRef@1");
impl_air_schema_ref!(aos_effect_types::LlmToolChoice => "sys/LlmToolChoice@1");
impl_air_schema_ref!(aos_effect_types::LlmRuntimeArgs => "sys/LlmRuntimeArgs@1");
impl_air_schema_ref!(aos_effect_types::LlmGenerateParams => "sys/LlmGenerateParams@1");
impl_air_schema_ref!(aos_effect_types::LlmToolCall => "sys/LlmToolCall@1");
impl_air_schema_ref!(aos_effect_types::LlmOutputEnvelope => "sys/LlmOutputEnvelope@1");
impl_air_schema_ref!(aos_effect_types::LlmGenerateReceipt => "sys/LlmGenerateReceipt@1");
impl_air_schema_ref!(aos_effect_types::VaultPutParams => "sys/VaultPutParams@1");
impl_air_schema_ref!(aos_effect_types::VaultPutReceipt => "sys/VaultPutReceipt@1");
impl_air_schema_ref!(aos_effect_types::VaultRotateParams => "sys/VaultRotateParams@1");
impl_air_schema_ref!(aos_effect_types::VaultRotateReceipt => "sys/VaultRotateReceipt@1");

impl_air_schema_ref!(aos_effect_types::HostMount => "sys/HostMount@1");
impl_air_schema_ref!(aos_effect_types::HostLocalTarget => "sys/HostLocalTarget@1");
impl_air_schema_ref!(aos_effect_types::HostSandboxTarget => "sys/HostSandboxTarget@1");
impl_air_schema_ref!(aos_effect_types::HostTarget => "sys/HostTarget@1");
impl_air_schema_ref!(aos_effect_types::HostSessionOpenParams => "sys/HostSessionOpenParams@1");
impl_air_schema_ref!(aos_effect_types::HostSessionOpenReceipt => "sys/HostSessionOpenReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostOutput => "sys/HostOutput@1");
impl_air_schema_ref!(aos_effect_types::HostExecParams => "sys/HostExecParams@1");
impl_air_schema_ref!(aos_effect_types::HostExecReceipt => "sys/HostExecReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostExecProgressFrame => "sys/HostExecProgressFrame@1");
impl_air_schema_ref!(aos_effect_types::HostSessionSignalParams => "sys/HostSessionSignalParams@1");
impl_air_schema_ref!(aos_effect_types::HostSessionSignalReceipt => "sys/HostSessionSignalReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostBlobRefInput => "sys/HostBlobRefInput@1");
impl_air_schema_ref!(aos_effect_types::HostFileContentInput => "sys/HostFileContentInput@1");
impl_air_schema_ref!(aos_effect_types::HostFsReadFileParams => "sys/HostFsReadFileParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsReadFileReceipt => "sys/HostFsReadFileReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostFsWriteFileParams => "sys/HostFsWriteFileParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsWriteFileReceipt => "sys/HostFsWriteFileReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostFsEditFileParams => "sys/HostFsEditFileParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsEditFileReceipt => "sys/HostFsEditFileReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostPatchInput => "sys/HostPatchInput@1");
impl_air_schema_ref!(aos_effect_types::HostPatchOpsSummary => "sys/HostPatchOpsSummary@1");
impl_air_schema_ref!(aos_effect_types::HostFsApplyPatchParams => "sys/HostFsApplyPatchParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsApplyPatchReceipt => "sys/HostFsApplyPatchReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostTextOutput => "sys/HostTextOutput@1");
impl_air_schema_ref!(aos_effect_types::HostFsGrepParams => "sys/HostFsGrepParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsGrepReceipt => "sys/HostFsGrepReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostFsGlobParams => "sys/HostFsGlobParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsGlobReceipt => "sys/HostFsGlobReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostFsStatParams => "sys/HostFsStatParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsStatReceipt => "sys/HostFsStatReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostFsExistsParams => "sys/HostFsExistsParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsExistsReceipt => "sys/HostFsExistsReceipt@1");
impl_air_schema_ref!(aos_effect_types::HostFsListDirParams => "sys/HostFsListDirParams@1");
impl_air_schema_ref!(aos_effect_types::HostFsListDirReceipt => "sys/HostFsListDirReceipt@1");

impl_air_schema_ref!(aos_effect_types::GovPatchInput => "sys/GovPatchInput@1");
impl_air_schema_ref!(aos_effect_types::GovPatchSummary => "sys/GovPatchSummary@1");
impl_air_schema_ref!(aos_effect_types::GovProposeParams => "sys/GovProposeParams@1");
impl_air_schema_ref!(aos_effect_types::GovProposeReceipt => "sys/GovProposeReceipt@1");
impl_air_schema_ref!(aos_effect_types::GovShadowParams => "sys/GovShadowParams@1");
impl_air_schema_ref!(aos_effect_types::GovShadowReceipt => "sys/GovShadowReceipt@1");
impl_air_schema_ref!(aos_effect_types::GovApproveParams => "sys/GovApproveParams@1");
impl_air_schema_ref!(aos_effect_types::GovApproveReceipt => "sys/GovApproveReceipt@1");
impl_air_schema_ref!(aos_effect_types::GovApplyParams => "sys/GovApplyParams@1");
impl_air_schema_ref!(aos_effect_types::GovApplyReceipt => "sys/GovApplyReceipt@1");

impl_air_schema_ref!(aos_effect_types::ReadMeta => "sys/ReadMeta@1");
impl_air_schema_ref!(aos_effect_types::IntrospectManifestParams => "sys/IntrospectManifestParams@1");
impl_air_schema_ref!(aos_effect_types::IntrospectManifestReceipt => "sys/IntrospectManifestReceipt@1");
impl_air_schema_ref!(aos_effect_types::IntrospectWorkflowStateParams => "sys/IntrospectWorkflowStateParams@1");
impl_air_schema_ref!(aos_effect_types::IntrospectWorkflowStateReceipt => "sys/IntrospectWorkflowStateReceipt@1");
impl_air_schema_ref!(aos_effect_types::IntrospectJournalHeadParams => "sys/IntrospectJournalHeadParams@1");
impl_air_schema_ref!(aos_effect_types::IntrospectJournalHeadReceipt => "sys/IntrospectJournalHeadReceipt@1");
impl_air_schema_ref!(aos_effect_types::IntrospectListCellsParams => "sys/IntrospectListCellsParams@1");
impl_air_schema_ref!(aos_effect_types::IntrospectListCellsReceipt => "sys/IntrospectListCellsReceipt@1");

impl_air_schema_ref!(aos_effect_types::WorkspaceResolveParams => "sys/WorkspaceResolveParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceResolveReceipt => "sys/WorkspaceResolveReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceListParams => "sys/WorkspaceListParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceListEntry => "sys/WorkspaceListEntry@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceListReceipt => "sys/WorkspaceListReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceReadRefParams => "sys/WorkspaceReadRefParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceRefEntry => "sys/WorkspaceRefEntry@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceReadBytesParams => "sys/WorkspaceReadBytesParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceWriteBytesParams => "sys/WorkspaceWriteBytesParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceWriteBytesReceipt => "sys/WorkspaceWriteBytesReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceWriteRefParams => "sys/WorkspaceWriteRefParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceWriteRefReceipt => "sys/WorkspaceWriteRefReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceRemoveParams => "sys/WorkspaceRemoveParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceRemoveReceipt => "sys/WorkspaceRemoveReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceDiffParams => "sys/WorkspaceDiffParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceDiffChange => "sys/WorkspaceDiffChange@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceDiffReceipt => "sys/WorkspaceDiffReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceAnnotationsGetParams => "sys/WorkspaceAnnotationsGetParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceAnnotationsGetReceipt => "sys/WorkspaceAnnotationsGetReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceAnnotationsSetParams => "sys/WorkspaceAnnotationsSetParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceAnnotationsSetReceipt => "sys/WorkspaceAnnotationsSetReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceEmptyRootParams => "sys/WorkspaceEmptyRootParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceEmptyRootReceipt => "sys/WorkspaceEmptyRootReceipt@1");

impl_air_effect_ref!(aos_effect_types::HttpRequestParams => "sys/http.request@1");
impl_air_effect_ref!(aos_effect_types::BlobPutParams => "sys/blob.put@1");
impl_air_effect_ref!(aos_effect_types::BlobGetParams => "sys/blob.get@1");
impl_air_effect_ref!(aos_effect_types::TimerSetParams => "sys/timer.set@1");
impl_air_effect_ref!(aos_effect_types::PortalSendParams => "sys/portal.send@1");
impl_air_effect_ref!(aos_effect_types::HostSessionOpenParams => "sys/host.session.open@1");
impl_air_effect_ref!(aos_effect_types::HostExecParams => "sys/host.exec@1");
impl_air_effect_ref!(aos_effect_types::HostSessionSignalParams => "sys/host.session.signal@1");
impl_air_effect_ref!(aos_effect_types::HostFsReadFileParams => "sys/host.fs.read_file@1");
impl_air_effect_ref!(aos_effect_types::HostFsWriteFileParams => "sys/host.fs.write_file@1");
impl_air_effect_ref!(aos_effect_types::HostFsEditFileParams => "sys/host.fs.edit_file@1");
impl_air_effect_ref!(aos_effect_types::HostFsApplyPatchParams => "sys/host.fs.apply_patch@1");
impl_air_effect_ref!(aos_effect_types::HostFsGrepParams => "sys/host.fs.grep@1");
impl_air_effect_ref!(aos_effect_types::HostFsGlobParams => "sys/host.fs.glob@1");
impl_air_effect_ref!(aos_effect_types::HostFsStatParams => "sys/host.fs.stat@1");
impl_air_effect_ref!(aos_effect_types::HostFsExistsParams => "sys/host.fs.exists@1");
impl_air_effect_ref!(aos_effect_types::HostFsListDirParams => "sys/host.fs.list_dir@1");
impl_air_effect_ref!(aos_effect_types::LlmGenerateParams => "sys/llm.generate@1");
impl_air_effect_ref!(aos_effect_types::VaultPutParams => "sys/vault.put@1");
impl_air_effect_ref!(aos_effect_types::VaultRotateParams => "sys/vault.rotate@1");
impl_air_effect_ref!(aos_effect_types::GovProposeParams => "sys/governance.propose@1");
impl_air_effect_ref!(aos_effect_types::GovShadowParams => "sys/governance.shadow@1");
impl_air_effect_ref!(aos_effect_types::GovApproveParams => "sys/governance.approve@1");
impl_air_effect_ref!(aos_effect_types::GovApplyParams => "sys/governance.apply@1");
impl_air_effect_ref!(aos_effect_types::IntrospectManifestParams => "sys/introspect.manifest@1");
impl_air_effect_ref!(
    aos_effect_types::IntrospectWorkflowStateParams => "sys/introspect.workflow_state@1"
);
impl_air_effect_ref!(aos_effect_types::IntrospectJournalHeadParams => "sys/introspect.journal_head@1");
impl_air_effect_ref!(aos_effect_types::IntrospectListCellsParams => "sys/introspect.list_cells@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceResolveParams => "sys/workspace.resolve@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceEmptyRootParams => "sys/workspace.empty_root@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceListParams => "sys/workspace.list@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceReadRefParams => "sys/workspace.read_ref@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceReadBytesParams => "sys/workspace.read_bytes@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceWriteBytesParams => "sys/workspace.write_bytes@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceWriteRefParams => "sys/workspace.write_ref@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceRemoveParams => "sys/workspace.remove@1");
impl_air_effect_ref!(aos_effect_types::WorkspaceDiffParams => "sys/workspace.diff@1");
impl_air_effect_ref!(
    aos_effect_types::WorkspaceAnnotationsGetParams => "sys/workspace.annotations_get@1"
);
impl_air_effect_ref!(
    aos_effect_types::WorkspaceAnnotationsSetParams => "sys/workspace.annotations_set@1"
);

/// Define an AIR secret export as a Rust item.
#[macro_export]
macro_rules! aos_air_secret {
    (
        $vis:vis struct $ty:ident {
            name: $name:literal,
            binding_id: $binding_id:literal $(,)?
        }
    ) => {
        $vis struct $ty;

        impl $crate::AirSecretExport for $ty {
            const AIR_SECRET_NAME: &'static str = $name;
            const AIR_SECRET_JSON: &'static str = concat!(
                r#"{"$kind":"defsecret","name":""#,
                $name,
                r#"","binding_id":""#,
                $binding_id,
                r#""}"#
            );
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __aos_air_route {
    ({
        event_schema: $event:ty,
        workflow: $workflow:ty,
        key_field: $key_field:literal $(,)?
    }) => {
        $crate::AirRoute {
            event: <$event as $crate::AirSchemaRef>::AIR_SCHEMA_NAME,
            workflow: <$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_NAME,
            key_field: $key_field,
        }
    };
    ({
        event: $event:literal,
        workflow: $workflow:ty,
        key_field: $key_field:literal $(,)?
    }) => {
        $crate::AirRoute {
            event: $event,
            workflow: <$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_NAME,
            key_field: $key_field,
        }
    };
    ({
        event: $event:literal,
        workflow_name: $workflow:literal,
        key_field: $key_field:literal $(,)?
    }) => {
        $crate::AirRoute {
            event: $event,
            workflow: $workflow,
            key_field: $key_field,
        }
    };
}

#[macro_export]
macro_rules! aos_air_world {
    (
        $vis:vis fn $name:ident() {
            air_version: $air_version:literal,
            schemas: [ $($schema:ty),* $(,)? ],
            workflows: [ $($workflow:ty),* $(,)? ],
            secrets: [ $($secret:ty),* $(,)? ],
            import_schemas: [ $($import_schema:literal),* $(,)? ],
            import_modules: [ $($import_module:literal),* $(,)? ],
            import_workflows: [ $($import_workflow:literal),* $(,)? ],
            $(effects: [ $($effect:literal),* $(,)? ],)?
            routing: [ $($route:tt),* $(,)? ] $(,)?
        }
    ) => {
        $vis fn $name() -> $crate::__aos_export::Vec<$crate::__aos_export::String> {
            let mut nodes = $crate::__aos_export::Vec::new();
            $(
                nodes.push(<$schema as $crate::AirSchemaExport>::air_schema_json());
            )*
            $(
                nodes.push(<$workflow as $crate::AirWorkflowExport>::air_module_json());
                nodes.push(<$workflow as $crate::AirWorkflowExport>::air_workflow_json());
            )*
            $(
                nodes.push($crate::__aos_export::String::from(
                    <$secret as $crate::AirSecretExport>::AIR_SECRET_JSON,
                ));
            )*

            let manifest_schemas: &[&str] = &[
                $(<$schema as $crate::AirSchemaRef>::AIR_SCHEMA_NAME,)*
                $($import_schema,)*
            ];
            let manifest_modules: &[&str] = &[
                $(<$workflow as $crate::AirWorkflowExport>::AIR_MODULE_NAME,)*
                $($import_module,)*
            ];
            let manifest_workflows: &[&str] = &[
                $(<$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_NAME,)*
                $($import_workflow,)*
            ];
            let manifest_secrets: &[&str] = &[
                $(<$secret as $crate::AirSecretExport>::AIR_SECRET_NAME,)*
            ];
            let manifest_effects: &[&str] = &[$($($effect,)*)?];
            let manifest_routes: &[$crate::AirRoute] = &[
                $($crate::__aos_air_route!($route),)*
            ];

            nodes.push($crate::air_manifest_json($crate::AirManifestParts {
                air_version: $air_version,
                schemas: manifest_schemas,
                modules: manifest_modules,
                workflows: manifest_workflows,
                secrets: manifest_secrets,
                effects: manifest_effects,
                routes: manifest_routes,
            }));
            nodes
        }
    };
}

/// Define an AIR manifest export as a Rust item.
#[macro_export]
macro_rules! aos_air_manifest {
    (
        $vis:vis struct $ty:ident {
            air_version: $air_version:literal,
            schemas: [ $first_schema:literal $(, $schema:literal)* $(,)? ],
            modules: [ $first_module:literal $(, $module:literal)* $(,)? ],
            workflows: [ $first_workflow:literal $(, $workflow:literal)* $(,)? ],
            secrets: [ $first_secret:literal $(, $secret:literal)* $(,)? ],
            effects: [ $first_effect:literal $(, $effect:literal)* $(,)? ],
            routing: [
                {
                    event: $first_route_event:literal,
                    workflow: $first_route_workflow:literal,
                    key_field: $first_route_key_field:literal $(,)?
                }
                $(,
                {
                    event: $route_event:literal,
                    workflow: $route_workflow:literal,
                    key_field: $route_key_field:literal $(,)?
                })*
                $(,)?
            ] $(,)?
        }
    ) => {
        $vis struct $ty;

        impl $crate::AirManifestExport for $ty {
            const AIR_MANIFEST_JSON: &'static str = concat!(
                r#"{"$kind":"manifest","air_version":""#,
                $air_version,
                r#"","schemas":["#,
                r#"{"name":""#,
                $first_schema,
                r#""}"#,
                $(r#",{"name":""#, $schema, r#""}"#,)*
                r#"],"modules":["#,
                r#"{"name":""#,
                $first_module,
                r#""}"#,
                $(r#",{"name":""#, $module, r#""}"#,)*
                r#"],"workflows":["#,
                r#"{"name":""#,
                $first_workflow,
                r#""}"#,
                $(r#",{"name":""#, $workflow, r#""}"#,)*
                r#"],"secrets":["#,
                r#"{"name":""#,
                $first_secret,
                r#""}"#,
                $(r#",{"name":""#, $secret, r#""}"#,)*
                r#"],"effects":["#,
                r#"{"name":""#,
                $first_effect,
                r#""}"#,
                $(r#",{"name":""#, $effect, r#""}"#,)*
                r#"],"routing":{"subscriptions":["#,
                r#"{"event":""#,
                $first_route_event,
                r#"","workflow":""#,
                $first_route_workflow,
                r#"","key_field":""#,
                $first_route_key_field,
                r#""}"#,
                $(
                    r#",{"event":""#,
                    $route_event,
                    r#"","workflow":""#,
                    $route_workflow,
                    r#"","key_field":""#,
                    $route_key_field,
                    r#""}"#,
                )*
                r#"]}}"#
            );
        }
    };
}

/// Collect Rust-authored AIR metadata exported by this crate.
///
/// Package-local export binaries or host-side authoring glue can pass the generated constant to
/// `aos_authoring::write_generated_air_nodes`.
///
/// ```
/// # use aos_wasm_sdk::{AirSchemaExport, AirSchemaRef};
/// # struct TaskSubmitted;
/// # impl AirSchemaRef for TaskSubmitted {
/// #     const AIR_SCHEMA_NAME: &'static str = "demo/TaskSubmitted@1";
/// # }
/// # impl AirSchemaExport for TaskSubmitted {
/// #     const AIR_SCHEMA_JSON: &'static str =
/// #         r#"{"$kind":"defschema","name":"demo/TaskSubmitted@1","type":{"record":{}}}"#;
/// # }
/// aos_wasm_sdk::aos_air_exports! {
///     pub const AOS_AIR_NODES_JSON = [TaskSubmitted];
/// }
///
/// assert_eq!(AOS_AIR_NODES_JSON.len(), 1);
/// let export_payload = aos_wasm_sdk::air_exports_json(AOS_AIR_NODES_JSON);
/// assert!(export_payload.contains("demo/TaskSubmitted@1"));
/// ```
#[macro_export]
macro_rules! aos_air_exports {
    (
        $vis:vis const $name:ident = {
            schemas: [ $($schema:ty),* $(,)? ],
            workflows: [ $($workflow:ty),* $(,)? ],
            secrets: [ $($secret:ty),* $(,)? ],
            manifest: $manifest:ty,
            raw: [ $($raw:expr),* $(,)? ] $(,)?
        };
    ) => {
        $vis const $name: &[&str] = &[
            $(<$schema as $crate::AirSchemaExport>::AIR_SCHEMA_JSON,)*
            $(
                <$workflow as $crate::AirWorkflowExport>::AIR_MODULE_JSON,
                <$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_JSON,
            )*
            $(<$secret as $crate::AirSecretExport>::AIR_SECRET_JSON,)*
            <$manifest as $crate::AirManifestExport>::AIR_MANIFEST_JSON,
            $($raw,)*
        ];
    };
    (
        $vis:vis const $name:ident = {
            schemas: [ $($schema:ty),* $(,)? ],
            workflows: [ $($workflow:ty),* $(,)? ],
            secrets: [ $($secret:ty),* $(,)? ],
            manifest: $manifest:ty $(,)?
        };
    ) => {
        $vis const $name: &[&str] = &[
            $(<$schema as $crate::AirSchemaExport>::AIR_SCHEMA_JSON,)*
            $(
                <$workflow as $crate::AirWorkflowExport>::AIR_MODULE_JSON,
                <$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_JSON,
            )*
            $(<$secret as $crate::AirSecretExport>::AIR_SECRET_JSON,)*
            <$manifest as $crate::AirManifestExport>::AIR_MANIFEST_JSON,
        ];
    };
    (
        $vis:vis const $name:ident = {
            schemas: [ $($schema:ty),* $(,)? ],
            workflows: [ $($workflow:ty),* $(,)? ],
            secrets: [ $($secret:ty),* $(,)? ] $(,)?
        };
    ) => {
        $vis const $name: &[&str] = &[
            $(<$schema as $crate::AirSchemaExport>::AIR_SCHEMA_JSON,)*
            $(
                <$workflow as $crate::AirWorkflowExport>::AIR_MODULE_JSON,
                <$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_JSON,
            )*
            $(<$secret as $crate::AirSecretExport>::AIR_SECRET_JSON,)*
        ];
    };
    (
        $vis:vis const $name:ident = {
            schemas: [ $($schema:ty),* $(,)? ],
            workflows: [ $($workflow:ty),* $(,)? ] $(,)?
        };
    ) => {
        $vis const $name: &[&str] = &[
            $(<$schema as $crate::AirSchemaExport>::AIR_SCHEMA_JSON,)*
            $(
                <$workflow as $crate::AirWorkflowExport>::AIR_MODULE_JSON,
                <$workflow as $crate::AirWorkflowExport>::AIR_WORKFLOW_JSON,
            )*
        ];
    };
    ($vis:vis const $name:ident = [ $($ty:ty),* $(,)? ];) => {
        $vis const $name: &[&str] = &[
            $(<$ty as $crate::AirSchemaExport>::AIR_SCHEMA_JSON),*
        ];
    };
}

use alloc::alloc::{Layout, alloc};
use alloc::string::String;
use core::slice;

/// Encode collected AIR JSON fragments as a JSON string array.
///
/// This is the package-local export binary contract. A Rust-authored AIR package can print this
/// payload, and host-side authoring code can feed it to
/// `aos_authoring::write_generated_air_export_json`.
pub fn air_exports_json(exports: &[&str]) -> String {
    let mut out = String::from("[");
    for (idx, export) in exports.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        push_json_string(&mut out, export);
    }
    out.push(']');
    out
}

pub fn air_exports_json_strings(exports: &[String]) -> String {
    let mut out = String::from("[");
    for (idx, export) in exports.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        push_json_string(&mut out, export);
    }
    out.push(']');
    out
}

fn push_json_string(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{00}'..='\u{1f}' => {
                out.push_str("\\u00");
                push_hex_byte(out, ch as u8);
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
}

fn push_hex_byte(out: &mut String, value: u8) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(HEX[(value >> 4) as usize] as char);
    out.push(HEX[(value & 0x0f) as usize] as char);
}

#[inline]
fn guest_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return core::ptr::null_mut();
    }
    let layout = Layout::from_size_align(len, 8).expect("layout");
    unsafe { alloc(layout) }
}

unsafe fn read_input<'a>(ptr: i32, len: i32) -> &'a [u8] {
    if ptr == 0 || len <= 0 {
        return &[];
    }
    unsafe { slice::from_raw_parts(ptr as *const u8, len as usize) }
}

fn write_back(bytes: &[u8]) -> (i32, i32) {
    let len = bytes.len();
    if len == 0 {
        return (0, 0);
    }
    let ptr = guest_alloc(len);
    unsafe {
        let out = slice::from_raw_parts_mut(ptr as *mut u8, len);
        out.copy_from_slice(bytes);
    }
    (ptr as i32, len as i32)
}

/// Exported allocator body (used by the macro-generated `alloc`).
pub fn exported_alloc(len: i32) -> i32 {
    if len <= 0 {
        return 0;
    }
    guest_alloc(len as usize) as i32
}

#[cfg(test)]
mod tests {
    use super::{AirManifestExport, AirSecretExport, air_exports_json};

    struct DemoTask;

    impl super::AirSchemaRef for DemoTask {
        const AIR_SCHEMA_NAME: &'static str = "demo/Task@1";
    }

    impl super::AirSchemaExport for DemoTask {
        const AIR_SCHEMA_JSON: &'static str =
            r#"{"$kind":"defschema","name":"demo/Task@1","type":{"record":{}}}"#;
    }

    struct DemoWorkflow;

    impl super::AirWorkflowExport for DemoWorkflow {
        const AIR_MODULE_NAME: &'static str = "demo/Workflow_wasm@1";
        const AIR_WORKFLOW_NAME: &'static str = "demo/Workflow@1";
        const AIR_MODULE_JSON: &'static str = r#"{"$kind":"defmodule","name":"demo/Workflow_wasm@1","runtime":{"kind":"wasm","artifact":{"kind":"wasm_module"}}}"#;
        const AIR_WORKFLOW_JSON: &'static str = r#"{"$kind":"defworkflow","name":"demo/Workflow@1","state":"demo/State@1","event":"demo/Task@1","effects_emitted":[],"impl":{"module":"demo/Workflow_wasm@1","entrypoint":"step"}}"#;
    }

    crate::aos_air_secret! {
        struct OpenAiSecret {
            name: "llm/openai_api@1",
            binding_id: "llm/openai_api",
        }
    }

    crate::aos_air_world! {
        fn demo_air_nodes() {
            air_version: "2",
            schemas: [DemoTask],
            workflows: [DemoWorkflow],
            secrets: [OpenAiSecret],
            import_schemas: ["aos.agent/SessionId@1"],
            import_modules: [],
            import_workflows: [],
            routing: [
                {
                    event_schema: DemoTask,
                    workflow: DemoWorkflow,
                    key_field: "task_id",
                }
            ],
        }
    }

    crate::aos_air_manifest! {
        struct DemoManifest {
            air_version: "2",
            schemas: ["demo/Task@1"],
            modules: ["demo/Workflow_wasm@1"],
            workflows: ["demo/Workflow@1"],
            secrets: ["llm/openai_api@1"],
            effects: ["sys/blob.put@1"],
            routing: [
                {
                    event: "demo/Task@1",
                    workflow: "demo/Workflow@1",
                    key_field: "task_id",
                }
            ],
        }
    }

    #[test]
    fn air_exports_json_encodes_string_array_payload() {
        let payload = air_exports_json(&[
            r#"{"$kind":"defschema","name":"demo/Task@1","type":{"record":{}}}"#,
            "line\nbreak",
        ]);
        let decoded: Vec<String> = serde_json::from_str(&payload).expect("parse export payload");
        assert_eq!(decoded.len(), 2);
        assert_eq!(
            decoded[0],
            r#"{"$kind":"defschema","name":"demo/Task@1","type":{"record":{}}}"#
        );
        assert_eq!(decoded[1], "line\nbreak");
    }

    #[test]
    fn aos_air_secret_emits_secret_json() {
        let value: serde_json::Value =
            serde_json::from_str(OpenAiSecret::AIR_SECRET_JSON).expect("parse secret");
        assert_eq!(value["$kind"], "defsecret");
        assert_eq!(value["name"], "llm/openai_api@1");
        assert_eq!(value["binding_id"], "llm/openai_api");
    }

    #[test]
    fn aos_air_manifest_emits_manifest_json() {
        let value: serde_json::Value =
            serde_json::from_str(DemoManifest::AIR_MANIFEST_JSON).expect("parse manifest");
        assert_eq!(value["$kind"], "manifest");
        assert_eq!(value["air_version"], "2");
        assert_eq!(value["schemas"][0]["name"], "demo/Task@1");
        assert_eq!(
            value["routing"]["subscriptions"][0]["workflow"],
            "demo/Workflow@1"
        );
    }

    #[test]
    fn aos_air_world_collects_defs_and_builds_manifest_from_items() {
        let nodes = demo_air_nodes();
        assert_eq!(nodes.len(), 5);
        let manifest: serde_json::Value =
            serde_json::from_str(nodes.last().expect("manifest")).expect("parse manifest");
        assert_eq!(manifest["schemas"][0]["name"], "demo/Task@1");
        assert_eq!(manifest["schemas"][1]["name"], "aos.agent/SessionId@1");
        assert_eq!(manifest["modules"][0]["name"], "demo/Workflow_wasm@1");
        assert_eq!(manifest["workflows"][0]["name"], "demo/Workflow@1");
        assert_eq!(manifest["secrets"][0]["name"], "llm/openai_api@1");
        assert_eq!(manifest["effects"].as_array().expect("effects").len(), 0);
        assert_eq!(
            manifest["routing"]["subscriptions"][0]["event"],
            "demo/Task@1"
        );
    }
}
