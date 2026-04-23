use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, Lit, LitStr, Meta, PathArguments, Token, Type,
    TypePath, Variant, parse_macro_input, punctuated::Punctuated, spanned::Spanned,
};

#[proc_macro_derive(AirSchema, attributes(aos))]
pub fn derive_air_schema(input: TokenStream) -> TokenStream {
    match derive_air_schema_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_derive(AirType, attributes(aos))]
pub fn derive_air_type(input: TokenStream) -> TokenStream {
    match derive_air_type_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn aos_workflow(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr with Punctuated::<Meta, Token![,]>::parse_terminated);
    match aos_workflow_impl(args, parse_macro_input!(item as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn aos_workflow_impl(
    args: Punctuated<Meta, Token![,]>,
    input: DeriveInput,
) -> syn::Result<proc_macro2::TokenStream> {
    let config = WorkflowConfig::parse(args)?;
    let ident = input.ident.clone();
    let module_json = defmodule_json(&config.module);
    let workflow_json = defworkflow_json(&config).unwrap_or_default();
    let workflow_json_expr = defworkflow_json_expr(&config, ident.span());
    let module_name_lit = LitStr::new(&config.module, ident.span());
    let workflow_name_lit = LitStr::new(&config.name, ident.span());
    let module_lit = LitStr::new(&module_json, ident.span());
    let workflow_lit = LitStr::new(&workflow_json, ident.span());

    Ok(quote! {
        #input

        impl ::aos_wasm_sdk::AirWorkflowExport for #ident {
            const AIR_MODULE_NAME: &'static str = #module_name_lit;
            const AIR_WORKFLOW_NAME: &'static str = #workflow_name_lit;
            const AIR_MODULE_JSON: &'static str = #module_lit;
            const AIR_WORKFLOW_JSON: &'static str = #workflow_lit;

            fn air_workflow_json() -> ::aos_wasm_sdk::__aos_export::String {
                #workflow_json_expr
            }
        }
    })
}

fn derive_air_schema_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let attrs = parse_type_attrs(&input.attrs, "AirSchema")?;
    let schema_name = attrs
        .schema
        .ok_or_else(|| syn::Error::new(input.ident.span(), "missing #[aos(schema = \"...\")]"))?;
    let generated_type = generated_air_type(&input, attrs.override_value)?;
    let generated_schema =
        generated_defschema_type(&schema_name, generated_type.clone(), input.ident.span());
    let ident = input.ident;
    let schema_name_lit = LitStr::new(&schema_name, ident.span());
    let schema_lit = LitStr::new(
        generated_schema.static_json.as_deref().unwrap_or(""),
        ident.span(),
    );
    let schema_expr = generated_schema.expr;
    let type_expr = generated_type.expr;

    Ok(quote! {
        impl ::aos_wasm_sdk::AirType for #ident {
            fn air_type_json() -> ::aos_wasm_sdk::__aos_export::String {
                #type_expr
            }
        }

        impl ::aos_wasm_sdk::AirSchemaRef for #ident {
            const AIR_SCHEMA_NAME: &'static str = #schema_name_lit;
        }

        impl ::aos_wasm_sdk::AirSchemaExport for #ident {
            const AIR_SCHEMA_JSON: &'static str = #schema_lit;

            fn air_schema_json() -> ::aos_wasm_sdk::__aos_export::String {
                #schema_expr
            }
        }
    })
}

fn derive_air_type_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let attrs = parse_type_attrs(&input.attrs, "AirType")?;
    if attrs.schema.is_some() {
        return Err(syn::Error::new(
            input.ident.span(),
            "AirType emits anonymous AIR types; use AirSchema for named schemas",
        ));
    }
    let generated_type = generated_air_type(&input, attrs.override_value)?;
    let ident = input.ident;
    let type_expr = generated_type.expr;

    Ok(quote! {
        impl ::aos_wasm_sdk::AirType for #ident {
            fn air_type_json() -> ::aos_wasm_sdk::__aos_export::String {
                #type_expr
            }
        }
    })
}

fn generated_air_type(
    input: &DeriveInput,
    override_value: Option<FieldOverride>,
) -> syn::Result<GeneratedJson> {
    match &input.data {
        Data::Struct(data) => {
            if let Some(override_value) = override_value {
                type_override_json(override_value, input.ident.span())?.ok_or_else(|| {
                    syn::Error::new(input.ident.span(), "expected AIR type override")
                })
            } else {
                match &data.fields {
                    Fields::Named(fields) => {
                        let field_entries = named_field_entries(&fields.named)?;
                        Ok(generated_record_type(&field_entries, input.ident.span()))
                    }
                    Fields::Unit => Ok(generated_static_json(
                        primitive_json("unit"),
                        input.ident.span(),
                    )),
                    Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                        let field = fields.unnamed.first().expect("one unnamed field");
                        type_json_with_attrs(&field.ty, parse_field_attrs(&field.attrs)?)
                    }
                    other => Err(syn::Error::new(
                        other.span(),
                        "AirType supports named structs, unit structs, or one-field tuple structs",
                    )),
                }
            }
        }
        Data::Enum(data) => {
            let mut variant_entries = Vec::new();
            for variant in &data.variants {
                variant_entries.push(variant_json_entry(variant)?);
            }
            Ok(generated_variant_type(&variant_entries, input.ident.span()))
        }
        _ => Err(syn::Error::new(
            input.ident.span(),
            "AirType only supports structs and enums in this phase",
        )),
    }
}

