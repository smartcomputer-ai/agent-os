use crate::{
    EmptyObject, HashRef, ValueList, ValueLiteral, ValueMap, ValueMapEntry, ValueNat, ValueNull,
    ValueRecord, ValueText, builtins::find_builtin_schema, validate_value_literal,
};

fn text(value: &str) -> ValueLiteral {
    ValueLiteral::Text(ValueText {
        text: value.to_string(),
    })
}

fn nat(value: u64) -> ValueLiteral {
    ValueLiteral::Nat(ValueNat { nat: value })
}

fn hash(value: &str) -> ValueLiteral {
    ValueLiteral::Hash(crate::ValueHash {
        hash: HashRef::new(value.to_string()).expect("hash literal"),
    })
}

fn null() -> ValueLiteral {
    ValueLiteral::Null(ValueNull {
        null: EmptyObject::default(),
    })
}

fn record(fields: Vec<(&str, ValueLiteral)>) -> ValueLiteral {
    ValueLiteral::Record(ValueRecord {
        record: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    })
}

fn map(entries: Vec<(ValueLiteral, ValueLiteral)>) -> ValueLiteral {
    ValueLiteral::Map(ValueMap {
        map: entries
            .into_iter()
            .map(|(key, value)| ValueMapEntry { key, value })
            .collect(),
    })
}

fn list(items: Vec<ValueLiteral>) -> ValueLiteral {
    ValueLiteral::List(ValueList { list: items })
}

#[test]
fn http_request_params_literal_matches_builtin_schema() {
    let schema = find_builtin_schema("sys/HttpRequestParams@1").expect("http params schema");
    let literal = record(vec![
        ("method", text("POST")),
        ("url", text("https://example.com")),
        (
            "headers",
            map(vec![(text("content-type"), text("application/json"))]),
        ),
        (
            "body_ref",
            hash("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        ),
    ]);
    validate_value_literal(&literal, &schema.schema.ty).expect("literal matches schema");
}

#[test]
fn http_request_receipt_literal_matches_builtin_schema() {
    let schema = find_builtin_schema("sys/HttpRequestReceipt@1").expect("http receipt schema");
    let literal = record(vec![
        ("status", ValueLiteral::Int(crate::ValueInt { int: 200 })),
        ("headers", map(vec![(text("server"), text("nginx"))])),
        ("body_ref", null()),
        (
            "timings",
            record(vec![("start_ns", nat(1)), ("end_ns", nat(2))]),
        ),
        ("adapter_id", text("http-adapter")),
    ]);
    validate_value_literal(&literal, &schema.schema.ty).expect("literal matches schema");
}

#[test]
fn llm_generate_params_literal_matches_builtin_schema() {
    let schema = find_builtin_schema("sys/LlmGenerateParams@1").expect("llm params schema");
    let literal = record(vec![
        ("provider", text("openai")),
        ("model", text("gpt-5.2")),
        (
            "temperature",
            ValueLiteral::Dec128(crate::ValueDec128 {
                dec128: "0.2".into(),
            }),
        ),
        ("max_tokens", nat(256)),
        (
            "message_refs",
            list(vec![hash(
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )]),
        ),
        ("tool_refs", null()),
        ("tool_choice", null()),
        ("api_key", null()),
    ]);
    validate_value_literal(&literal, &schema.schema.ty).expect("literal matches schema");
}

#[test]
fn llm_generate_receipt_literal_matches_builtin_schema() {
    let schema = find_builtin_schema("sys/LlmGenerateReceipt@1").expect("llm receipt schema");
    let literal = record(vec![
        (
            "output_ref",
            hash("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ),
        ("raw_output_ref", null()),
        (
            "token_usage",
            record(vec![("prompt", nat(128)), ("completion", nat(64))]),
        ),
        ("cost_cents", nat(42)),
        ("provider_id", text("openai")),
    ]);
    validate_value_literal(&literal, &schema.schema.ty).expect("literal matches schema");
}

#[test]
fn llm_generate_receipt_requires_token_usage_fields() {
    let schema = find_builtin_schema("sys/LlmGenerateReceipt@1").expect("llm receipt schema");
    let literal = record(vec![
        (
            "output_ref",
            hash("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
        ),
        ("raw_output_ref", null()),
        ("token_usage", record(vec![("prompt", nat(1))])),
        ("cost_cents", nat(0)),
        ("provider_id", text("openai")),
    ]);
    assert!(validate_value_literal(&literal, &schema.schema.ty).is_err());
}
