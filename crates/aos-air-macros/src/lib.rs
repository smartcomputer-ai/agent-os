use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, Lit, LitStr, Meta, PathArguments, Token, Type,
    Variant, parse_macro_input, punctuated::Punctuated, spanned::Spanned,
};

#[proc_macro_derive(AirSchema, attributes(aos))]
pub fn derive_air_schema(input: TokenStream) -> TokenStream {
    match derive_air_schema_impl(parse_macro_input!(input as DeriveInput)) {
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
    let workflow_json = defworkflow_json(&config);
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
        }
    })
}

fn derive_air_schema_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let schema_name = parse_schema_name(&input.attrs)?
        .ok_or_else(|| syn::Error::new(input.ident.span(), "missing #[aos(schema = \"...\")]"))?;
    let generated_schema = match &input.data {
        Data::Struct(data) => {
            let fields = match &data.fields {
                Fields::Named(fields) => fields,
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "AirSchema only supports structs with named fields in this phase",
                    ));
                }
            };

            let mut field_entries = Vec::new();
            for field in &fields.named {
                let ident = field
                    .ident
                    .as_ref()
                    .ok_or_else(|| syn::Error::new(field.span(), "expected named field"))?;
                let name = ident.to_string();
                let override_value = parse_type_override(&field.attrs)?;
                let ty_json = type_json_with_override(&field.ty, override_value)?;
                field_entries.push((name, ty_json));
            }

            generated_defschema_record(&schema_name, &field_entries, input.ident.span())
        }
        Data::Enum(data) => {
            let mut variant_entries = Vec::new();
            for variant in &data.variants {
                variant_entries.push(variant_json_entry(variant)?);
            }
            generated_defschema_variant(&schema_name, &variant_entries, input.ident.span())
        }
        _ => {
            return Err(syn::Error::new(
                input.ident.span(),
                "AirSchema only supports structs and enums in this phase",
            ));
        }
    };
    let ident = input.ident;
    let schema_name_lit = LitStr::new(&schema_name, ident.span());
    let schema_lit = LitStr::new(
        generated_schema.static_json.as_deref().unwrap_or(""),
        ident.span(),
    );
    let schema_expr = generated_schema.expr;

    Ok(quote! {
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

#[derive(Debug, Default)]
struct WorkflowConfig {
    name: String,
    module: String,
    state: String,
    event: String,
    context: Option<String>,
    key_schema: Option<String>,
    effects: Vec<String>,
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
                "state" => config.state = expr_string(&name_value.value, key.as_str())?,
                "event" => config.event = expr_string(&name_value.value, key.as_str())?,
                "context" => config.context = Some(expr_string(&name_value.value, key.as_str())?),
                "key_schema" => {
                    config.key_schema = Some(expr_string(&name_value.value, key.as_str())?)
                }
                "entrypoint" => config.entrypoint = expr_string(&name_value.value, key.as_str())?,
                "effects" => config.effects = expr_string_array(&name_value.value, key.as_str())?,
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
        config.require("state", &config.state)?;
        config.require("event", &config.event)?;
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

fn expr_string_array(expr: &Expr, key: &str) -> syn::Result<Vec<String>> {
    let Expr::Array(array) = expr else {
        return Err(syn::Error::new(
            expr.span(),
            format!("aos_workflow option '{key}' must be an array of string literals"),
        ));
    };
    let mut values = Vec::new();
    for elem in &array.elems {
        values.push(expr_string(elem, key)?);
    }
    Ok(values)
}

fn parse_schema_name(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut schema = None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("aos")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("schema") {
                let value: LitStr = meta.value()?.parse()?;
                schema = Some(value.value());
                Ok(())
            } else {
                Err(meta.error("unsupported #[aos(...)] option for AirSchema"))
            }
        })?;
    }
    Ok(schema)
}

#[derive(Debug, Clone)]
enum FieldOverride {
    None,
    SchemaRef(String),
    Primitive(String),
}

struct GeneratedJson {
    static_json: Option<String>,
    expr: proc_macro2::TokenStream,
}

fn parse_type_override(attrs: &[Attribute]) -> syn::Result<FieldOverride> {
    let mut override_value = FieldOverride::None;
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("aos")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("schema_ref") {
                ensure_no_override(&override_value, &meta)?;
                let value: LitStr = meta.value()?.parse()?;
                override_value = FieldOverride::SchemaRef(value.value());
                Ok(())
            } else if meta.path.is_ident("air_type") {
                ensure_no_override(&override_value, &meta)?;
                let value: LitStr = meta.value()?.parse()?;
                override_value = FieldOverride::Primitive(value.value());
                Ok(())
            } else {
                Err(meta.error("supported field options are schema_ref and air_type"))
            }
        })?;
    }
    Ok(override_value)
}

