use crate::{
    EmptyObject, HashRef, ValueList, ValueLiteral, ValueMap, ValueMapEntry, ValueNat, ValueNull,
    ValueRecord, ValueText, ValueVariant,
    builtins::{builtin_schemas, find_builtin_schema},
    schema_index::{SchemaIndex, validate_literal},
    validate_value_literal,
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

fn variant(tag: &str, value: ValueLiteral) -> ValueLiteral {
    ValueLiteral::Variant(ValueVariant {
        tag: tag.to_string(),
        value: Some(Box::new(value)),
    })
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
    ]);
    validate_value_literal(&literal, &schema.schema.ty).expect("literal matches schema");
}

#[test]
fn llm_generate_params_literal_matches_builtin_schema() {
    let schema = find_builtin_schema("sys/LlmGenerateParams@1").expect("llm params schema");
    let schemas = SchemaIndex::new(
        builtin_schemas()
            .iter()
            .map(|schema| (schema.schema.name.clone(), schema.schema.ty.clone()))
            .collect(),
    );
    let literal = record(vec![
        ("provider", text("openai")),
        ("model", text("gpt-5.2")),
        (
            "window_items",
            list(vec![record(vec![
                ("item_id", text("msg-1")),
                ("kind", variant("MessageRef", null())),
                (
                    "ref_",
                    hash("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                ),
                ("lane", null()),
                ("source_range", null()),
                ("source_refs", list(vec![])),
                ("provider_compatibility", null()),
                ("estimated_tokens", null()),
                ("metadata", map(vec![])),
            ])]),
        ),
        (
            "runtime",
            record(vec![
                (
                    "temperature",
                    ValueLiteral::Dec128(crate::ValueDec128 {
                        dec128: "0.2".into(),
                    }),
                ),
                ("top_p", null()),
                ("max_tokens", nat(256)),
                ("tool_refs", null()),
                ("tool_choice", null()),
                ("reasoning_effort", null()),
                ("stop_sequences", null()),
                ("metadata", null()),
                ("provider_options_ref", null()),
                ("response_format_ref", null()),
            ]),
        ),
        ("api_key", null()),
    ]);
    validate_literal(&literal, &schema.schema.ty, &schema.schema.name, &schemas)
        .expect("literal matches schema");
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
            record(vec![
                ("prompt", nat(128)),
                ("completion", nat(64)),
                ("total", nat(192)),
            ]),
        ),
        (
            "finish_reason",
            record(vec![("reason", text("stop")), ("raw", null())]),
        ),
        ("provider_response_id", null()),
        ("usage_details", null()),
        ("warnings_ref", null()),
        ("rate_limit_ref", null()),
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
        ("provider_response_id", null()),
        (
            "finish_reason",
            record(vec![("reason", text("stop")), ("raw", null())]),
        ),
        ("token_usage", record(vec![("prompt", nat(1))])),
        ("usage_details", null()),
        ("warnings_ref", null()),
        ("rate_limit_ref", null()),
        ("cost_cents", nat(0)),
        ("provider_id", text("openai")),
    ]);
    assert!(validate_value_literal(&literal, &schema.schema.ty).is_err());
}

