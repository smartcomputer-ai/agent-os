use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use aos_effect_types::GovDecision;
use aos_node::{CborPayload, CommandRecord, CommandStatus};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::GlobalOpts;
use crate::client::{ApiClient, ApiTarget};
use crate::config::{ConfigPaths, ProfileKind, load_config};
use crate::output::{OutputOpts, print_success};
use crate::render::decode_workspace_key_bytes;

const WORKSPACE_WORKFLOW: &str = "sys/Workspace@1";

pub(crate) fn resolve_target(global: &GlobalOpts) -> Result<ApiTarget> {
    let paths = ConfigPaths::resolve(global.config.as_deref())?;
    let config = load_config(&paths)?;
    let selected_profile = global
        .profile
        .clone()
        .or_else(|| std::env::var("AOS_FDB_PROFILE").ok())
        .or(config.current_profile.clone());
    let profile = selected_profile
        .as_ref()
        .and_then(|name| config.profiles.get(name))
        .cloned();
    let api = global
        .api
        .clone()
        .or_else(|| std::env::var("AOS_FDB_API").ok())
        .or_else(|| profile.as_ref().map(|p| p.api.clone()))
        .ok_or_else(|| anyhow!("no API endpoint configured; set --api or create a profile"))?;
    let mut headers = profile
        .as_ref()
        .map(|p| p.headers.clone())
        .unwrap_or_default();
    for (key, value) in parse_headers(&global.header)? {
        headers.insert(key, value);
    }
    let token = global
        .token
        .clone()
        .or_else(|| std::env::var("AOS_TOKEN").ok())
        .or_else(|| std::env::var("AOS_FDB_TOKEN").ok())
        .or_else(|| profile.as_ref().and_then(|p| p.token.clone()))
        .or_else(|| {
            profile
                .as_ref()
                .and_then(|p| p.token_env.as_ref())
                .and_then(|name| std::env::var(name).ok())
        });
    let kind = profile
        .as_ref()
        .map(|p| p.kind)
        .unwrap_or(ProfileKind::Remote);
    Ok(ApiTarget {
        kind,
        api,
        headers,
        token,
        verbose: global.verbose,
        world: global
            .world
            .clone()
            .or_else(|| std::env::var("AOS_FDB_WORLD").ok())
            .or_else(|| profile.as_ref().and_then(|p| p.world.clone())),
    })
}

fn parse_headers(values: &[String]) -> Result<BTreeMap<String, String>> {
    let mut headers = BTreeMap::new();
    for value in values {
        let (name, header_value) = value
            .split_once('=')
            .ok_or_else(|| anyhow!("headers must be KEY=VALUE: {value}"))?;
        headers.insert(name.to_string(), header_value.to_string());
    }
    Ok(headers)
}

fn require_world_selector(target: &ApiTarget) -> Result<&str> {
    target
        .world
        .as_deref()
        .ok_or_else(|| anyhow!("no world selected; use --world or `aos profile set --world ...`"))
}

pub(crate) fn resolve_selected_world(target: &ApiTarget) -> Result<String> {
    resolve_world_selector(require_world_selector(target)?)
}

pub(crate) fn resolve_world_arg_or_selected(
    target: &ApiTarget,
    selector: Option<&str>,
) -> Result<String> {
    match selector {
        Some(selector) => resolve_world_selector(selector),
        None => resolve_world_selector(require_world_selector(target)?),
    }
}

pub(crate) fn resolve_world_selector(selector: &str) -> Result<String> {
    Uuid::parse_str(selector)
        .map(|_| selector.to_string())
        .with_context(|| format!("world selector '{selector}' must be a world UUID"))
}

pub(crate) async fn fetch_command(
    client: &ApiClient,
    world: &str,
    command_id: &str,
) -> Result<Value> {
    let command_id = encode_path_segment(command_id);
    client
        .get_json(&format!("/v1/worlds/{world}/commands/{command_id}"), &[])
        .await
}

pub(crate) async fn wait_for_command(
    client: &ApiClient,
    world: &str,
    command_id: &str,
    interval_ms: u64,
    timeout_ms: u64,
) -> Result<Value> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let data = fetch_command(client, world, command_id).await?;
        let status = data
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if is_terminal_state(status) {
            return Ok(data);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("timed out waiting for command '{command_id}'"));
        }
        tokio::time::sleep(Duration::from_millis(interval_ms)).await;
    }
}

