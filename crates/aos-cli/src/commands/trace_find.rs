//! `aos trace-find` command.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use aos_host::control::RequestEnvelope;
use aos_kernel::journal::JournalRecord;
use base64::Engine as _;
use clap::Args;
use serde_json::{Value, json};

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;

use super::try_control_client;

#[derive(Args, Debug)]
pub struct TraceFindArgs {
    /// Domain event schema to search for
    #[arg(long)]
    pub schema: String,

    /// Optional correlation path (example: $value.chat_id)
    #[arg(long)]
    pub correlate_by: Option<String>,

    /// Optional correlation value (JSON literal or plain text)
    #[arg(long)]
    pub value: Option<String>,

    /// Journal sequence to start scanning from
    #[arg(long, default_value_t = 0)]
    pub from: u64,

    /// Maximum journal entries to scan
    #[arg(long, default_value_t = 1000)]
    pub limit: u64,

    /// Maximum matching roots to return
    #[arg(long, default_value_t = 20)]
    pub max_results: usize,

    /// Write JSON output to file
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub async fn cmd_trace_find(opts: &WorldOpts, args: &TraceFindArgs) -> Result<()> {
    if args.correlate_by.is_some() ^ args.value.is_some() {
        anyhow::bail!("trace-find requires both --correlate-by and --value when using correlation");
    }

    let compare_value = if let Some(raw) = &args.value {
        Some(serde_json::from_str::<Value>(raw).unwrap_or_else(|_| Value::String(raw.clone())))
    } else {
        None
    };

    let dirs = resolve_dirs(opts)?;
    let mut client = try_control_client(&dirs).await.ok_or_else(|| {
        if matches!(opts.mode, Mode::Daemon) {
            anyhow::anyhow!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            )
        } else {
            anyhow::anyhow!(
                "trace-find requires a running daemon; no control socket at {}",
                dirs.control_socket.display()
            )
        }
    })?;

    let req = RequestEnvelope {
        v: 1,
        id: "cli-trace-find".into(),
        cmd: "journal-list".into(),
        payload: json!({
            "from": args.from,
            "limit": args.limit,
            "kinds": ["domain_event"],
        }),
    };
    let resp = client.request(&req).await?;
    if !resp.ok {
        anyhow::bail!("journal-list failed: {:?}", resp.error);
    }

    let entries = resp
        .result
        .as_ref()
        .and_then(|v| v.get("entries"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut matches = Vec::new();
    for raw in entries.into_iter().rev() {
        if matches.len() >= args.max_results {
            break;
        }
        let seq = raw.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
        let record = raw.get("record").cloned().unwrap_or(Value::Null);
        let Ok(record) = serde_json::from_value::<JournalRecord>(record) else {
            continue;
        };
        let JournalRecord::DomainEvent(domain) = record else {
            continue;
        };
        if domain.schema != args.schema {
            continue;
        }
        let Ok(value_json) = serde_cbor::from_slice::<Value>(&domain.value) else {
            continue;
        };

        if let (Some(path), Some(expected)) = (args.correlate_by.as_deref(), compare_value.as_ref()) {
            let Some(actual) = json_path_get(&value_json, path) else {
                continue;
            };
            if actual != expected {
                continue;
            }
        }

        matches.push(json!({
            "seq": seq,
            "event_hash": domain.event_hash,
            "schema": domain.schema,
            "key_b64": domain.key.map(|k| base64::prelude::BASE64_STANDARD.encode(k)),
            "value": value_json,
        }));
    }

    let result = json!({
        "query": {
            "schema": args.schema,
            "correlate_by": args.correlate_by,
            "value": compare_value,
            "from": args.from,
            "limit": args.limit,
            "max_results": args.max_results,
        },
        "matches": matches,
    });

    if let Some(path) = &args.out {
        fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    }

    if opts.json || opts.pretty {
        return print_success(opts, result, None, vec![]);
    }

    render_human(&result);
    Ok(())
}

fn render_human(result: &Value) {
    let matches = result
        .get("matches")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    println!("trace-find: {} match(es)", matches.len());
    for item in matches {
        let seq = item.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
        let event_hash = item
            .get("event_hash")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let schema = item.get("schema").and_then(|v| v.as_str()).unwrap_or("?");
        println!("#{seq} {event_hash} {schema}");
    }
}

fn json_path_get<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let normalized = path.trim();
    // Preserve "$value"/"$tag" AIR union keys while still allowing "$.foo" syntax.
    let normalized = if let Some(rest) = normalized.strip_prefix("$.") {
        rest
    } else {
        normalized
    };
    if normalized.is_empty() {
        return Some(value);
    }

    let mut current = value;
    for segment in normalized.split('.') {
        if segment.is_empty() {
            continue;
        }
        let obj = current.as_object()?;
        current = obj.get(segment)?;
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::json_path_get;
    use serde_json::json;

    #[test]
    fn json_path_get_supports_air_union_fields() {
        let value = json!({
            "$tag": "UserMessage",
            "$value": { "request_id": 2 }
        });
        assert_eq!(
            json_path_get(&value, "$value.request_id"),
            Some(&json!(2))
        );
        assert_eq!(
            json_path_get(&value, "$.value.request_id"),
            None
        );
    }
}