fn variant_json_entry(variant: &Variant) -> syn::Result<(String, GeneratedJson)> {
    let override_value = parse_type_override(&variant.attrs)?;
    let name = variant.ident.to_string();
    let ty_json = match &variant.fields {
        Fields::Unit => type_override_json(override_value, variant.span())?
            .unwrap_or_else(|| generated_static_json(primitive_json("unit"), variant.span())),
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
            let field = fields.unnamed.first().expect("one unnamed field");
            type_json_with_override(&field.ty, override_value)?
        }
        other => {
            return Err(syn::Error::new(
                other.span(),
                "AirSchema enum variants support unit variants or single-field tuple variants",
            ));
        }
    };
    Ok((name, ty_json))
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
            if !is_string_type(key) {
                return Err(syn::Error::new(
                    key.span(),
                    "AIR only supports BTreeMap<String, T> in this phase",
                ));
            }
            Ok(map_generated_json(type_json(value)?, ty.span()))
        }
        _ => Ok(generated_schema_ref_for_type(ty)),
    }
}

fn type_json_with_override(ty: &Type, override_value: FieldOverride) -> syn::Result<GeneratedJson> {
    let Some(base) = type_override_json(override_value, ty.span())? else {
        return type_json(ty);
    };
    Ok(wrap_generated_json_for_outer_type(ty, base))
}

fn type_override_json(
    override_value: FieldOverride,
    span: proc_macro2::Span,
) -> syn::Result<Option<GeneratedJson>> {
    match override_value {
        FieldOverride::SchemaRef(reference) => Ok(Some(generated_static_json(
            schema_ref_json(&reference),
            span,
        ))),
        FieldOverride::Primitive(kind) => Ok(Some(generated_static_json(
            primitive_type_json(&kind).ok_or_else(|| {
                syn::Error::new(span, format!("unsupported AIR primitive override '{kind}'"))
            })?,
            span,
        ))),
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
        "Option" => wrap_generated_json("option", base, ty.span()),
        "Vec" => wrap_generated_json("list", base, ty.span()),
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

fn map_generated_json(value: GeneratedJson, span: proc_macro2::Span) -> GeneratedJson {
    let static_json = value
        .static_json
        .as_ref()
        .map(|value_json| format!(r#"{{"map":{{"key":{{"text":{{}}}},"value":{value_json}}}}}"#));
    let value_expr = value.expr;
    let prefix = LitStr::new(r#"{"map":{"key":{"text":{}},"value":"#, span);
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

fn generated_defschema_record(
    schema_name: &str,
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
        .map(|fields| defschema_record_json(schema_name, fields));

    let schema_name_lit = LitStr::new(schema_name, span);
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
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(r#"{"$kind":"defschema","name":"#);
            ::aos_wasm_sdk::push_air_json_string(&mut out, #schema_name_lit);
            out.push_str(r#","type":{"record":{"#);
            #(#field_chunks)*
            out.push_str("}}}");
            out
        }},
    }
}

fn generated_defschema_variant(
    schema_name: &str,
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
        .map(|variants| defschema_variant_json(schema_name, variants));

    let schema_name_lit = LitStr::new(schema_name, span);
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
            let mut out = ::aos_wasm_sdk::__aos_export::String::from(r#"{"$kind":"defschema","name":"#);
            ::aos_wasm_sdk::push_air_json_string(&mut out, #schema_name_lit);
            out.push_str(r#","type":{"variant":{"#);
            #(#variant_chunks)*
            out.push_str("}}}");
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

fn is_string_type(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "String"),
        _ => false,
    }
}

fn defschema_record_json(schema_name: &str, fields: &[(String, String)]) -> String {
    let name = json_string(schema_name);
    let mut out = format!(r#"{{"$kind":"defschema","name":{name},"type":{{"record":{{"#);
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

fn defschema_variant_json(schema_name: &str, variants: &[(String, String)]) -> String {
    let name = json_string(schema_name);
    let mut out = format!(r#"{{"$kind":"defschema","name":{name},"type":{{"variant":{{"#);
    for (idx, (variant, ty)) in variants.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(&json_string(variant));
        out.push(':');
        out.push_str(ty);
    }
    out.push_str("}}}");
    out
}

fn defmodule_json(module_name: &str) -> String {
    format!(
        r#"{{"$kind":"defmodule","name":{},"runtime":{{"kind":"wasm","artifact":{{"kind":"wasm_module"}}}}}}"#,
        json_string(module_name)
    )
}

fn defworkflow_json(config: &WorkflowConfig) -> String {
    let mut out = format!(
        r#"{{"$kind":"defworkflow","name":{},"state":{},"event":{}"#,
        json_string(&config.name),
        json_string(&config.state),
        json_string(&config.event)
    );
    if let Some(context) = &config.context {
        out.push_str(r#","context":"#);
        out.push_str(&json_string(context));
    }
    if let Some(key_schema) = &config.key_schema {
        out.push_str(r#","key_schema":"#);
        out.push_str(&json_string(key_schema));
    }
    out.push_str(r#","effects_emitted":"#);
    out.push_str(&json_string_array(&config.effects));
    out.push_str(r#","impl":{"module":"#);
    out.push_str(&json_string(&config.module));
    out.push_str(r#","entrypoint":"#);
    out.push_str(&json_string(&config.entrypoint));
    out.push_str("}}");
    out
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
