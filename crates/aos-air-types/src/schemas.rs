//! Embedded AIR JSON Schema documents. Source of truth lives under `spec/schemas/`.

pub const AIR_SPEC_VERSION: &str = "1.0";

macro_rules! embed_schema {
    ($($const:ident => $path:literal),+ $(,)?) => {
        $(pub const $const: &str = include_str!($path);)+

        pub const ALL: &[SchemaDoc] = &[
            $(SchemaDoc { name: stringify!($const), json: $const },)+
        ];
    };
}

#[derive(Debug, Clone, Copy)]
pub struct SchemaDoc {
    pub name: &'static str,
    pub json: &'static str,
}

embed_schema! {
    COMMON => "../../../spec/schemas/common.schema.json",
    DEFSCHEMA => "../../../spec/schemas/defschema.schema.json",
    DEFMODULE => "../../../spec/schemas/defmodule.schema.json",
    DEFCAP => "../../../spec/schemas/defcap.schema.json",
    DEFPOLICY => "../../../spec/schemas/defpolicy.schema.json",
    MANIFEST => "../../../spec/schemas/manifest.schema.json",
    PATCH => "../../../spec/schemas/patch.schema.json",
}

// Legacy schema kept for archival/tests only; not part of active schema set.
pub const DEFPLAN: &str = include_str!("../../../spec/schemas/defplan.schema.json");

pub fn find(name: &str) -> Option<&'static str> {
    ALL.iter()
        .find(|doc| doc.name.eq_ignore_ascii_case(name))
        .map(|doc| doc.json)
}