#[test]
fn llm_compact_params_and_receipt_literals_match_builtin_schemas() {
    let schemas = SchemaIndex::new(
        builtin_schemas()
            .iter()
            .map(|schema| (schema.schema.name.clone(), schema.schema.ty.clone()))
            .collect(),
    );
    let message_ref =
        hash("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let summary_ref =
        hash("sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    let source_range = record(vec![("start_seq", nat(0)), ("end_seq", nat(1))]);
    let source_item = record(vec![
        ("item_id", text("ledger:0")),
        ("kind", variant("MessageRef", null())),
        ("ref_", message_ref.clone()),
        ("lane", null()),
        ("source_range", source_range.clone()),
        ("source_refs", list(vec![message_ref])),
        ("provider_compatibility", null()),
        ("estimated_tokens", nat(128)),
        ("metadata", map(vec![])),
    ]);
    let summary_item = record(vec![
        ("item_id", text("compact:ctx-op-1:summary")),
        ("kind", variant("AosSummaryRef", null())),
        ("ref_", summary_ref.clone()),
        ("lane", text("Summary")),
        ("source_range", source_range.clone()),
        ("source_refs", list(vec![summary_ref.clone()])),
        ("provider_compatibility", null()),
        ("estimated_tokens", nat(32)),
        ("metadata", map(vec![])),
    ]);

    let params_schema =
        find_builtin_schema("sys/LlmCompactParams@1").expect("llm compact params schema");
    let params_literal = record(vec![
        ("correlation_id", null()),
        ("operation_id", text("ctx-op-1")),
        ("provider", text("openai")),
        ("model", text("gpt-5.2")),
        ("strategy", variant("AosSummary", null())),
        ("source_window_items", list(vec![source_item])),
        ("preserve_window_items", list(vec![])),
        ("recent_tail_items", list(vec![])),
        ("source_range", source_range.clone()),
        ("target_tokens", nat(1024)),
        ("provider_options_ref", null()),
        ("api_key", null()),
    ]);
    validate_literal(
        &params_literal,
        &params_schema.schema.ty,
        &params_schema.schema.name,
        &schemas,
    )
    .expect("params literal matches schema");

    let receipt_schema =
        find_builtin_schema("sys/LlmCompactReceipt@1").expect("llm compact receipt schema");
    let receipt_literal = record(vec![
        ("operation_id", text("ctx-op-1")),
        ("artifact_kind", variant("AosSummary", null())),
        ("artifact_refs", list(vec![summary_ref])),
        ("source_range", source_range),
        ("compacted_through", nat(1)),
        ("active_window_items", list(vec![summary_item])),
        (
            "token_usage",
            record(vec![
                ("prompt", nat(128)),
                ("completion", nat(32)),
                ("total", nat(160)),
            ]),
        ),
        ("provider_metadata_ref", null()),
        ("warnings_ref", null()),
        ("provider_id", text("llm.mock")),
    ]);
    validate_literal(
        &receipt_literal,
        &receipt_schema.schema.ty,
        &receipt_schema.schema.name,
        &schemas,
    )
    .expect("receipt literal matches schema");
}

#[test]
fn llm_count_tokens_params_and_receipt_literals_match_builtin_schemas() {
    let schemas = SchemaIndex::new(
        builtin_schemas()
            .iter()
            .map(|schema| (schema.schema.name.clone(), schema.schema.ty.clone()))
            .collect(),
    );
    let message_ref =
        hash("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let window_item = record(vec![
        ("item_id", text("ledger:0")),
        ("kind", variant("MessageRef", null())),
        ("ref_", message_ref.clone()),
        ("lane", null()),
        ("source_range", null()),
        ("source_refs", list(vec![message_ref.clone()])),
        ("provider_compatibility", null()),
        ("estimated_tokens", nat(128)),
        ("metadata", map(vec![])),
    ]);

    let params_schema =
        find_builtin_schema("sys/LlmCountTokensParams@1").expect("llm count params schema");
    let params_literal = record(vec![
        ("correlation_id", null()),
        ("provider", text("openai")),
        ("model", text("gpt-5.2")),
        ("window_items", list(vec![window_item])),
        ("tool_definitions_ref", null()),
        ("response_format_ref", null()),
        ("provider_options_ref", null()),
        ("rendering_profile", null()),
        ("candidate_plan_id", text("plan-1")),
        ("metadata", map(vec![])),
        ("api_key", null()),
    ]);
    validate_literal(
        &params_literal,
        &params_schema.schema.ty,
        &params_schema.schema.name,
        &schemas,
    )
    .expect("params literal matches schema");

    let receipt_schema =
        find_builtin_schema("sys/LlmCountTokensReceipt@1").expect("llm count receipt schema");
    let receipt_literal = record(vec![
        ("input_tokens", nat(128)),
        ("original_input_tokens", nat(128)),
        (
            "counts_by_ref",
            list(vec![record(vec![
                ("ref_", message_ref),
                ("tokens", nat(128)),
                ("quality", variant("LocalEstimate", null())),
            ])]),
        ),
        ("tool_tokens", null()),
        ("response_format_tokens", null()),
        ("quality", variant("LocalEstimate", null())),
        ("provider", text("openai")),
        ("model", text("gpt-5.2")),
        ("candidate_plan_id", text("plan-1")),
        ("provider_metadata_ref", null()),
        ("warnings_ref", null()),
    ]);
    validate_literal(
        &receipt_literal,
        &receipt_schema.schema.ty,
        &receipt_schema.schema.name,
        &schemas,
    )
    .expect("receipt literal matches schema");
}

#[test]
fn host_target_accepts_local_and_sandbox_variants() {
    let schema = find_builtin_schema("sys/HostTarget@1").expect("host target schema");
    let schemas = SchemaIndex::new(
        builtin_schemas()
            .iter()
            .map(|schema| (schema.schema.name.clone(), schema.schema.ty.clone()))
            .collect(),
    );
    let local = variant(
        "local",
        record(vec![
            ("mounts", null()),
            ("workdir", null()),
            ("env", null()),
            ("network_mode", text("none")),
        ]),
    );
    validate_literal(&local, &schema.schema.ty, &schema.schema.name, &schemas)
        .expect("local target matches schema");

    let sandbox = variant(
        "sandbox",
        record(vec![
            ("image", text("docker.io/library/alpine:latest")),
            ("runtime_class", null()),
            ("workdir", null()),
            ("env", null()),
            ("network_mode", text("egress")),
            ("mounts", null()),
            ("cpu_limit_millis", null()),
            ("memory_limit_bytes", null()),
        ]),
    );
    validate_literal(&sandbox, &schema.schema.ty, &schema.schema.name, &schemas)
        .expect("sandbox target matches schema");
}
