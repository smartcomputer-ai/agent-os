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
impl_air_schema_ref!(aos_effect_types::LlmCompactParams => "sys/LlmCompactParams@1");
impl_air_schema_ref!(aos_effect_types::LlmCompactReceipt => "sys/LlmCompactReceipt@1");
impl_air_schema_ref!(aos_effect_types::LlmCompactStrategy => "sys/LlmCompactStrategy@1");
impl_air_schema_ref!(
    aos_effect_types::LlmCompactionArtifactKind => "sys/LlmCompactionArtifactKind@1"
);
impl_air_schema_ref!(aos_effect_types::LlmTranscriptRange => "sys/LlmTranscriptRange@1");
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
impl_air_schema_ref!(
    aos_effect_types::IntrospectWorkflowStateReceipt => "sys/IntrospectWorkflowStateReceipt@1"
);
impl_air_schema_ref!(aos_effect_types::IntrospectJournalHeadParams => "sys/IntrospectJournalHeadParams@1");
impl_air_schema_ref!(
    aos_effect_types::IntrospectJournalHeadReceipt => "sys/IntrospectJournalHeadReceipt@1"
);
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
impl_air_schema_ref!(
    aos_effect_types::WorkspaceWriteBytesReceipt => "sys/WorkspaceWriteBytesReceipt@1"
);
impl_air_schema_ref!(aos_effect_types::WorkspaceWriteRefParams => "sys/WorkspaceWriteRefParams@1");
impl_air_schema_ref!(
    aos_effect_types::WorkspaceWriteRefReceipt => "sys/WorkspaceWriteRefReceipt@1"
);
impl_air_schema_ref!(aos_effect_types::WorkspaceRemoveParams => "sys/WorkspaceRemoveParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceRemoveReceipt => "sys/WorkspaceRemoveReceipt@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceDiffParams => "sys/WorkspaceDiffParams@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceDiffChange => "sys/WorkspaceDiffChange@1");
impl_air_schema_ref!(aos_effect_types::WorkspaceDiffReceipt => "sys/WorkspaceDiffReceipt@1");
impl_air_schema_ref!(
    aos_effect_types::WorkspaceAnnotationsGetParams => "sys/WorkspaceAnnotationsGetParams@1"
);
impl_air_schema_ref!(
    aos_effect_types::WorkspaceAnnotationsGetReceipt => "sys/WorkspaceAnnotationsGetReceipt@1"
);
impl_air_schema_ref!(
    aos_effect_types::WorkspaceAnnotationsSetParams => "sys/WorkspaceAnnotationsSetParams@1"
);
impl_air_schema_ref!(
    aos_effect_types::WorkspaceAnnotationsSetReceipt => "sys/WorkspaceAnnotationsSetReceipt@1"
);
impl_air_schema_ref!(aos_effect_types::WorkspaceEmptyRootParams => "sys/WorkspaceEmptyRootParams@1");
impl_air_schema_ref!(
    aos_effect_types::WorkspaceEmptyRootReceipt => "sys/WorkspaceEmptyRootReceipt@1"
);

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
impl_air_effect_ref!(aos_effect_types::LlmCompactParams => "sys/llm.compact@1");
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