#[derive(Default)]
struct WorkflowConfig {
    name: String,
    module: String,
    state: Option<WorkflowSchemaRef>,
    event: Option<WorkflowSchemaRef>,
    context: Option<WorkflowSchemaRef>,
    key_schema: Option<WorkflowSchemaRef>,
    effects: Vec<WorkflowEffectRef>,
    entrypoint: String,
}

impl WorkflowConfig {
    fn parse(args: Punctuated<Meta, Token![,]>) -> syn::Result<Self> {
        let mut config = WorkflowConfig {
            entrypoint: "step".into(),
            ..WorkflowConfig::default()
        };
        for meta in args {
            let Meta::NameValue(name_value) = meta else {
                return Err(syn::Error::new(meta.span(), "expected key = value"));
            };
            let Some(key) = name_value.path.get_ident().map(|ident| ident.to_string()) else {
                return Err(syn::Error::new(
                    name_value.path.span(),
                    "expected simple key",
                ));
            };
            match key.as_str() {
                "name" => config.name = expr_string(&name_value.value, key.as_str())?,
                "module" => config.module = expr_string(&name_value.value, key.as_str())?,
                "state" => config.state = Some(expr_schema_ref(&name_value.value, key.as_str())?),
                "event" => config.event = Some(expr_schema_ref(&name_value.value, key.as_str())?),
                "context" => {
                    config.context = Some(expr_schema_ref(&name_value.value, key.as_str())?)
                }
                "key_schema" => {
                    config.key_schema = Some(expr_schema_ref(&name_value.value, key.as_str())?)
                }
                "entrypoint" => config.entrypoint = expr_string(&name_value.value, key.as_str())?,
                "effects" => config.effects = expr_effect_array(&name_value.value, key.as_str())?,
                _ => {
                    return Err(syn::Error::new(
                        name_value.path.span(),
                        format!("unsupported aos_workflow option '{key}'"),
                    ));
                }
            }
        }
        config.require("name", &config.name)?;
        config.require("module", &config.module)?;
        if config.state.is_none() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "missing required aos_workflow option 'state'",
            ));
        }
        if config.event.is_none() {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "missing required aos_workflow option 'event'",
            ));
        }
        Ok(config)
    }

    fn require(&self, key: &str, value: &str) -> syn::Result<()> {
        if value.trim().is_empty() {
            Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("missing required aos_workflow option '{key}'"),
            ))
        } else {
            Ok(())
        }
    }
}

fn expr_string(expr: &Expr, key: &str) -> syn::Result<String> {
    let Expr::Lit(expr_lit) = expr else {
        return Err(syn::Error::new(
            expr.span(),
            format!("aos_workflow option '{key}' must be a string literal"),
        ));
    };
    let Lit::Str(value) = &expr_lit.lit else {
        return Err(syn::Error::new(
            expr.span(),
            format!("aos_workflow option '{key}' must be a string literal"),
        ));
    };
    Ok(value.value())
}

#[derive(Clone)]
enum WorkflowSchemaRef {
    Literal(String),
    Type(Type),
}

#[derive(Clone)]
enum WorkflowEffectRef {
    Literal(String),
    Type(Type),
}

impl WorkflowSchemaRef {
    fn literal(&self) -> Option<&str> {
        match self {
            Self::Literal(value) => Some(value),
            Self::Type(_) => None,
        }
    }
}

impl WorkflowEffectRef {
    fn literal(&self) -> Option<&str> {
        match self {
            Self::Literal(value) => Some(value),
            Self::Type(_) => None,
        }
    }
}

fn expr_schema_ref(expr: &Expr, key: &str) -> syn::Result<WorkflowSchemaRef> {
    if let Some(value) = expr_literal_string(expr) {
        return Ok(WorkflowSchemaRef::Literal(value));
    }
    expr_type_path(expr, key).map(WorkflowSchemaRef::Type)
}

fn expr_effect_ref(expr: &Expr, key: &str) -> syn::Result<WorkflowEffectRef> {
    if let Some(value) = expr_literal_string(expr) {
        return Ok(WorkflowEffectRef::Literal(value));
    }
    expr_type_path(expr, key).map(WorkflowEffectRef::Type)
}

