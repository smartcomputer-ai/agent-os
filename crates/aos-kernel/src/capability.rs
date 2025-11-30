use std::{collections::HashMap, sync::Arc};

use aos_air_types::{
    CapGrant, CapGrantBudget, CapType, DefCap, Manifest, Name, TypeExpr, TypeList, TypeMap,
    TypeOption, TypePrimitive, TypeRecord, TypeSet, TypeVariant, ValueLiteral, builtins,
    catalog::EffectCatalog, plan_literals::SchemaIndex, validate_value_literal,
};
use aos_cbor::to_canonical_cbor;
use aos_effects::{CapabilityBudget, CapabilityGrant};
use indexmap::IndexMap;

use crate::error::KernelError;

pub trait CapabilityGate {
    fn resolve(&self, cap_name: &str, effect_kind: &str) -> Result<CapabilityGrant, KernelError>;
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
}

impl CapabilityResolver {
    fn new(grants: HashMap<String, ResolvedGrant>, effect_catalog: Arc<EffectCatalog>) -> Self {
        Self {
            grants,
            effect_catalog,
        }
    }

    pub fn from_runtime_grants<I>(grants: I) -> Self
    where
        I: IntoIterator<Item = (CapabilityGrant, CapType)>,
    {
        let map = grants
            .into_iter()
            .map(|(grant, cap_type)| (grant.name.clone(), ResolvedGrant { grant, cap_type }))
            .collect();
        let catalog =
            EffectCatalog::from_defs(builtins::builtin_effects().iter().map(|e| e.effect.clone()));
        Self::new(map, Arc::new(catalog))
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
        let expected = expected_cap_type(&self.effect_catalog, effect_kind)?;
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
                    budget: None,
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