pub(crate) async fn decode_command_payload<T: serde::de::DeserializeOwned>(
    client: &ApiClient,
    world: &str,
    record: &Value,
) -> Result<T> {
    let record: CommandRecord =
        serde_json::from_value(record.clone()).context("decode command record")?;
    if !matches!(record.status, CommandStatus::Succeeded) {
        return Err(anyhow!(
            "command '{}' is not succeeded (status {:?})",
            record.command_id,
            record.status
        ));
    }
    let payload = record
        .result_payload
        .ok_or_else(|| anyhow!("command '{}' has no result payload", record.command_id))?;
    let bytes = load_cbor_payload(client, world, &payload).await?;
    serde_cbor::from_slice(&bytes).context("decode command result payload")
}

async fn load_cbor_payload(
    client: &ApiClient,
    world: &str,
    payload: &CborPayload,
) -> Result<Vec<u8>> {
    if let Some(bytes) = &payload.inline_cbor {
        return Ok(bytes.clone());
    }
    let blob_ref = payload
        .cbor_ref
        .as_deref()
        .ok_or_else(|| anyhow!("CBOR payload is missing both inline_cbor and cbor_ref"))?;
    client
        .get_bytes(
            &format!("/v1/cas/blobs/{blob_ref}"),
            &universe_query_for_world(client, world).await?,
        )
        .await
}

pub(crate) async fn resolve_workspace_ref(
    client: &ApiClient,
    world: &str,
    reference: &crate::workspace::WorkspaceRef,
) -> Result<Value> {
    client
        .get_json(
            &format!("/v1/worlds/{world}/workspace/resolve"),
            &[
                ("workspace", Some(reference.workspace.clone())),
                ("version", reference.version.map(|value| value.to_string())),
            ],
        )
        .await
}

pub(crate) async fn list_workspace_names(
    client: &ApiClient,
    world: &str,
    limit: Option<u64>,
) -> Result<Value> {
    let data = client
        .get_json(
            &format!(
                "/v1/worlds/{world}/state/{}/cells",
                encode_path_segment(WORKSPACE_WORKFLOW)
            ),
            &[],
        )
        .await?;
    let cells = data
        .get("cells")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let names = decode_workspace_names(cells, limit)?;
    Ok(Value::Array(names.into_iter().map(Value::String).collect()))
}