fn expr_literal_string(expr: &Expr) -> Option<String> {
    let Expr::Lit(expr_lit) = expr else {
        return None;
    };
    let Lit::Str(value) = &expr_lit.lit else {
        return None;
    };
    Some(value.value())
}

fn expr_type_path(expr: &Expr, key: &str) -> syn::Result<Type> {
    let Expr::Path(expr_path) = expr else {
        return Err(syn::Error::new(
            expr.span(),
            format!("aos_workflow option '{key}' must be a string literal or type path"),
        ));
    };
    if expr_path.qself.is_some() {
        return Err(syn::Error::new(
            expr.span(),
            format!("aos_workflow option '{key}' does not support qualified self paths"),
        ));
    }
    Ok(Type::Path(TypePath {
        qself: None,
        path: expr_path.path.clone(),
    }))
}

fn expr_effect_array(expr: &Expr, key: &str) -> syn::Result<Vec<WorkflowEffectRef>> {
    let Expr::Array(array) = expr else {
        return Err(syn::Error::new(
            expr.span(),
            format!(
                "aos_workflow option '{key}' must be an array of string literals or type paths"
            ),
        ));
    };
    let mut values = Vec::new();
    for elem in &array.elems {
        values.push(expr_effect_ref(elem, key)?);
    }
    Ok(values)
}

struct AosTypeAttrs {
    schema: Option<String>,
    override_value: Option<FieldOverride>,
}

fn parse_type_attrs(attrs: &[Attribute], owner: &str) -> syn::Result<AosTypeAttrs> {
    let mut parsed = AosTypeAttrs {
        schema: None,
        override_value: None,
    };
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("aos")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("schema") {
                let value: LitStr = meta.value()?.parse()?;
                parsed.schema = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("schema_ref") {
                ensure_no_override_option(&parsed.override_value, &meta)?;
                parsed.override_value = Some(parse_schema_ref_override(&meta)?);
                Ok(())
            } else if meta.path.is_ident("air_type") {
                ensure_no_override_option(&parsed.override_value, &meta)?;
                let value: LitStr = meta.value()?.parse()?;
                parsed.override_value = Some(FieldOverride::Primitive(value.value()));
                Ok(())
            } else if meta.path.is_ident("type_json") {
                ensure_no_override_option(&parsed.override_value, &meta)?;
                let value: LitStr = meta.value()?.parse()?;
                parsed.override_value = Some(FieldOverride::RawJson(value.value()));
                Ok(())
            } else if meta.path.is_ident("inline") || meta.path.is_ident("map_key_air_type") {
                Err(meta.error(format!(
                    "supported #[aos(...)] options for {owner} are schema, schema_ref, air_type, and type_json"
                )))
            } else {
                Err(meta.error(format!(
                    "supported #[aos(...)] options for {owner} are schema, schema_ref, air_type, and type_json"
                )))
            }
        })?;
    }
    Ok(parsed)
}

#[derive(Clone)]
enum FieldOverride {
    None,
    SchemaRefLiteral(String),
    SchemaRefType(Type),
    Primitive(String),
    RawJson(String),
}

#[derive(Clone)]
struct FieldAttrs {
    override_value: FieldOverride,
    map_key_air_type: Option<String>,
    inline: bool,
}

impl Default for FieldAttrs {
    fn default() -> Self {
        Self {
            override_value: FieldOverride::None,
            map_key_air_type: None,
            inline: false,
        }
    }
}

fn parse_schema_ref_override(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<FieldOverride> {
    let value = meta.value()?;
    if value.peek(LitStr) {
        let value: LitStr = value.parse()?;
        return Ok(FieldOverride::SchemaRefLiteral(value.value()));
    }
    let ty: Type = value.parse()?;
    ensure_type_path(&ty, "schema_ref")?;
    Ok(FieldOverride::SchemaRefType(ty))
}

#[derive(Clone)]
struct GeneratedJson {
    static_json: Option<String>,
    expr: proc_macro2::TokenStream,
}

fn parse_field_attrs(attrs: &[Attribute]) -> syn::Result<FieldAttrs> {
    let mut parsed = FieldAttrs::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("aos")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("schema_ref") {
                ensure_no_override(&parsed.override_value, &meta)?;
                ensure_not_inline(parsed.inline, &meta)?;
                parsed.override_value = parse_schema_ref_override(&meta)?;
                Ok(())
            } else if meta.path.is_ident("air_type") {
                ensure_no_override(&parsed.override_value, &meta)?;
                ensure_not_inline(parsed.inline, &meta)?;
                let value: LitStr = meta.value()?.parse()?;
                parsed.override_value = FieldOverride::Primitive(value.value());
                Ok(())
            } else if meta.path.is_ident("type_json") {
                ensure_no_override(&parsed.override_value, &meta)?;
                ensure_not_inline(parsed.inline, &meta)?;
                let value: LitStr = meta.value()?.parse()?;
                parsed.override_value = FieldOverride::RawJson(value.value());
                Ok(())
            } else if meta.path.is_ident("map_key_air_type") {
                if parsed.map_key_air_type.is_some() {
                    return Err(meta.error("field already has an AIR map key override"));
                }
                let value: LitStr = meta.value()?.parse()?;
                parsed.map_key_air_type = Some(value.value());
                Ok(())
            } else if meta.path.is_ident("inline") {
                ensure_no_override(&parsed.override_value, &meta)?;
                if parsed.map_key_air_type.is_some() {
                    return Err(meta.error("inline fields cannot define a map key override"));
                }
                parsed.inline = true;
                Ok(())
            } else {
                Err(meta.error(
                    "supported field options are schema_ref, air_type, type_json, map_key_air_type, and inline",
                ))
            }
        })?;
    }
    Ok(parsed)
}

