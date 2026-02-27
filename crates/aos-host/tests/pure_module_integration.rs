#[path = "helpers.rs"]
mod helpers;

use std::sync::Arc;

use aos_kernel::Kernel;
use aos_kernel::journal::mem::MemJournal;
use aos_wasm_abi::{ABI_VERSION, PureContext, PureInput, PureOutput};
use helpers::fixtures::{self, TestStore};

#[tokio::test]
async fn pure_module_integration_flow() {
    let store: Arc<TestStore> = fixtures::new_mem_store();

    let output_payload = serde_cbor::to_vec(&serde_json::json!({ "value": "ok" })).unwrap();
    let pure_output = PureOutput {
        output: output_payload,
    };

    let module = fixtures::stub_pure_module(
        &store,
        "com.acme/Pure@1",
        &pure_output,
        "com.acme/PureInput@1",
        "com.acme/PureOutput@1",
    );

    let mut manifest = fixtures::build_loaded_manifest(vec![module], vec![]);
    fixtures::insert_test_schemas(
        &mut manifest,
        vec![
            fixtures::def_text_record_schema(
                "com.acme/PureInput@1",
                vec![("value", fixtures::text_type())],
            ),
            fixtures::def_text_record_schema(
                "com.acme/PureOutput@1",
                vec![("value", fixtures::text_type())],
            ),
        ],
    );

    let mut kernel =
        Kernel::from_loaded_manifest(store.clone(), manifest, Box::new(MemJournal::new())).unwrap();

    let input_payload = serde_cbor::to_vec(&serde_json::json!({ "value": "hi" })).unwrap();
    let ctx = PureContext {
        logical_now_ns: 10,
        journal_height: 1,
        manifest_hash: "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            .into(),
        module: "com.acme/Pure@1".into(),
    };
    let ctx_bytes = serde_cbor::to_vec(&ctx).unwrap();
    let input = PureInput {
        version: ABI_VERSION,
        input: input_payload,
        ctx: Some(ctx_bytes),
    };

    let output = kernel.invoke_pure("com.acme/Pure@1", &input).unwrap();
    let decoded: serde_json::Value = serde_cbor::from_slice(&output.output).unwrap();
    assert_eq!(decoded["value"], "ok");
}
