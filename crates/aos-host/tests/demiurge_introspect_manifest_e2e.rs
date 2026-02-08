#![cfg(feature = "test-fixtures")]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use aos_air_types::HashRef;
use aos_cbor::{Hash, to_canonical_cbor};
use aos_effects::builtins::{LlmGenerateReceipt, TokenUsage};
use aos_effects::{EffectIntent, EffectReceipt, ReceiptStatus};
use aos_host::adapters::traits::AsyncEffectAdapter;
use aos_host::config::HostConfig;
use aos_host::fixtures;
use aos_host::host::WorldHost;
use aos_host::manifest_loader;
use aos_host::testhost::TestHost;
use aos_kernel::{Kernel, KernelConfig, cap_enforcer::CapCheckOutput};
use aos_store::{FsStore, Store};
use aos_wasm_abi::PureOutput;
use aos_wasm_build::builder::{BuildRequest, Builder};
use camino::Utf8PathBuf;
use serde::Deserialize;
use serde_json::json;

fn load_world_env(world_root: &Path) -> Result<()> {
    let env_path = world_root.join(".env");
    if env_path.exists() {
        for item in dotenvy::from_path_iter(&env_path).context("load .env")? {
            let (key, val) = item?;
            if std::env::var_os(&key).is_none() {
                unsafe {
                    std::env::set_var(&key, &val);
                }
            }
        }
    }
    Ok(())
}
const DEMIURGE_REDUCER: &str = "demiurge/Demiurge@1";
const TOOL_CALL_ID: &str = "call_1";

struct ToolLlmAdapter<S: aos_store::Store> {
    store: Arc<S>,
    call_count: Arc<AtomicUsize>,
}

impl<S: aos_store::Store> ToolLlmAdapter<S> {
    fn new(store: Arc<S>, call_count: Arc<AtomicUsize>) -> Self {
        Self { store, call_count }
    }
}

#[async_trait::async_trait]
impl<S: aos_store::Store + Send + Sync + 'static> AsyncEffectAdapter for ToolLlmAdapter<S> {
    fn kind(&self) -> &str {
        aos_effects::EffectKind::LLM_GENERATE
    }

    async fn execute(&self, intent: &EffectIntent) -> anyhow::Result<EffectReceipt> {
        let call = self.call_count.fetch_add(1, Ordering::SeqCst);
        let output_value = if call == 0 {
            json!([
                {
                    "type": "function_call",
                    "name": "introspect_manifest",
                    "arguments": "{\"consistency\":\"head\"}",
                    "call_id": TOOL_CALL_ID
                }
            ])
        } else {
            json!([
                {
                    "type": "message",
                    "role": "assistant",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "manifest received"
                        }
                    ]
                }
            ])
        };

        let output_bytes = serde_json::to_vec(&output_value)?;
        let output_hash = self.store.put_blob(&output_bytes)?;
        let output_ref = HashRef::new(output_hash.to_hex())?;

        let receipt = LlmGenerateReceipt {
            output_ref,
            token_usage: TokenUsage {
                prompt: 5,
                completion: 5,
            },
            cost_cents: None,
            provider_id: "mock".into(),
        };

        Ok(EffectReceipt {
            intent_hash: intent.intent_hash,
            adapter_id: "llm.mock".into(),
            status: ReceiptStatus::Ok,
            payload_cbor: serde_cbor::to_vec(&receipt)?,
            cost_cents: Some(0),
            signature: vec![0; 64],
        })
    }
}

#[derive(Debug, Deserialize)]
struct ChatState {
    messages: Vec<ChatMessage>,
}

#[derive(Debug, Deserialize)]
struct ChatMessage {
    role: ChatRole,
    message_ref: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "$tag", content = "$value")]
enum ChatRole {
    User,
    Assistant,
}