fn variant_json_entry(variant: &Variant) -> syn::Result<(String, GeneratedJson)> {
    let attrs = parse_field_attrs(&variant.attrs)?;
    if attrs.inline || attrs.map_key_air_type.is_some() {
        return Err(syn::Error::new(
            variant.span(),
            "AirSchema enum variants do not support inline or map_key_air_type",
        ));
    }
    let override_value = attrs.override_value;
    let name = variant.ident.to_string();
    let ty_json = if let Some(override_json) =
        type_override_json(override_value.clone(), variant.span())?
    {
        match &variant.fields {
            Fields::Unit => override_json,
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field = fields.unnamed.first().expect("one unnamed field");
                wrap_generated_json_for_outer_type(&field.ty, override_json)
            }
            _ => override_json,
        }
    } else {
        match &variant.fields {
            Fields::Unit => generated_static_json(primitive_json("unit"), variant.span()),
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                let field = fields.unnamed.first().expect("one unnamed field");
                type_json_with_attrs(&field.ty, parse_field_attrs(&field.attrs)?)?
            }
            Fields::Named(fields) => {
                generated_record_type(&named_field_entries(&fields.named)?, variant.span())
            }
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "AirSchema enum variants support unit, one-field tuple, or named-field variants",
                ));
            }
        }
    };
    Ok((name, ty_json))
}

fn named_field_entries(
    fields: &Punctuated<syn::Field, Token![,]>,
) -> syn::Result<Vec<(String, GeneratedJson)>> {
    let mut field_entries = Vec::new();
    for field in fields {
        let ident = field
            .ident
            .as_ref()
            .ok_or_else(|| syn::Error::new(field.span(), "expected named field"))?;
        let name = ident.to_string();
        let attrs = parse_field_attrs(&field.attrs)?;
        let ty_json = type_json_with_attrs(&field.ty, attrs)?;
        field_entries.push((name, ty_json));
    }
    Ok(field_entries)
}

fn ensure_no_override(
    current: &FieldOverride,
    meta: &syn::meta::ParseNestedMeta<'_>,
) -> syn::Result<()> {
    if matches!(current, FieldOverride::None) {
        Ok(())
    } else {
        Err(meta.error("field already has an AIR type override"))
    }
}

fn ensure_no_override_option(
    current: &Option<FieldOverride>,
    meta: &syn::meta::ParseNestedMeta<'_>,
) -> syn::Result<()> {
    if current.is_none() {
        Ok(())
    } else {
        Err(meta.error("field already has an AIR type override"))
    }
}

fn ensure_not_inline(inline: bool, meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<()> {
    if inline {
        Err(meta.error("inline fields cannot define an AIR type override"))
    } else {
        Ok(())
    }
}

fn ensure_type_path(ty: &Type, key: &str) -> syn::Result<()> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new(
            ty.span(),
            format!("AIR option '{key}' must be a string literal or type path"),
        ));
    };
    if path.qself.is_some() {
        return Err(syn::Error::new(
            ty.span(),
            format!("AIR option '{key}' does not support qualified self paths"),
        ));
    }
    Ok(())
}

fn type_json(ty: &Type) -> syn::Result<GeneratedJson> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new(ty.span(), "unsupported AIR field type"));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(syn::Error::new(ty.span(), "unsupported AIR field type"));
    };
    let ident = segment.ident.to_string();
    match ident.as_str() {
        "String" => Ok(generated_static_json(primitive_json("text"), ty.span())),
        "bool" => Ok(generated_static_json(primitive_json("bool"), ty.span())),
        "u64" => Ok(generated_static_json(primitive_json("nat"), ty.span())),
        "i64" => Ok(generated_static_json(primitive_json("int"), ty.span())),
        "Vec" => {
            let inner = single_generic_type(&segment.arguments, ty.span(), "Vec")?;
            Ok(wrap_generated_json("list", type_json(inner)?, ty.span()))
        }
        "Option" => {
            let inner = single_generic_type(&segment.arguments, ty.span(), "Option")?;
            Ok(wrap_generated_json("option", type_json(inner)?, ty.span()))
        }
        "BTreeMap" => {
            let (key, value) = two_generic_types(&segment.arguments, ty.span(), "BTreeMap")?;
            let key_json = map_key_json(key, None)?;
            Ok(map_generated_json(key_json, type_json(value)?, ty.span()))
        }
        _ => Ok(generated_schema_ref_for_type(ty)),
    }
}

