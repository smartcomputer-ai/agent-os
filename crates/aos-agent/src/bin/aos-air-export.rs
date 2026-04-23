fn main() {
    print!(
        "{}",
        aos_wasm_sdk::air_exports_json_strings(&aos_agent::aos_air_nodes())
    );
}
