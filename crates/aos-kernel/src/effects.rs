use std::sync::Arc;

use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::builtins::BlobPutParams;
use aos_effects::{EffectIntent, EffectSource, effect_ops, normalize_effect_params};
use aos_wasm_abi::WorkflowEffect;

use crate::error::KernelError;
use crate::secret::{SecretResolver, normalize_secret_variants};
use aos_air_types::catalog::EffectCatalog;
use aos_air_types::schema_index::SchemaIndex;

#[derive(Debug, Clone)]
pub struct EffectRuntimeIdentity {
    pub effect_name: String,
    pub effect_hash: Option<String>,
    pub executor_module: Option<String>,
    pub executor_module_hash: Option<String>,
    pub executor_entrypoint: String,
}

#[derive(Default)]
pub struct EffectQueue {
    intents: Vec<EffectIntent>,
}

impl EffectQueue {
    pub fn push(&mut self, intent: EffectIntent) {
        self.intents.push(intent);
    }

    pub fn drain(&mut self) -> Vec<EffectIntent> {
        std::mem::take(&mut self.intents)
    }

    pub fn is_empty(&self) -> bool {
        self.intents.is_empty()
    }

    pub fn as_slice(&self) -> &[EffectIntent] {
        &self.intents
    }

    pub fn set(&mut self, intents: Vec<EffectIntent>) {
        self.intents = intents;
    }
}

pub struct EffectManager {
    queue: EffectQueue,
    effect_catalog: Arc<EffectCatalog>,
    schema_index: Arc<SchemaIndex>,
    param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>>,
    logical_now_ns: u64,
    secret_catalog: Option<crate::secret::SecretCatalog>,
    secret_resolver: Option<Arc<dyn SecretResolver>>,
}

pub trait EffectParamPreprocessor: Send + Sync {
    fn preprocess(
        &self,
        source: &EffectSource,
        effect: &str,
        params_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, KernelError>;
}

impl EffectManager {
    pub fn new(
        effect_catalog: Arc<EffectCatalog>,
        schema_index: Arc<SchemaIndex>,
        param_preprocessor: Option<Arc<dyn EffectParamPreprocessor>>,
        secret_catalog: Option<crate::secret::SecretCatalog>,
        secret_resolver: Option<Arc<dyn SecretResolver>>,
    ) -> Self {
        Self {
            queue: EffectQueue::default(),
            effect_catalog,
            schema_index,
            param_preprocessor,
            logical_now_ns: 0,
            secret_catalog,
            secret_resolver,
        }
    }

    pub fn enqueue_workflow_effect(
        &mut self,
        workflow_name: &str,
        effect: &WorkflowEffect,
    ) -> Result<EffectIntent, KernelError> {
        self.enqueue_workflow_effect_authorized(workflow_name, effect)
    }

    pub fn enqueue_workflow_effect_authorized(
        &mut self,
        workflow_name: &str,
        effect: &WorkflowEffect,
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Workflow {
            name: workflow_name.to_string(),
        };
        let idempotency_key = normalize_idempotency_key(effect.idempotency_key.as_deref())?;
        self.enqueue_authorized_effect(
            source,
            effect.kind.as_str(),
            None,
            effect.params_cbor.clone(),
            idempotency_key,
        )
    }

    pub fn enqueue_workflow_effect_with_identity(
        &mut self,
        workflow_name: &str,
        effect: &WorkflowEffect,
        identity: EffectRuntimeIdentity,
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Workflow {
            name: workflow_name.to_string(),
        };
        let idempotency_key = normalize_idempotency_key(effect.idempotency_key.as_deref())?;
        let effect_name = identity.effect_name.clone();
        self.enqueue_authorized_effect(
            source,
            effect_name.as_str(),
            Some(identity),
            effect.params_cbor.clone(),
            idempotency_key,
        )
    }

    pub fn queued(&self) -> &[EffectIntent] {
        self.queue.as_slice()
    }

    pub fn enqueue_plan_effect(
        &mut self,
        plan_name: &str,
        effect: &str,
        params_cbor: Vec<u8>,
        idempotency_key: [u8; 32],
    ) -> Result<EffectIntent, KernelError> {
        let source = EffectSource::Plan {
            name: plan_name.to_string(),
        };
        self.enqueue_authorized_effect(source, effect, None, params_cbor, idempotency_key)
    }