fn type_json_with_attrs(ty: &Type, attrs: FieldAttrs) -> syn::Result<GeneratedJson> {
    if attrs.inline {
        return Ok(generated_inline_type_for_type(ty));
    }
    if attrs.map_key_air_type.is_some() || is_btree_map_type(ty) {
        return map_type_json_with_attrs(ty, attrs);
    }
    let Some(base) = type_override_json(attrs.override_value, ty.span())? else {
        return type_json(ty);
    };
    Ok(wrap_generated_json_for_outer_type(ty, base))
}

fn map_type_json_with_attrs(ty: &Type, attrs: FieldAttrs) -> syn::Result<GeneratedJson> {
    if is_option_or_vec_type(ty) {
        return Err(syn::Error::new(
            ty.span(),
            "map_key_air_type cannot be applied through Option<T> or Vec<T>",
        ));
    }
    let map_parts = btree_map_types(ty)?;
    let key_json = if let Some(key_air_type) = attrs.map_key_air_type.as_deref() {
        map_key_primitive_json(key_air_type).ok_or_else(|| {
            syn::Error::new(
                ty.span(),
                format!("unsupported AIR map key primitive override '{key_air_type}'"),
            )
        })?
    } else if let Some((key, _)) = map_parts {
        map_key_json(key, None)?
    } else {
        primitive_json("text")
    };

    let value_json = if let Some((_, value)) = map_parts {
        let Some(base) = type_override_json(attrs.override_value, value.span())? else {
            return Ok(map_generated_json(key_json, type_json(value)?, ty.span()));
        };
        wrap_generated_json_for_outer_type(value, base)
    } else {
        type_override_json(attrs.override_value, ty.span())?.ok_or_else(|| {
            syn::Error::new(
                ty.span(),
                "map_key_air_type on non-BTreeMap fields requires schema_ref, air_type, or type_json",
            )
        })?
    };
    Ok(map_generated_json(key_json, value_json, ty.span()))
}

fn type_override_json(
    override_value: FieldOverride,
    span: proc_macro2::Span,
) -> syn::Result<Option<GeneratedJson>> {
    match override_value {
        FieldOverride::SchemaRefLiteral(reference) => Ok(Some(generated_static_json(
            schema_ref_json(&reference),
            span,
        ))),
        FieldOverride::SchemaRefType(ty) => Ok(Some(generated_schema_ref_for_type(&ty))),
        FieldOverride::Primitive(kind) => Ok(Some(generated_static_json(
            primitive_type_json(&kind).ok_or_else(|| {
                syn::Error::new(span, format!("unsupported AIR primitive override '{kind}'"))
            })?,
            span,
        ))),
        FieldOverride::RawJson(json) => Ok(Some(generated_static_json(json, span))),
        FieldOverride::None => Ok(None),
    }
}

fn wrap_generated_json_for_outer_type(ty: &Type, base: GeneratedJson) -> GeneratedJson {
    let Type::Path(path) = ty else {
        return base;
    };
    let Some(segment) = path.path.segments.last() else {
        return base;
    };
    match segment.ident.to_string().as_str() {
        "Option" => {
            let Ok(inner) = single_generic_type(&segment.arguments, ty.span(), "Option") else {
                return wrap_generated_json("option", base, ty.span());
            };
            wrap_generated_json(
                "option",
                wrap_generated_json_for_outer_type(inner, base),
                ty.span(),
            )
        }
        "Vec" => {
            let Ok(inner) = single_generic_type(&segment.arguments, ty.span(), "Vec") else {
                return wrap_generated_json("list", base, ty.span());
            };
            wrap_generated_json(
                "list",
                wrap_generated_json_for_outer_type(inner, base),
                ty.span(),
            )
        }
        _ => base,
    }
}

