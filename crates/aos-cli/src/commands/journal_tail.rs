//! `aos journal tail` command.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use aos_host::control::RequestEnvelope;
use clap::Args;
use serde_json::{Value, json};

use crate::opts::{Mode, WorldOpts, resolve_dirs};
use crate::output::print_success;

use super::try_control_client;

#[derive(Args, Debug)]
pub struct JournalTailArgs {
    /// Sequence to start from (inclusive)
    #[arg(long, default_value_t = 0)]
    pub from: u64,

    /// Maximum number of entries to return
    #[arg(long, default_value_t = 200)]
    pub limit: u64,

    /// Comma-separated or repeated list of journal kinds to include
    #[arg(long, value_delimiter = ',')]
    pub kinds: Vec<String>,

    /// Write full JSON result to file
    #[arg(long)]
    pub out: Option<PathBuf>,
}

pub async fn cmd_journal_tail(opts: &WorldOpts, args: &JournalTailArgs) -> Result<()> {
    let dirs = resolve_dirs(opts)?;
    let mut client = try_control_client(&dirs).await.ok_or_else(|| {
        if matches!(opts.mode, Mode::Daemon) {
            anyhow::anyhow!(
                "daemon mode requested but no control socket at {}",
                dirs.control_socket.display()
            )
        } else {
            anyhow::anyhow!(
                "journal tail requires a running daemon; no control socket at {}",
                dirs.control_socket.display()
            )
        }
    })?;

    let req = RequestEnvelope {
        v: 1,
        id: "cli-journal-tail".into(),
        cmd: "journal-list".into(),
        payload: json!({
            "from": args.from,
            "limit": args.limit,
            "kinds": args.kinds,
        }),
    };
    let resp = client.request(&req).await?;
    if !resp.ok {
        anyhow::bail!("journal-list failed: {:?}", resp.error);
    }
    let result = resp.result.unwrap_or_else(|| json!({}));

    if let Some(path) = &args.out {
        fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    }

    if opts.json || opts.pretty {
        return print_success(opts, result, None, vec![]);
    }

    render_journal_tail_human(&result);
    Ok(())
}

fn render_journal_tail_human(result: &Value) {
    let from = result.get("from").and_then(|v| v.as_u64()).unwrap_or(0);
    let to = result.get("to").and_then(|v| v.as_u64()).unwrap_or(from);
    println!("journal tail: seq {from}..{to}");

    let entries = result
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if entries.is_empty() {
        println!("(no entries)");
        return;
    }

    for entry in entries {
        let seq = entry.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
        let kind = entry
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let record = entry.get("record").cloned().unwrap_or(Value::Null);
        println!("#{seq} {kind} {}", summarize_record(kind, &record));
    }
}

fn summarize_record(kind: &str, record: &Value) -> String {
    match kind {
        "domain_event" => {
            let schema = find_str(record, "schema").unwrap_or("?");
            let event_hash = find_str(record, "event_hash").unwrap_or("?");
            format!("schema={schema} event_hash={event_hash}")
        }
        "effect_intent" => {
            let intent = find_str(record, "intent_hash").unwrap_or("?");
            let effect_kind = find_str(record, "kind").unwrap_or("?");
            format!("intent={intent} kind={effect_kind}")
        }
        "effect_receipt" => {
            let intent = find_str(record, "intent_hash").unwrap_or("?");
            let status = find_str(record, "status").unwrap_or("?");
            format!("intent={intent} status={status}")
        }
        "cap_decision" | "policy_decision" => {
            let intent = find_str(record, "intent_hash").unwrap_or("?");
            let decision = find_str(record, "decision").unwrap_or("?");
            format!("intent={intent} decision={decision}")
        }
        "plan_ended" => {
            let plan = find_str(record, "plan_id").unwrap_or("?");
            let status = find_str(record, "status").unwrap_or("?");
            format!("plan_id={plan} status={status}")
        }
        _ => String::new(),
    }
}

fn find_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    match value {
        Value::Object(map) => {
            if let Some(v) = map.get(key).and_then(Value::as_str) {
                return Some(v);
            }
            for v in map.values() {
                if let Some(found) = find_str(v, key) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => {
            for item in items {
                if let Some(found) = find_str(item, key) {
                    return Some(found);
                }
            }
            None
        }
        _ => None,
    }
}