fn decode_workspace_names(cells: Vec<Value>, limit: Option<u64>) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for cell in cells {
        let key_bytes = decode_workspace_key_bytes(&cell)?;
        if let Some(name) = decode_workspace_key(&key_bytes) {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    if let Some(limit) = limit {
        if limit > 0 && names.len() > limit as usize {
            names.truncate(limit as usize);
        }
    }
    Ok(names)
}

pub(crate) fn print_workspace_cat(
    output: OutputOpts,
    bytes: &[u8],
    out: Option<&Path>,
    raw: bool,
    meta: Option<Value>,
    warnings: Vec<String>,
) -> Result<()> {
    if let Some(out) = out {
        fs::write(out, bytes).with_context(|| format!("write {}", out.display()))?;
        return print_success(
            output,
            json!({ "path": out.display().to_string(), "bytes": bytes.len() }),
            meta,
            warnings,
        );
    }

    if output.json || output.pretty {
        return print_success(
            output,
            json!({ "data_b64": BASE64_STANDARD.encode(bytes) }),
            meta,
            warnings,
        );
    }

    for warning in warnings {
        eprintln!("notice: {warning}");
    }

    if raw {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        stdout.write_all(bytes)?;
        stdout.flush()?;
        return Ok(());
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            println!("{}", serde_json::to_string_pretty(&value)?);
        } else {
            print!("{text}");
        }
        return Ok(());
    }

    if let Ok(value) = serde_cbor::from_slice::<Value>(bytes) {
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    anyhow::bail!("binary content; use --raw, --out, or --json");
}

fn decode_workspace_key(bytes: &[u8]) -> Option<String> {
    serde_cbor::from_slice::<String>(bytes).ok()
}

pub(crate) async fn universe_query_for_world(
    client: &ApiClient,
    world: &str,
) -> Result<Vec<(&'static str, Option<String>)>> {
    Ok(vec![(
        "universe_id",
        Some(universe_id_for_world(client, world).await?),
    )])
}

pub(crate) async fn universe_id_for_world(client: &ApiClient, world: &str) -> Result<String> {
    let data = client
        .get_json(&format!("/v1/worlds/{world}/runtime"), &[])
        .await?;
    Ok(data
        .get("universe_id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("world runtime response missing universe_id"))?
        .to_string())
}

pub(crate) fn encode_path_segment(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

pub(crate) fn is_terminal_state(state: &str) -> bool {
    matches!(
        state,
        "completed" | "failed" | "succeeded" | "rejected" | "applied"
    )
}

pub(crate) fn default_approver() -> String {
    std::env::var("AOS_OWNER")
        .ok()
        .and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .or_else(|| {
            std::env::var("USER").ok().and_then(|value| {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                }
            })
        })
        .unwrap_or_else(|| "aos".to_string())
}

pub(crate) fn ensure_approved(decision: GovDecision, proposal_id: u64) -> Result<()> {
    if decision != GovDecision::Approve {
        return Err(anyhow!(
            "governance approval for proposal {} did not approve",
            proposal_id
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{CliConfig, ProfileConfig, ProfileKind, save_config};
    use tempfile::TempDir;

    fn temp_config() -> (TempDir, ConfigPaths) {
        let temp = TempDir::new().expect("temp dir");
        let paths = ConfigPaths {
            path: temp.path().join("cli.json"),
        };
        (temp, paths)
    }

    #[test]
    fn decode_workspace_names_extracts_and_sorts_names() {
        let alpha = BASE64_STANDARD.encode(serde_cbor::to_vec(&"alpha").expect("encode alpha"));
        let beta = BASE64_STANDARD.encode(serde_cbor::to_vec(&"beta").expect("encode beta"));
        let names = decode_workspace_names(
            vec![
                json!({ "key_b64": beta }),
                json!({ "key_b64": alpha }),
                json!({ "key_b64": alpha }),
            ],
            None,
        )
        .expect("decode names");
        assert_eq!(names, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn decode_workspace_names_honors_limit() {
        let alpha = BASE64_STANDARD.encode(serde_cbor::to_vec(&"alpha").expect("encode alpha"));
        let beta = BASE64_STANDARD.encode(serde_cbor::to_vec(&"beta").expect("encode beta"));
        let names = decode_workspace_names(
            vec![json!({ "key_b64": beta }), json!({ "key_b64": alpha })],
            Some(1),
        )
        .expect("decode names");
        assert_eq!(names, vec!["alpha".to_string()]);
    }

    #[test]
    fn decode_workspace_names_accepts_key_bytes_shape() {
        let workflow = serde_cbor::to_vec(&"workflow").expect("encode workflow");
        let names = decode_workspace_names(vec![json!({ "key_bytes": workflow })], None)
            .expect("decode names");
        assert_eq!(names, vec!["workflow".to_string()]);
    }

    #[test]
    fn encode_path_segment_escapes_slashes() {
        assert_eq!(
            encode_path_segment("sys/Workspace@1"),
            "sys%2FWorkspace%401"
        );
    }

    #[test]
    fn resolve_target_does_not_invent_local_world() {
        let (_temp, paths) = temp_config();
        save_config(
            &paths,
            &CliConfig {
                current_profile: Some("local".into()),
                profiles: [(
                    "local".into(),
                    ProfileConfig {
                        kind: ProfileKind::Local,
                        api: "http://127.0.0.1:9010".into(),
                        token: None,
                        token_env: None,
                        headers: Default::default(),
                        universe: None,
                        world: None,
                    },
                )]
                .into_iter()
                .collect(),
                chat: Default::default(),
            },
        )
        .expect("save config");

        let global = crate::GlobalOpts {
            profile: None,
            api: None,
            token: None,
            header: Vec::new(),
            universe: None,
            world: None,
            config: Some(paths.path.clone()),
            json: false,
            pretty: false,
            quiet: false,
            no_meta: false,
            verbose: false,
        };

        let target = resolve_target(&global).expect("resolve target");
        assert_eq!(target.kind, ProfileKind::Local);
        assert_eq!(target.world, None);
    }
}