    fn enqueue_authorized_effect(
        &mut self,
        source: EffectSource,
        effect: &str,
        effect_identity: Option<EffectRuntimeIdentity>,
        params_cbor: Vec<u8>,
        idempotency_key: [u8; 32],
    ) -> Result<EffectIntent, KernelError> {
        let canonical_params = if let Some(identity) = effect_identity.as_ref() {
            self.ensure_effect_known(&identity.effect_name)?;
            self.canonicalize_effect_params(&source, identity.effect_name.as_str(), params_cbor)?
        } else {
            self.ensure_effect_known(effect)?;
            self.canonicalize_effect_params(&source, effect, params_cbor)?
        };
        let intent = if let Some(identity) = effect_identity {
            EffectIntent::from_raw_params_with_identity(
                identity.effect_name,
                identity.effect_hash,
                identity.executor_module,
                identity.executor_module_hash,
                Some(identity.executor_entrypoint),
                canonical_params,
                idempotency_key,
            )
        } else {
            EffectIntent::from_raw_params_with_identity(
                effect.to_string(),
                None,
                None,
                None,
                None,
                canonical_params,
                idempotency_key,
            )
        }
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        Ok(intent)
    }

    fn ensure_effect_known(&self, effect: &str) -> Result<(), KernelError> {
        if self.effect_catalog.params_schema(effect).is_some() {
            Ok(())
        } else {
            Err(KernelError::UnsupportedEffect(effect.into()))
        }
    }

    fn canonicalize_effect_params(
        &self,
        source: &EffectSource,
        effect: &str,
        params_cbor: Vec<u8>,
    ) -> Result<Vec<u8>, KernelError> {
        let params_cbor = if let Some(preprocessor) = &self.param_preprocessor {
            preprocessor.preprocess(source, effect, params_cbor)?
        } else {
            params_cbor
        };
        let params_cbor = if effect == effect_ops::BLOB_PUT {
            normalize_blob_put_params(params_cbor)?
        } else {
            params_cbor
        };

        let canonical_params = normalize_effect_params(
            &self.effect_catalog,
            &self.schema_index,
            effect,
            &params_cbor,
        )
        .map_err(|err| KernelError::EffectManager(err.to_string()))?;
        normalize_secret_variants(&canonical_params)
            .map_err(|err| KernelError::SecretResolution(err.to_string()))
    }

    pub fn drain(&mut self) -> Result<Vec<EffectIntent>, KernelError> {
        let mut intents = self.queue.drain();
        for intent in intents.iter_mut() {
            self.prepare_intent_for_execution(intent)?;
        }
        Ok(intents)
    }

    pub fn has_pending(&self) -> bool {
        !self.queue.is_empty()
    }

    pub fn restore_queue(&mut self, intents: Vec<EffectIntent>) {
        self.queue.set(intents);
    }

    pub fn secret_resolver(&self) -> Option<Arc<dyn SecretResolver>> {
        self.secret_resolver.clone()
    }

    pub fn logical_now_ns(&self) -> u64 {
        self.logical_now_ns
    }

    pub fn update_logical_now_ns(&mut self, logical_now_ns: u64) {
        self.logical_now_ns = self.logical_now_ns.max(logical_now_ns);
    }

    pub fn prepare_intent_for_execution(
        &self,
        intent: &mut EffectIntent,
    ) -> Result<(), KernelError> {
        if let (Some(catalog), Some(resolver)) =
            (self.secret_catalog.as_ref(), self.secret_resolver.as_ref())
        {
            let injected = crate::secret::inject_secrets_in_params(
                &intent.params_cbor,
                catalog,
                resolver.as_ref(),
            )
            .map_err(|err| KernelError::SecretResolution(err.to_string()))?;
            intent.params_cbor = injected;
        }
        Ok(())
    }
}

fn normalize_blob_put_params(params_cbor: Vec<u8>) -> Result<Vec<u8>, KernelError> {
    let mut params: BlobPutParams = serde_cbor::from_slice(&params_cbor)
        .map_err(|err| KernelError::EffectManager(format!("decode blob.put params: {err}")))?;
    let computed = Hash::of_bytes(&params.bytes);
    let computed_ref = aos_air_types::HashRef::new(computed.to_hex())
        .map_err(|err| KernelError::EffectManager(format!("invalid computed blob hash: {err}")))?;
    if let Some(provided_ref) = params.blob_ref.as_ref() {
        if provided_ref != &computed_ref {
            return Err(KernelError::EffectManager(
                "blob.put blob_ref does not match sha256(bytes)".into(),
            ));
        }
    }
    if params.refs.is_none() {
        params.refs = Some(Vec::new());
    }
    params.blob_ref = Some(computed_ref);
    to_canonical_cbor(&params)
        .map_err(|err| KernelError::EffectManager(format!("encode blob.put params: {err}")))
}

fn normalize_idempotency_key(value: Option<&[u8]>) -> Result<[u8; 32], KernelError> {
    match value {
        None => Ok([0u8; 32]),
        Some(bytes) => {
            let hash = aos_cbor::Hash::from_bytes(bytes).map_err(|err| {
                KernelError::IdempotencyKeyInvalid(format!("expected 32 bytes, got {}", err.0))
            })?;
            Ok(*hash.as_bytes())
        }
    }
}
