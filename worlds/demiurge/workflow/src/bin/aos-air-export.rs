fn main() {
    print!(
        "{}",
        aos_wasm_sdk::air_exports_json_strings(&demiurge_workflow::aos_air_nodes())
    );
}
