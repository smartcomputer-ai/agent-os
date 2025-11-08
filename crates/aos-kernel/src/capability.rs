use std::collections::HashMap;

use aos_air_types::{CapGrant, CapGrantBudget, CapType, DefCap, Manifest, Name, ValueLiteral};
use aos_cbor::to_canonical_cbor;
use aos_effects::{CapabilityBudget, CapabilityGrant};

use crate::error::KernelError;

pub trait CapabilityGate {
    fn resolve(&self, cap_name: &str, effect_kind: &str) -> Result<CapabilityGrant, KernelError>;
}

#[derive(Clone)]
pub struct CapabilityResolver {
    grants: HashMap<String, ResolvedGrant>,
}

#[derive(Clone)]
struct ResolvedGrant {
    grant: CapabilityGrant,
    cap_type: CapType,
}

impl CapabilityResolver {
    fn new(grants: HashMap<String, ResolvedGrant>) -> Self {
        Self { grants }
    }

    pub fn from_runtime_grants<I>(grants: I) -> Self
    where
        I: IntoIterator<Item = (CapabilityGrant, CapType)>,
    {
        let map = grants
            .into_iter()
            .map(|(grant, cap_type)| (grant.name.clone(), ResolvedGrant { grant, cap_type }))
            .collect();
        Self::new(map)
    }

    pub fn has_grant(&self, name: &str) -> bool {
        self.grants.contains_key(name)
    }

    pub fn resolve(
        &self,
        cap_name: &str,
        effect_kind: &str,
    ) -> Result<CapabilityGrant, KernelError> {
        let resolved = self
            .grants
            .get(cap_name)
            .ok_or_else(|| KernelError::CapabilityGrantNotFound(cap_name.to_string()))?;
        let expected = expected_cap_type(effect_kind)?;
        if resolved.cap_type != expected {
            return Err(KernelError::CapabilityTypeMismatch {
                grant: cap_name.to_string(),
                expected: cap_type_as_str(&expected).to_string(),
                found: cap_type_as_str(&resolved.cap_type).to_string(),
                effect_kind: effect_kind.to_string(),
            });
        }
        Ok(resolved.grant.clone())
    }

    pub fn from_manifest(
        manifest: &Manifest,
        caps: &HashMap<Name, DefCap>,
    ) -> Result<Self, KernelError> {
        let mut grants = HashMap::new();
        if let Some(defaults) = manifest.defaults.as_ref() {
            for grant in &defaults.cap_grants {
                if grants.contains_key(&grant.name) {
                    return Err(KernelError::DuplicateCapabilityGrant(grant.name.clone()));
                }
                let resolved = resolve_grant(grant, caps)?;
                grants.insert(grant.name.clone(), resolved);
            }
        }
        Ok(Self::new(grants))
    }
}

fn resolve_grant(
    grant: &CapGrant,
    caps: &HashMap<Name, DefCap>,
) -> Result<ResolvedGrant, KernelError> {
    let defcap = caps
        .get(&grant.cap)
        .ok_or_else(|| KernelError::CapabilityDefinitionNotFound(grant.cap.clone()))?;
    let params_cbor = encode_value_literal(&grant.params)?;
    let capability_grant = CapabilityGrant {
        name: grant.name.clone(),
        cap: grant.cap.clone(),
        params_cbor,
        expiry_ns: grant.expiry_ns,
        budget: grant.budget.as_ref().map(convert_budget),
    };
    Ok(ResolvedGrant {
        grant: capability_grant,
        cap_type: defcap.cap_type.clone(),
    })
}

fn convert_budget(budget: &CapGrantBudget) -> CapabilityBudget {
    CapabilityBudget {
        tokens: budget.tokens,
        bytes: budget.bytes,
        cents: budget.cents,
    }
}

fn encode_value_literal(value: &ValueLiteral) -> Result<Vec<u8>, KernelError> {
    to_canonical_cbor(value).map_err(|err| KernelError::CapabilityEncoding(err.to_string()))
}

fn expected_cap_type(effect_kind: &str) -> Result<CapType, KernelError> {
    match effect_kind {
        aos_effects::EffectKind::HTTP_REQUEST => Ok(CapType::HttpOut),
        aos_effects::EffectKind::BLOB_PUT | aos_effects::EffectKind::BLOB_GET => Ok(CapType::Blob),
        aos_effects::EffectKind::TIMER_SET => Ok(CapType::Timer),
        aos_effects::EffectKind::LLM_GENERATE => Ok(CapType::LlmBasic),
        other => Err(KernelError::UnsupportedEffectKind(other.to_string())),
    }
}

fn cap_type_as_str(cap_type: &CapType) -> &'static str {
    match cap_type {
        CapType::HttpOut => "http.out",
        CapType::Blob => "blob",
        CapType::Timer => "timer",
        CapType::LlmBasic => "llm.basic",
    }
}
