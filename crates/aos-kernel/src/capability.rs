use std::{collections::HashMap, sync::Arc};

use aos_air_types::{
    CapEnforcer, CapGrant, CapType, DefCap, Manifest, Name, TypeExpr, TypeList, TypeMap,
    TypeOption, TypePrimitive, TypeRecord, TypeSet, TypeVariant, ValueLiteral, builtins,
    catalog::EffectCatalog, plan_literals::SchemaIndex, validate_value_literal,
};
use aos_cbor::Hash;
use aos_effects::CapabilityGrant;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as Base64Engine;
use indexmap::IndexMap;

use crate::error::KernelError;

pub trait CapabilityGate {
    fn resolve(&self, cap_name: &str, effect_kind: &str)
    -> Result<CapGrantResolution, KernelError>;
}

#[derive(Clone)]
pub struct CapabilityResolver {
    grants: HashMap<String, ResolvedGrant>,
    effect_catalog: Arc<EffectCatalog>,
}

#[derive(Clone)]
struct ResolvedGrant {
    grant: CapabilityGrant,
    cap_type: CapType,
    enforcer: CapEnforcer,
    grant_hash: [u8; 32],
}

#[derive(Clone)]
pub struct CapGrantResolution {
    pub grant: CapabilityGrant,
    pub cap_type: CapType,
    pub enforcer: CapEnforcer,
    pub grant_hash: [u8; 32],
}

pub const CAP_ALLOW_ALL_ENFORCER: &str = "sys/CapAllowAll@1";
pub const CAP_HTTP_ENFORCER: &str = "sys/CapEnforceHttpOut@1";
pub const CAP_LLM_ENFORCER: &str = "sys/CapEnforceLlmBasic@1";
pub const CAP_WORKSPACE_ENFORCER: &str = "sys/CapEnforceWorkspace@1";

impl CapabilityResolver {
    fn new(grants: HashMap<String, ResolvedGrant>, effect_catalog: Arc<EffectCatalog>) -> Self {
        Self {
            grants,
            effect_catalog,
        }
    }

    pub fn from_runtime_grants<I>(grants: I) -> Result<Self, KernelError>
    where
        I: IntoIterator<Item = (CapabilityGrant, CapType)>,
    {
        let map = grants
            .into_iter()
            .map(|(grant, cap_type)| {
                let enforcer = default_enforcer_for_cap_type(&cap_type);
                let grant_hash = compute_grant_hash(&grant, &cap_type)?;
                Ok((
                    grant.name.clone(),
                    ResolvedGrant {
                        grant,
                        cap_type,
                        enforcer,
                        grant_hash,
                    },
                ))
            })
            .collect::<Result<HashMap<_, _>, KernelError>>()?;
        let catalog =
            EffectCatalog::from_defs(builtins::builtin_effects().iter().map(|e| e.effect.clone()));
        Ok(Self::new(map, Arc::new(catalog)))
    }

    pub fn has_grant(&self, name: &str) -> bool {
        self.grants.contains_key(name)
    }

    pub fn resolve(
        &self,
        cap_name: &str,
        effect_kind: &str,
    ) -> Result<CapGrantResolution, KernelError> {
        let resolved = self
            .grants
            .get(cap_name)
            .ok_or_else(|| KernelError::CapabilityGrantNotFound(cap_name.to_string()))?;
        let expected = expected_cap_type(&self.effect_catalog, effect_kind)?;
        if resolved.cap_type != expected {
            return Err(KernelError::CapabilityTypeMismatch {
                grant: cap_name.to_string(),
                expected: cap_type_as_str(&expected).to_string(),
                found: cap_type_as_str(&resolved.cap_type).to_string(),
                effect_kind: effect_kind.to_string(),
            });
        }
        Ok(CapGrantResolution {
            grant: resolved.grant.clone(),
            cap_type: resolved.cap_type.clone(),
            enforcer: resolved.enforcer.clone(),
            grant_hash: resolved.grant_hash,
        })
    }

