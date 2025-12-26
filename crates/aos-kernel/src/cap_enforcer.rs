use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aos_air_types::{DefModule, Name};
use aos_cbor::to_canonical_cbor;
use aos_store::Store;
use aos_wasm_abi::{ABI_VERSION, PureInput, PureOutput};
use serde::{Deserialize, Serialize};
use serde_bytes;

use crate::error::KernelError;
use crate::journal::CapDenyReason;
use crate::pure::PureRegistry;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapCheckInput {
    pub cap_def: String,
    pub grant_name: String,
    #[serde(with = "serde_bytes")]
    pub cap_params: Vec<u8>,
    pub effect_kind: String,
    #[serde(with = "serde_bytes")]
    pub effect_params: Vec<u8>,
    pub origin: CapEffectOrigin,
    pub logical_now_ns: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapEffectOrigin {
    pub kind: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapCheckOutput {
    pub constraints_ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<CapDenyReason>,
}

pub trait CapEnforcerInvoker: Send + Sync {
    fn check(&self, module: &str, input: CapCheckInput) -> Result<CapCheckOutput, KernelError>;
}

pub struct PureCapEnforcer<S: Store> {
    module_defs: Arc<HashMap<Name, DefModule>>,
    pures: Arc<Mutex<PureRegistry<S>>>,
}

impl<S: Store> PureCapEnforcer<S> {
    pub fn new(
        module_defs: Arc<HashMap<Name, DefModule>>,
        pures: Arc<Mutex<PureRegistry<S>>>,
    ) -> Self {
        Self { module_defs, pures }
    }
}

impl<S: Store> CapEnforcerInvoker for PureCapEnforcer<S> {
    fn check(&self, module: &str, input: CapCheckInput) -> Result<CapCheckOutput, KernelError> {
        let module_def = self
            .module_defs
            .get(module)
            .ok_or_else(|| KernelError::PureNotFound(module.to_string()))?;
        if module_def.module_kind != aos_air_types::ModuleKind::Pure {
            return Err(KernelError::Manifest(format!(
                "module '{module}' is not a pure module"
            )));
        }
        let input_bytes = to_canonical_cbor(&input)
            .map_err(|err| KernelError::Manifest(format!("encode cap input: {err}")))?;
        let pure_input = PureInput {
            version: ABI_VERSION,
            input: input_bytes,
        };
        let output = {
            let mut pures = self
                .pures
                .lock()
                .map_err(|_| KernelError::Manifest("pure registry lock poisoned".into()))?;
            pures.ensure_loaded(module, module_def)?;
            pures.invoke(module, &pure_input)?
        };
        decode_cap_output(&output)
    }
}

fn decode_cap_output(output: &PureOutput) -> Result<CapCheckOutput, KernelError> {
    serde_cbor::from_slice(&output.output)
        .map_err(|err| KernelError::Manifest(format!("decode cap output: {err}")))
}