fn generated_static_json(json: String, span: proc_macro2::Span) -> GeneratedJson {
    let lit = LitStr::new(&json, span);
    GeneratedJson {
        static_json: Some(json),
        expr: quote! {
            ::aos_wasm_sdk::__aos_export::String::from(#lit)
        },
    }
}

fn generated_schema_ref_for_type(ty: &Type) -> GeneratedJson {
    let ty_tokens = quote! { #ty };
    GeneratedJson {
        static_json: None,
        expr: quote! {{
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(r#"{"ref":"#);
            ::aos_wasm_sdk::push_air_json_string(
                &mut out,
                <#ty_tokens as ::aos_wasm_sdk::AirSchemaRef>::AIR_SCHEMA_NAME,
            );
            out.push('}');
            out
        }},
    }
}

fn generated_inline_type_for_type(ty: &Type) -> GeneratedJson {
    let ty_tokens = quote! { #ty };
    GeneratedJson {
        static_json: None,
        expr: quote! {
            <#ty_tokens as ::aos_wasm_sdk::AirType>::air_type_json()
        },
    }
}

fn wrap_generated_json(kind: &str, inner: GeneratedJson, span: proc_macro2::Span) -> GeneratedJson {
    let static_json = inner
        .static_json
        .as_ref()
        .map(|inner_json| format!(r#"{{"{kind}":{inner_json}}}"#));
    let prefix = LitStr::new(&format!(r#"{{"{kind}":"#), span);
    let inner_expr = inner.expr;
    GeneratedJson {
        static_json,
        expr: quote! {{
            let inner = #inner_expr;
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(#prefix);
            out.push_str(&inner);
            out.push('}');
            out
        }},
    }
}

fn map_generated_json(
    key_json: String,
    value: GeneratedJson,
    span: proc_macro2::Span,
) -> GeneratedJson {
    let static_json = value
        .static_json
        .as_ref()
        .map(|value_json| format!(r#"{{"map":{{"key":{key_json},"value":{value_json}}}}}"#));
    let value_expr = value.expr;
    let prefix = LitStr::new(&format!(r#"{{"map":{{"key":{key_json},"value":"#), span);
    GeneratedJson {
        static_json,
        expr: quote! {{
            let value = #value_expr;
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(#prefix);
            out.push_str(&value);
            out.push_str("}}");
            out
        }},
    }
}

fn generated_record_type(
    fields: &[(String, GeneratedJson)],
    span: proc_macro2::Span,
) -> GeneratedJson {
    let static_fields: Option<Vec<(String, String)>> = fields
        .iter()
        .map(|(field, ty)| {
            ty.static_json
                .as_ref()
                .map(|json| (field.clone(), json.clone()))
        })
        .collect();
    let static_json = static_fields
        .as_ref()
        .map(|fields| record_type_json(fields));

    let field_chunks = fields.iter().enumerate().map(|(idx, (field, ty))| {
        let comma = idx > 0;
        let field_lit = LitStr::new(field, span);
        let ty_expr = ty.expr.clone();
        quote! {
            if #comma {
                out.push(',');
            }
            ::aos_wasm_sdk::push_air_json_string(&mut out, #field_lit);
            out.push(':');
            let field_ty = #ty_expr;
            out.push_str(&field_ty);
        }
    });

    GeneratedJson {
        static_json,
        expr: quote! {{
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(r#"{"record":{"#);
            #(#field_chunks)*
            out.push_str("}}");
            out
        }},
    }
}

fn generated_defschema_type(
    schema_name: &str,
    ty: GeneratedJson,
    span: proc_macro2::Span,
) -> GeneratedJson {
    let static_json = ty
        .static_json
        .as_ref()
        .map(|ty_json| defschema_type_json(schema_name, ty_json));
    let schema_name_lit = LitStr::new(schema_name, span);
    let ty_expr = ty.expr;
    GeneratedJson {
        static_json,
        expr: quote! {{
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(r#"{"$kind":"defschema","name":"#);
            ::aos_wasm_sdk::push_air_json_string(&mut out, #schema_name_lit);
            out.push_str(r#","type":"#);
            let ty = #ty_expr;
            out.push_str(&ty);
            out.push('}');
            out
        }},
    }
}

fn generated_variant_type(
    variants: &[(String, GeneratedJson)],
    span: proc_macro2::Span,
) -> GeneratedJson {
    let static_variants: Option<Vec<(String, String)>> = variants
        .iter()
        .map(|(variant, ty)| {
            ty.static_json
                .as_ref()
                .map(|json| (variant.clone(), json.clone()))
        })
        .collect();
    let static_json = static_variants
        .as_ref()
        .map(|variants| variant_type_json(variants));

    let variant_chunks = variants.iter().enumerate().map(|(idx, (variant, ty))| {
        let comma = idx > 0;
        let variant_lit = LitStr::new(variant, span);
        let ty_expr = ty.expr.clone();
        quote! {
            if #comma {
                out.push(',');
            }
            ::aos_wasm_sdk::push_air_json_string(&mut out, #variant_lit);
            out.push(':');
            let variant_ty = #ty_expr;
            out.push_str(&variant_ty);
        }
    });

    GeneratedJson {
        static_json,
        expr: quote! {{
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(r#"{"variant":{"#);
            #(#variant_chunks)*
            out.push_str("}}");
            out
        }},
    }
}

fn single_generic_type<'a>(
    args: &'a PathArguments,
    span: proc_macro2::Span,
    outer: &str,
) -> syn::Result<&'a Type> {
    let PathArguments::AngleBracketed(args) = args else {
        return Err(syn::Error::new(
            span,
            format!("{outer}<T> requires one generic argument"),
        ));
    };
    let mut types = args.args.iter().filter_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });
    let Some(first) = types.next() else {
        return Err(syn::Error::new(
            span,
            format!("{outer}<T> requires one generic argument"),
        ));
    };
    if types.next().is_some() {
        return Err(syn::Error::new(
            span,
            format!("{outer}<T> requires exactly one generic argument"),
        ));
    }
    Ok(first)
}

fn two_generic_types<'a>(
    args: &'a PathArguments,
    span: proc_macro2::Span,
    outer: &str,
) -> syn::Result<(&'a Type, &'a Type)> {
    let PathArguments::AngleBracketed(args) = args else {
        return Err(syn::Error::new(
            span,
            format!("{outer}<K, V> requires two generic arguments"),
        ));
    };
    let mut types = args.args.iter().filter_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });
    let Some(first) = types.next() else {
        return Err(syn::Error::new(
            span,
            format!("{outer}<K, V> requires two generic arguments"),
        ));
    };
    let Some(second) = types.next() else {
        return Err(syn::Error::new(
            span,
            format!("{outer}<K, V> requires two generic arguments"),
        ));
    };
    if types.next().is_some() {
        return Err(syn::Error::new(
            span,
            format!("{outer}<K, V> requires exactly two generic arguments"),
        ));
    }
    Ok((first, second))
}

fn is_btree_map_type(ty: &Type) -> bool {
    path_last_ident(ty).is_some_and(|ident| ident == "BTreeMap")
}

fn is_option_or_vec_type(ty: &Type) -> bool {
    path_last_ident(ty).is_some_and(|ident| ident == "Option" || ident == "Vec")
}

fn btree_map_types(ty: &Type) -> syn::Result<Option<(&Type, &Type)>> {
    let Type::Path(path) = ty else {
        return Ok(None);
    };
    let Some(segment) = path.path.segments.last() else {
        return Ok(None);
    };
    if segment.ident != "BTreeMap" {
        return Ok(None);
    }
    two_generic_types(&segment.arguments, ty.span(), "BTreeMap").map(Some)
}

fn path_last_ident(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

fn map_key_json(ty: &Type, override_kind: Option<&str>) -> syn::Result<String> {
    if let Some(kind) = override_kind {
        return map_key_primitive_json(kind).ok_or_else(|| {
            syn::Error::new(
                ty.span(),
                format!("unsupported AIR map key primitive override '{kind}'"),
            )
        });
    }
    let Some(ident) = path_last_ident(ty) else {
        return Err(syn::Error::new(ty.span(), "unsupported AIR map key type"));
    };
    match ident.as_str() {
        "String" => Ok(primitive_json("text")),
        "u64" => Ok(primitive_json("nat")),
        "i64" => Ok(primitive_json("int")),
        other => Err(syn::Error::new(
            ty.span(),
            format!(
                "unsupported AIR map key type '{other}'; use map_key_air_type for text, nat, int, uuid, or hash keys"
            ),
        )),
    }
}

fn map_key_primitive_json(kind: &str) -> Option<String> {
    match kind {
        "int" | "nat" | "text" | "uuid" | "hash" => Some(primitive_json(kind)),
        _ => None,
    }
}

fn record_type_json(fields: &[(String, String)]) -> String {
    let mut out = String::from(r#"{"record":{"#);
    for (idx, (field, ty)) in fields.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&json_string(field));
        out.push(':');
        out.push_str(ty);
    }
    out.push_str("}}}");
    out
}

fn defschema_type_json(schema_name: &str, ty: &str) -> String {
    let name = json_string(schema_name);
    format!(r#"{{"$kind":"defschema","name":{name},"type":{ty}}}"#)
}

fn variant_type_json(variants: &[(String, String)]) -> String {
    let mut out = String::from(r#"{"variant":{"#);
    for (idx, (variant, ty)) in variants.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&json_string(variant));
        out.push(':');
        out.push_str(ty);
    }
    out.push_str("}}");
    out
}

fn defmodule_json(module_name: &str) -> String {
    format!(
        r#"{{"$kind":"defmodule","name":{},"runtime":{{"kind":"wasm","artifact":{{"kind":"wasm_module"}}}}}}"#,
        json_string(module_name)
    )
}

fn defworkflow_json(config: &WorkflowConfig) -> Option<String> {
    let state = config.state.as_ref()?.literal()?;
    let event = config.event.as_ref()?.literal()?;
    let context = match &config.context {
        Some(context) => Some(context.literal()?),
        None => None,
    };
    let key_schema = match &config.key_schema {
        Some(key_schema) => Some(key_schema.literal()?),
        None => None,
    };
    let effects: Option<Vec<String>> = config
        .effects
        .iter()
        .map(|effect| effect.literal().map(ToOwned::to_owned))
        .collect();
    let effects = effects?;
    let mut out = format!(
        r#"{{"$kind":"defworkflow","name":{},"state":{},"event":{}"#,
        json_string(&config.name),
        json_string(state),
        json_string(event)
    );
    if let Some(context) = context {
        out.push_str(r#","context":"#);
        out.push_str(&json_string(context));
    }
    if let Some(key_schema) = key_schema {
        out.push_str(r#","key_schema":"#);
        out.push_str(&json_string(key_schema));
    }
    out.push_str(r#","effects_emitted":"#);
    out.push_str(&json_string_array(&effects));
    out.push_str(r#","impl":{"module":"#);
    out.push_str(&json_string(&config.module));
    out.push_str(r#","entrypoint":"#);
    out.push_str(&json_string(&config.entrypoint));
    out.push_str("}}");
    Some(out)
}

fn defworkflow_json_expr(
    config: &WorkflowConfig,
    span: proc_macro2::Span,
) -> proc_macro2::TokenStream {
    let name_lit = LitStr::new(&config.name, span);
    let module_lit = LitStr::new(&config.module, span);
    let entrypoint_lit = LitStr::new(&config.entrypoint, span);
    let state_expr = schema_ref_name_expr(config.state.as_ref().expect("state"));
    let event_expr = schema_ref_name_expr(config.event.as_ref().expect("event"));
    let context_chunk = config.context.as_ref().map(|context| {
        let context_expr = schema_ref_name_expr(context);
        quote! {
            out.push_str(r#","context":"#);
            ::aos_wasm_sdk::push_air_json_string(&mut out, #context_expr);
        }
    });
    let key_schema_chunk = config.key_schema.as_ref().map(|key_schema| {
        let key_schema_expr = schema_ref_name_expr(key_schema);
        quote! {
            out.push_str(r#","key_schema":"#);
            ::aos_wasm_sdk::push_air_json_string(&mut out, #key_schema_expr);
        }
    });
    let effect_chunks = config.effects.iter().enumerate().map(|(idx, effect)| {
        let comma = idx > 0;
        let effect_expr = effect_ref_name_expr(effect);
        quote! {
            if #comma {
                out.push(',');
            }
            ::aos_wasm_sdk::push_air_json_string(&mut out, #effect_expr);
        }
    });

    quote! {{
        let mut out = ::aos_wasm_sdk::__aos_export::String::from(
            r#"{"$kind":"defworkflow","name":"#,
        );
        ::aos_wasm_sdk::push_air_json_string(&mut out, #name_lit);
        out.push_str(r#","state":"#);
        ::aos_wasm_sdk::push_air_json_string(&mut out, #state_expr);
        out.push_str(r#","event":"#);
        ::aos_wasm_sdk::push_air_json_string(&mut out, #event_expr);
        #context_chunk
        #key_schema_chunk
        out.push_str(r#","effects_emitted":["#);
        #(#effect_chunks)*
        out.push_str(r#"],"impl":{"module":"#);
        ::aos_wasm_sdk::push_air_json_string(&mut out, #module_lit);
        out.push_str(r#","entrypoint":"#);
        ::aos_wasm_sdk::push_air_json_string(&mut out, #entrypoint_lit);
        out.push_str("}}");
        out
    }}
}

fn schema_ref_name_expr(reference: &WorkflowSchemaRef) -> proc_macro2::TokenStream {
    match reference {
        WorkflowSchemaRef::Literal(value) => {
            let lit = LitStr::new(value, proc_macro2::Span::call_site());
            quote! { #lit }
        }
        WorkflowSchemaRef::Type(ty) => {
            quote! { <#ty as ::aos_wasm_sdk::AirSchemaRef>::AIR_SCHEMA_NAME }
        }
    }
}

fn effect_ref_name_expr(reference: &WorkflowEffectRef) -> proc_macro2::TokenStream {
    match reference {
        WorkflowEffectRef::Literal(value) => {
            let lit = LitStr::new(value, proc_macro2::Span::call_site());
            quote! { #lit }
        }
        WorkflowEffectRef::Type(ty) => {
            quote! { <#ty as ::aos_wasm_sdk::AirEffectRef>::AIR_EFFECT_NAME }
        }
    }
}

fn schema_ref_json(reference: &str) -> String {
    format!(r#"{{"ref":{}}}"#, json_string(reference))
}

fn primitive_type_json(kind: &str) -> Option<String> {
    match kind {
        "bool" | "int" | "nat" | "dec128" | "bytes" | "text" | "time" | "duration" | "hash"
        | "uuid" | "unit" => Some(primitive_json(kind)),
        _ => None,
    }
}

fn primitive_json(kind: &str) -> String {
    format!(r#"{{"{kind}":{{}}}}"#)
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).expect("serialize JSON string")
}

fn json_string_array(values: &[String]) -> String {
    serde_json::to_string(values).expect("serialize JSON string array")
}
