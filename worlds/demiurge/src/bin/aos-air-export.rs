fn main() {
    print!(
        "{}",
        aos_wasm_sdk::air_exports_json_strings(&demiurge::aos_air_nodes())
    );
}