    pub fn resolve_grant(&self, cap_name: &str) -> Result<CapGrantResolution, KernelError> {
        let resolved = self
            .grants
            .get(cap_name)
            .ok_or_else(|| KernelError::CapabilityGrantNotFound(cap_name.to_string()))?;
        Ok(CapGrantResolution {
            grant: resolved.grant.clone(),
            cap_type: resolved.cap_type.clone(),
            enforcer: resolved.enforcer.clone(),
            grant_hash: resolved.grant_hash,
        })
    }

    pub fn unique_grant_for_effect_kind(
        &self,
        effect_kind: &str,
    ) -> Result<Option<CapGrantResolution>, KernelError> {
        let expected = expected_cap_type(&self.effect_catalog, effect_kind)?;
        let mut matches = self
            .grants
            .values()
            .filter(|grant| grant.cap_type == expected);
        let first = matches.next();
        if first.is_none() || matches.next().is_some() {
            return Ok(None);
        }
        let resolved = first.expect("first checked");
        Ok(Some(CapGrantResolution {
            grant: resolved.grant.clone(),
            cap_type: resolved.cap_type.clone(),
            enforcer: resolved.enforcer.clone(),
            grant_hash: resolved.grant_hash,
        }))
    }

    pub fn from_manifest(
        manifest: &Manifest,
        caps: &HashMap<Name, DefCap>,
        schema_index: &SchemaIndex,
        effect_catalog: Arc<EffectCatalog>,
    ) -> Result<Self, KernelError> {
        let mut grants = HashMap::new();
        if let Some(defaults) = manifest.defaults.as_ref() {
            for grant in &defaults.cap_grants {
                if grants.contains_key(&grant.name) {
                    return Err(KernelError::DuplicateCapabilityGrant(grant.name.clone()));
                }
                let resolved = resolve_grant(grant, caps, schema_index)?;
                grants.insert(grant.name.clone(), resolved);
            }
        }
        Ok(Self::new(grants, effect_catalog))
    }
}

fn resolve_grant(
    grant: &CapGrant,
    caps: &HashMap<Name, DefCap>,
    schema_index: &SchemaIndex,
) -> Result<ResolvedGrant, KernelError> {
    let defcap = caps
        .get(&grant.cap)
        .ok_or_else(|| KernelError::CapabilityDefinitionNotFound(grant.cap.clone()))?;
    let expanded_schema = expand_cap_schema(&defcap.schema, schema_index, defcap.name.as_str())?;
    validate_value_literal(&grant.params, &expanded_schema).map_err(|err| {
        KernelError::CapabilityParamInvalid {
            grant: grant.name.clone(),
            cap: grant.cap.clone(),
            reason: err.to_string(),
        }
    })?;
    let params_cbor = encode_value_literal(&grant.params, &expanded_schema, schema_index)?;
    let capability_grant = CapabilityGrant {
        name: grant.name.clone(),
        cap: grant.cap.clone(),
        params_cbor,
        expiry_ns: grant.expiry_ns,
    };
    let grant_hash = compute_grant_hash(&capability_grant, &defcap.cap_type)?;
    Ok(ResolvedGrant {
        grant: capability_grant,
        cap_type: defcap.cap_type.clone(),
        enforcer: defcap.enforcer.clone(),
        grant_hash,
    })
}

fn compute_grant_hash(
    grant: &CapabilityGrant,
    cap_type: &CapType,
) -> Result<[u8; 32], KernelError> {
    #[derive(serde::Serialize)]
    struct GrantHashInput<'a> {
        defcap_ref: &'a str,
        cap_type: &'a str,
        #[serde(with = "serde_bytes")]
        params_cbor: &'a [u8],
        #[serde(skip_serializing_if = "Option::is_none")]
        expiry_ns: Option<u64>,
    }

    let input = GrantHashInput {
        defcap_ref: grant.cap.as_str(),
        cap_type: cap_type.as_str(),
        params_cbor: &grant.params_cbor,
        expiry_ns: grant.expiry_ns,
    };
    let hash = Hash::of_cbor(&input)
        .map_err(|err| KernelError::EffectManager(format!("grant hash encoding failed: {err}")))?;
    Ok(*hash.as_bytes())
}

