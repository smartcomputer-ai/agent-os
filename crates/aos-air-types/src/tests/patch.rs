use serde_json::json;

use crate::schemas::PATCH;
use super::assert_json_schema;

#[test]
fn patch_schema_accepts_minimal_add_def() {
    let doc = json!({
        "base_manifest_hash": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        "patches": [
            {
                "add_def": {
                    "kind": "defschema",
                    "node": {
                        "$kind": "defschema",
                        "name": "demo/Foo@1",
                        "type": { "bool": {} }
                    }
                }
            }
        ]
    });
    assert_json_schema(PATCH, &doc);
}