#[tokio::test(flavor = "current_thread")]
async fn demiurge_introspect_manifest_roundtrip() -> Result<()> {
    let tmp = tempfile::tempdir().context("tempdir")?;
    let store = Arc::new(FsStore::open(tmp.path()).context("open store")?);

    let asset_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/demiurge");
    load_world_env(&asset_root).context("load demiurge .env")?;
    let asset_root = asset_root.as_path();
    let mut loaded = manifest_loader::load_from_assets(store.clone(), asset_root)
        .context("load demiurge assets")?
        .context("missing demiurge manifest")?;
    let _manifest_bytes =
        aos_cbor::to_canonical_cbor(&loaded.manifest).context("encode manifest bytes")?;

    let reducer_root = asset_root.join("reducer");
    let reducer_dir =
        Utf8PathBuf::from_path_buf(reducer_root.to_path_buf()).expect("utf8 reducer path");
    let mut request = BuildRequest::new(reducer_dir);
    request.config.release = false;
    let artifact = Builder::compile(request).context("compile demiurge reducer")?;
    let wasm_hash = store
        .put_blob(&artifact.wasm_bytes)
        .context("store reducer wasm")?;

    let module = loaded
        .modules
        .get_mut(DEMIURGE_REDUCER)
        .expect("demiurge module");
    module.wasm_hash = HashRef::new(wasm_hash.to_hex()).expect("wasm hash ref");

    let allow_output = PureOutput {
        output: serde_cbor::to_vec(&CapCheckOutput {
            constraints_ok: true,
            deny: None,
        })
        .expect("encode cap output"),
    };
    let llm_enforcer = fixtures::stub_pure_module(
        &store,
        "sys/CapEnforceLlmBasic@1",
        &allow_output,
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );
    let workspace_enforcer = fixtures::stub_pure_module(
        &store,
        "sys/CapEnforceWorkspace@1",
        &allow_output,
        "sys/CapCheckInput@1",
        "sys/CapCheckOutput@1",
    );
    loaded
        .modules
        .insert(llm_enforcer.name.clone(), llm_enforcer);
    loaded
        .modules
        .insert(workspace_enforcer.name.clone(), workspace_enforcer);

    let kernel_config = KernelConfig {
        allow_placeholder_secrets: true,
        ..KernelConfig::default()
    };
    let kernel = Kernel::from_loaded_manifest_with_config(
        store.clone(),
        loaded,
        Box::new(aos_kernel::journal::mem::MemJournal::new()),
        kernel_config,
    )
    .context("build kernel")?;
    let world = WorldHost::from_kernel(kernel, store.clone(), HostConfig::default());
    let mut host = TestHost::from_world_host(world);

    let call_count = Arc::new(AtomicUsize::new(0));
    host.register_adapter(Box::new(ToolLlmAdapter::new(
        store.clone(),
        call_count.clone(),
    )));

    let tool_bytes = std::fs::read(asset_root.join("tools").join("introspect.manifest.json"))
        .context("read tool file")?;
    let tool_hash = store
        .put_blob(&tool_bytes)
        .context("store tool blob")?
        .to_hex();

    host.send_event(
        "demiurge/ChatEvent@1",
        json!({
            "$tag": "ChatCreated",
            "$value": {
                "chat_id": "chat-1",
                "title": "Test",
                "created_at_ms": 1
            }
        }),
    )?;

    let message = json!({
        "role": "user",
        "content": [
            { "type": "text", "text": "hi" }
        ]
    });
    let message_bytes = serde_json::to_vec(&message).context("encode message")?;
    let message_hash = store.put_blob(&message_bytes).context("store message")?;

    host.send_event(
        "demiurge/ChatEvent@1",
        json!({
            "$tag": "UserMessage",
            "$value": {
                "chat_id": "chat-1",
                "request_id": 1,
                "text": "hi",
                "message_ref": message_hash.to_hex(),
                "model": "gpt-mock",
                "provider": "mock",
                "max_tokens": 64,
                "tool_refs": [tool_hash],
                "tool_choice": { "$tag": "Auto" }
            }
        }),
    )?;

    for _ in 0..10 {
        let outcome = host.run_cycle_batch().await?;
        if outcome.effects_dispatched == 0 && outcome.receipts_applied == 0 {
            break;
        }
    }

    let key = to_canonical_cbor(&"chat-1").context("encode chat key")?;
    let state_bytes = host
        .kernel()
        .reducer_state_bytes(DEMIURGE_REDUCER, Some(&key))
        .context("load reducer state")?
        .ok_or_else(|| anyhow::anyhow!("missing reducer state"))?;
    let state: ChatState = serde_cbor::from_slice(&state_bytes).context("decode reducer state")?;
    assert!(state.messages.len() >= 3, "expected tool flow messages");
    assert!(matches!(state.messages[0].role, ChatRole::User));
    assert!(matches!(state.messages[1].role, ChatRole::Assistant));
    assert!(matches!(state.messages[2].role, ChatRole::Assistant));

    let tool_message_ref = state.messages[1]
        .message_ref
        .clone()
        .expect("tool message ref");
    let tool_message_hash = Hash::from_hex_str(&tool_message_ref)?;
    let tool_message_bytes = store
        .get_blob(tool_message_hash)
        .context("load tool message blob")?;
    let tool_message_value: serde_json::Value =
        serde_json::from_slice(&tool_message_bytes).context("decode tool message")?;
    let call_id = tool_message_value
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("call_id"))
        .and_then(|value| value.as_str())
        .unwrap_or("");
    assert_eq!(call_id, TOOL_CALL_ID);

    assert!(
        call_count.load(Ordering::SeqCst) >= 2,
        "expected llm adapter to be called twice"
    );

    Ok(())
}