fn encode_value_literal(
    value: &ValueLiteral,
    schema: &TypeExpr,
    schemas: &SchemaIndex,
) -> Result<Vec<u8>, KernelError> {
    let cbor_value = literal_to_cbor_value(value)?;
    let normalized =
        aos_air_types::value_normalize::normalize_value_with_schema(cbor_value, schema, schemas)
            .map_err(|err| KernelError::CapabilityEncoding(err.to_string()))?;
    Ok(normalized.bytes)
}

fn literal_to_cbor_value(value: &ValueLiteral) -> Result<serde_cbor::Value, KernelError> {
    use serde_cbor::Value as CborValue;
    Ok(match value {
        ValueLiteral::Null(_) => CborValue::Null,
        ValueLiteral::Bool(v) => CborValue::Bool(v.bool),
        ValueLiteral::Int(v) => CborValue::Integer(v.int as i128),
        ValueLiteral::Nat(v) => CborValue::Integer(v.nat as i128),
        ValueLiteral::Dec128(v) => CborValue::Text(v.dec128.clone()),
        ValueLiteral::Bytes(v) => {
            let bytes = Base64Engine
                .decode(&v.bytes_b64)
                .map_err(|err| KernelError::CapabilityEncoding(err.to_string()))?;
            CborValue::Bytes(bytes)
        }
        ValueLiteral::Text(v) => CborValue::Text(v.text.clone()),
        ValueLiteral::TimeNs(v) => CborValue::Integer(v.time_ns as i128),
        ValueLiteral::DurationNs(v) => CborValue::Integer(v.duration_ns as i128),
        ValueLiteral::Hash(v) => CborValue::Text(v.hash.as_str().to_string()),
        ValueLiteral::Uuid(v) => CborValue::Text(v.uuid.clone()),
        ValueLiteral::List(v) => CborValue::Array(
            v.list
                .iter()
                .map(literal_to_cbor_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ValueLiteral::Set(v) => CborValue::Array(
            v.set
                .iter()
                .map(literal_to_cbor_value)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        ValueLiteral::Map(v) => {
            let mut map = std::collections::BTreeMap::new();
            for entry in &v.map {
                let key = literal_to_cbor_value(&entry.key)?;
                let value = literal_to_cbor_value(&entry.value)?;
                map.insert(key, value);
            }
            CborValue::Map(map)
        }
        ValueLiteral::Record(v) => {
            let mut map = std::collections::BTreeMap::new();
            for (key, value) in &v.record {
                map.insert(CborValue::Text(key.clone()), literal_to_cbor_value(value)?);
            }
            CborValue::Map(map)
        }
        ValueLiteral::Variant(v) => {
            let mut map = std::collections::BTreeMap::new();
            let value = match &v.value {
                Some(inner) => literal_to_cbor_value(inner)?,
                None => CborValue::Null,
            };
            map.insert(CborValue::Text(v.tag.clone()), value);
            CborValue::Map(map)
        }
        ValueLiteral::SecretRef(_) => {
            return Err(KernelError::CapabilityEncoding(
                "secret_ref literals are not supported in capability params".into(),
            ));
        }
    })
}

fn expected_cap_type(catalog: &EffectCatalog, effect_kind: &str) -> Result<CapType, KernelError> {
    let effect_kind = aos_air_types::EffectKind::new(effect_kind.to_string());
    catalog
        .cap_type(&effect_kind)
        .cloned()
        .ok_or_else(|| KernelError::UnsupportedEffectKind(effect_kind.to_string()))
}

fn cap_type_as_str(cap_type: &CapType) -> &str {
    cap_type.as_str()
}

fn default_enforcer_for_cap_type(cap_type: &CapType) -> CapEnforcer {
    let module = match cap_type.as_str() {
        CapType::HTTP_OUT => CAP_HTTP_ENFORCER,
        CapType::LLM_BASIC => CAP_LLM_ENFORCER,
        CapType::WORKSPACE => CAP_WORKSPACE_ENFORCER,
        _ => CAP_ALLOW_ALL_ENFORCER,
    };
    CapEnforcer {
        module: module.to_string(),
    }
}

fn expand_cap_schema(
    schema: &TypeExpr,
    schema_index: &SchemaIndex,
    context: &str,
) -> Result<TypeExpr, KernelError> {
    match schema {
        TypeExpr::Primitive(_) => Ok(schema.clone()),
        TypeExpr::Record(record) => {
            let mut expanded = IndexMap::with_capacity(record.record.len());
            for (field, field_schema) in &record.record {
                let nested = format!("{context}.{field}");
                expanded.insert(
                    field.clone(),
                    expand_cap_schema(field_schema, schema_index, &nested)?,
                );
            }
            Ok(TypeExpr::Record(TypeRecord { record: expanded }))
        }
        TypeExpr::Variant(variant) => {
            let mut expanded = IndexMap::with_capacity(variant.variant.len());
            for (tag, ty) in &variant.variant {
                let nested = format!("{context}::{tag}");
                expanded.insert(tag.clone(), expand_cap_schema(ty, schema_index, &nested)?);
            }
            Ok(TypeExpr::Variant(TypeVariant { variant: expanded }))
        }
        TypeExpr::List(list) => Ok(TypeExpr::List(TypeList {
            list: Box::new(expand_cap_schema(&list.list, schema_index, context)?),
        })),
        TypeExpr::Set(set) => Ok(TypeExpr::Set(TypeSet {
            set: Box::new(expand_cap_schema(&set.set, schema_index, context)?),
        })),
        TypeExpr::Map(map) => Ok(TypeExpr::Map(TypeMap {
            map: aos_air_types::TypeMapEntry {
                key: map.map.key.clone(),
                value: Box::new(expand_cap_schema(&map.map.value, schema_index, context)?),
            },
        })),
        TypeExpr::Option(opt) => Ok(TypeExpr::Option(TypeOption {
            option: Box::new(expand_cap_schema(&opt.option, schema_index, context)?),
        })),
        TypeExpr::Ref(reference) => {
            let schema_name = reference.reference.as_str();
            let target = schema_index.get(schema_name).ok_or_else(|| {
                KernelError::Manifest(format!(
                    "schema '{schema_name}' referenced by {context} not found"
                ))
            })?;
            expand_cap_schema(target, schema_index, schema_name)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aos_air_types::{
        CapGrant, Manifest, ManifestDefaults, SchemaRef, TypeExpr, TypePrimitive, TypeRecord,
        TypeRef, TypeSet, ValueLiteral, ValueRecord, ValueSet, ValueText,
    };
    use indexmap::IndexMap;

    fn text_literal(text: &str) -> ValueLiteral {
        ValueLiteral::Text(ValueText { text: text.into() })
    }

    fn hosts_schema() -> TypeExpr {
        TypeExpr::Record(TypeRecord {
            record: IndexMap::from([(
                "hosts".into(),
                TypeExpr::Set(TypeSet {
                    set: Box::new(TypeExpr::Primitive(TypePrimitive::Text(
                        aos_air_types::TypePrimitiveText {
                            text: aos_air_types::EmptyObject {},
                        },
                    ))),
                }),
            )]),
        })
    }

    fn manifest_with_grant(params: ValueLiteral) -> Manifest {
        Manifest {
            air_version: aos_air_types::CURRENT_AIR_VERSION.to_string(),
            schemas: vec![],
            modules: vec![],
            plans: vec![],
            effects: vec![],
            caps: vec![],
            policies: vec![],
            secrets: vec![],
            defaults: Some(ManifestDefaults {
                policy: None,
                cap_grants: vec![CapGrant {
                    name: "http_cap".into(),
                    cap: "sys/http.out@1".into(),
                    params,
                    expiry_ns: None,
                }],
            }),
            module_bindings: IndexMap::new(),
            routing: None,
            triggers: vec![],
        }
    }

    fn defcap() -> DefCap {
        DefCap {
            name: "sys/http.out@1".into(),
            cap_type: CapType::http_out(),
            schema: hosts_schema(),
            enforcer: aos_air_types::CapEnforcer {
                module: "sys/CapEnforceHttpOut@1".into(),
            },
        }
    }

    fn empty_schema_index() -> SchemaIndex {
        SchemaIndex::new(HashMap::new())
    }

    fn empty_effect_catalog() -> Arc<EffectCatalog> {
        Arc::new(EffectCatalog::new())
    }

    #[test]
    fn capability_params_must_match_schema() {
        let mut record = IndexMap::new();
        record.insert(
            "hosts".into(),
            ValueLiteral::Set(ValueSet {
                set: vec![text_literal("example.com")],
            }),
        );
        let manifest = manifest_with_grant(ValueLiteral::Record(ValueRecord { record }));
        let caps = HashMap::from([("sys/http.out@1".into(), defcap())]);
        assert!(
            CapabilityResolver::from_manifest(
                &manifest,
                &caps,
                &empty_schema_index(),
                empty_effect_catalog(),
            )
            .is_ok()
        );
    }

    #[test]
    fn invalid_capability_params_error() {
        let manifest = manifest_with_grant(ValueLiteral::Record(ValueRecord {
            record: IndexMap::new(),
        }));
        let caps = HashMap::from([("sys/http.out@1".into(), defcap())]);
        let err = match CapabilityResolver::from_manifest(
            &manifest,
            &caps,
            &empty_schema_index(),
            empty_effect_catalog(),
        ) {
            Ok(_) => panic!("expected validation error"),
            Err(err) => err,
        };
        assert!(matches!(
            err,
            KernelError::CapabilityParamInvalid { grant, .. } if grant == "http_cap"
        ));
    }

    #[test]
    fn capability_schema_refs_are_expanded() {
        let referenced_schema = "com.acme/GrantSchema@1";
        let ref_schema = hosts_schema();
        let cap_with_ref = DefCap {
            name: "sys/http.out@1".into(),
            cap_type: CapType::http_out(),
            schema: TypeExpr::Ref(TypeRef {
                reference: SchemaRef::new(referenced_schema).expect("schema ref"),
            }),
            enforcer: aos_air_types::CapEnforcer {
                module: "sys/CapEnforceHttpOut@1".into(),
            },
        };
        let schema_index =
            SchemaIndex::new(HashMap::from([(referenced_schema.to_string(), ref_schema)]));

        let mut record = IndexMap::new();
        record.insert(
            "hosts".into(),
            ValueLiteral::Set(ValueSet {
                set: vec![text_literal("example.com")],
            }),
        );
        let manifest = manifest_with_grant(ValueLiteral::Record(ValueRecord { record }));
        let caps = HashMap::from([("sys/http.out@1".into(), cap_with_ref.clone())]);
        assert!(
            CapabilityResolver::from_manifest(
                &manifest,
                &caps,
                &schema_index,
                empty_effect_catalog(),
            )
            .is_ok()
        );

        let invalid_manifest = manifest_with_grant(ValueLiteral::Record(ValueRecord {
            record: IndexMap::new(),
        }));
        let err = match CapabilityResolver::from_manifest(
            &invalid_manifest,
            &caps,
            &schema_index,
            empty_effect_catalog(),
        ) {
            Ok(_) => panic!("schema refs should enforce fields"),
            Err(err) => err,
        };
        assert!(matches!(
            err,
            KernelError::CapabilityParamInvalid { cap, .. } if cap == "sys/http.out@1"
        ));
    }
}
