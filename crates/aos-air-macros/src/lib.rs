use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Fields, LitStr, PathArguments, Type, parse_macro_input,
    spanned::Spanned,
};

#[proc_macro_derive(AirSchema, attributes(aos))]
pub fn derive_air_schema(input: TokenStream) -> TokenStream {
    match derive_air_schema_impl(parse_macro_input!(input as DeriveInput)) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn derive_air_schema_impl(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let schema_name = parse_schema_name(&input.attrs)?
        .ok_or_else(|| syn::Error::new(input.ident.span(), "missing #[aos(schema = \"...\")]"))?;
    let fields = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => fields,
            other => {
                return Err(syn::Error::new(
                    other.span(),
                    "AirSchema only supports structs with named fields in this phase",
                ));
            }
        },
        _ => {
            return Err(syn::Error::new(
                input.ident.span(),
                "AirSchema only supports structs in this phase",
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
        let overrides = parse_field_overrides(&field.attrs)?;
        let ty_json = match overrides {
            FieldOverride::SchemaRef(reference) => schema_ref_json(&reference),
            FieldOverride::Primitive(kind) => primitive_type_json(&kind).ok_or_else(|| {
                syn::Error::new(
                    field.span(),
                    format!("unsupported AIR primitive override '{kind}'"),
                )
            })?,
            FieldOverride::None => type_json(&field.ty)?,
        };
        field_entries.push((name, ty_json));
    }

    let schema_json = defschema_json(&schema_name, &field_entries);
    let ident = input.ident;
    let schema_lit = LitStr::new(&schema_json, ident.span());

    Ok(quote! {
        impl ::aos_wasm_sdk::AirSchemaExport for #ident {
            const AIR_SCHEMA_JSON: &'static str = #schema_lit;
        }
    })
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

#[derive(Debug)]
enum FieldOverride {
    None,
    SchemaRef(String),
    Primitive(String),
}

fn parse_field_overrides(attrs: &[Attribute]) -> syn::Result<FieldOverride> {
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

fn type_json(ty: &Type) -> syn::Result<String> {
    let Type::Path(path) = ty else {
        return Err(syn::Error::new(ty.span(), "unsupported AIR field type"));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(syn::Error::new(ty.span(), "unsupported AIR field type"));
    };
    let ident = segment.ident.to_string();
    match ident.as_str() {
        "String" => Ok(primitive_json("text")),
        "bool" => Ok(primitive_json("bool")),
        "u64" => Ok(primitive_json("nat")),
        "i64" => Ok(primitive_json("int")),
        "Vec" => {
            let inner = single_generic_type(&segment.arguments, ty.span(), "Vec")?;
            Ok(format!(r#"{{"list":{}}}"#, type_json(inner)?))
        }
        "Option" => {
            let inner = single_generic_type(&segment.arguments, ty.span(), "Option")?;
            Ok(format!(r#"{{"option":{}}}"#, type_json(inner)?))
        }
        "BTreeMap" => {
            let (key, value) = two_generic_types(&segment.arguments, ty.span(), "BTreeMap")?;
            if !is_string_type(key) {
                return Err(syn::Error::new(
                    key.span(),
                    "AIR only supports BTreeMap<String, T> in this phase",
                ));
            }
            Ok(format!(
                r#"{{"map":{{"key":{{"text":{{}}}},"value":{}}}}}"#,
                type_json(value)?
            ))
        }
        _ => Err(syn::Error::new(
            ty.span(),
            format!(
                "unsupported AIR field type '{ident}'; add #[aos(schema_ref = \"...\")] or #[aos(air_type = \"...\")]"
            ),
        )),
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

fn defschema_json(schema_name: &str, fields: &[(String, String)]) -> String {
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
