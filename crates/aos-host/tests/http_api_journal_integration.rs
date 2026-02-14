use std::net::SocketAddr;

use aos_host::control::{JournalTail, JournalTailEntry};
use aos_host::http::{HttpState, api};
use aos_host::modes::daemon::ControlMsg;
use serde_json::json;
use tokio::sync::{broadcast, mpsc};

#[tokio::test]
async fn http_journal_tail_forwards_kind_filters() {
    let (control_tx, mut control_rx) = mpsc::channel::<ControlMsg>(8);
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let state = HttpState::new(control_tx, shutdown_tx.clone());

    let app = axum::Router::new()
        .nest("/api", api::router())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind listener");
    let addr: SocketAddr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let control = tokio::spawn(async move {
        if let Some(ControlMsg::JournalTail {
            from,
            limit,
            kinds,
            resp,
        }) = control_rx.recv().await
        {
            assert_eq!(from, 0);
            assert_eq!(limit, Some(10));
            assert_eq!(
                kinds,
                Some(vec![
                    "domain_event".to_string(),
                    "effect_receipt".to_string()
                ])
            );
            let _ = resp.send(Ok(JournalTail {
                from: 0,
                to: 2,
                entries: vec![
                    JournalTailEntry {
                        kind: "domain_event".into(),
                        seq: 1,
                        record: json!({"schema":"demo/Event@1"}),
                    },
                    JournalTailEntry {
                        kind: "effect_receipt".into(),
                        seq: 2,
                        record: json!({"status":"ok"}),
                    },
                ],
            }));
        } else {
            panic!("expected journal tail control message");
        }
    });

    let url =
        format!("http://{addr}/api/journal?from=0&limit=10&kinds=domain_event,effect_receipt");
    let response = reqwest::get(url).await.expect("http get");
    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.expect("decode json");
    assert_eq!(body["from"], 0);
    assert_eq!(body["to"], 2);
    assert_eq!(body["entries"][0]["kind"], "domain_event");
    assert_eq!(body["entries"][1]["kind"], "effect_receipt");

    control.await.expect("control task");
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn http_journal_tail_cursor_resume_has_no_duplicates() {
    let (control_tx, mut control_rx) = mpsc::channel::<ControlMsg>(8);
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let state = HttpState::new(control_tx, shutdown_tx.clone());

    let app = axum::Router::new()
        .nest("/api", api::router())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind listener");
    let addr: SocketAddr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let control = tokio::spawn(async move {
        if let Some(ControlMsg::JournalTail {
            from,
            limit,
            kinds,
            resp,
        }) = control_rx.recv().await
        {
            assert_eq!(from, 0);
            assert_eq!(limit, Some(2));
            assert_eq!(kinds, None);
            let _ = resp.send(Ok(JournalTail {
                from: 0,
                to: 4,
                entries: vec![
                    JournalTailEntry {
                        kind: "domain_event".into(),
                        seq: 0,
                        record: json!({"schema":"demo/Event@1","event_hash":"e0"}),
                    },
                    JournalTailEntry {
                        kind: "domain_event".into(),
                        seq: 1,
                        record: json!({"schema":"demo/Event@1","event_hash":"e1"}),
                    },
                ],
            }));
        } else {
            panic!("expected first journal tail control message");
        }

        if let Some(ControlMsg::JournalTail {
            from,
            limit,
            kinds,
            resp,
        }) = control_rx.recv().await
        {
            assert_eq!(from, 1);
            assert_eq!(limit, Some(10));
            assert_eq!(kinds, None);
            let _ = resp.send(Ok(JournalTail {
                from: 1,
                to: 4,
                entries: vec![
                    JournalTailEntry {
                        kind: "domain_event".into(),
                        seq: 2,
                        record: json!({"schema":"demo/Event@1","event_hash":"e2"}),
                    },
                    JournalTailEntry {
                        kind: "domain_event".into(),
                        seq: 3,
                        record: json!({"schema":"demo/Event@1","event_hash":"e3"}),
                    },
                ],
            }));
        } else {
            panic!("expected second journal tail control message");
        }
    });

    let url_page_1 = format!("http://{addr}/api/journal?from=0&limit=2");
    let response = reqwest::get(url_page_1).await.expect("http get");
    assert!(response.status().is_success());
    let body_1: serde_json::Value = response.json().await.expect("decode json page 1");
    let entries_1 = body_1
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let seqs_1: Vec<u64> = entries_1
        .iter()
        .filter_map(|entry| entry.get("seq").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(seqs_1, vec![0, 1]);
    let resume_from = *seqs_1.last().expect("first page cursor");

    let url_page_2 = format!("http://{addr}/api/journal?from={resume_from}&limit=10");
    let response = reqwest::get(url_page_2).await.expect("http get");
    assert!(response.status().is_success());
    let body_2: serde_json::Value = response.json().await.expect("decode json page 2");
    let entries_2 = body_2
        .get("entries")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let seqs_2: Vec<u64> = entries_2
        .iter()
        .filter_map(|entry| entry.get("seq").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(seqs_2, vec![2, 3]);
    assert!(
        seqs_2.iter().all(|seq| !seqs_1.contains(seq)),
        "resume from last processed cursor should not duplicate durable entries"
    );

    control.await.expect("control task");
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn http_debug_trace_forwards_query() {
    let (control_tx, mut control_rx) = mpsc::channel::<ControlMsg>(8);
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let state = HttpState::new(control_tx, shutdown_tx.clone());

    let app = axum::Router::new()
        .nest("/api", api::router())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind listener");
    let addr: SocketAddr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let control = tokio::spawn(async move {
        if let Some(ControlMsg::TraceGet {
            event_hash,
            schema,
            correlate_by,
            correlate_value,
            window_limit,
            resp,
        }) = control_rx.recv().await
        {
            assert_eq!(
                event_hash,
                Some(
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string()
                )
            );
            assert_eq!(schema, None);
            assert_eq!(correlate_by, None);
            assert_eq!(correlate_value, None);
            assert_eq!(window_limit, Some(32));
            let _ = resp.send(Ok(json!({
                "query": {"event_hash": event_hash, "window_limit": 32},
                "root_event": {"seq": 1, "record": {"event_hash": "x"}},
                "journal_window": {"from_seq": 1, "to_seq": 1, "entries": []},
                "live_wait": {},
                "terminal_state": "completed"
            })));
        } else {
            panic!("expected trace get control message");
        }
    });

    let url = format!(
        "http://{addr}/api/debug/trace?event_hash=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa&window_limit=32"
    );
    let response = reqwest::get(url).await.expect("http get");
    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.expect("decode json");
    assert_eq!(body["terminal_state"], "completed");
    assert_eq!(body["query"]["window_limit"], 32);

    control.await.expect("control task");
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn http_debug_trace_forwards_correlation_query() {
    let (control_tx, mut control_rx) = mpsc::channel::<ControlMsg>(8);
    let (shutdown_tx, _) = broadcast::channel::<()>(1);
    let state = HttpState::new(control_tx, shutdown_tx.clone());

    let app = axum::Router::new()
        .nest("/api", api::router())
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind listener");
    let addr: SocketAddr = listener.local_addr().expect("local addr");

    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let control = tokio::spawn(async move {
        if let Some(ControlMsg::TraceGet {
            event_hash,
            schema,
            correlate_by,
            correlate_value,
            window_limit,
            resp,
        }) = control_rx.recv().await
        {
            assert_eq!(event_hash, None);
            assert_eq!(schema.as_deref(), Some("demiurge/ChatEvent@1"));
            assert_eq!(correlate_by.as_deref(), Some("$value.request_id"));
            assert_eq!(correlate_value, Some(json!(42)));
            assert_eq!(window_limit, Some(64));
            let _ = resp.send(Ok(json!({
                "query": {
                    "schema": schema,
                    "correlate_by": correlate_by,
                    "value": correlate_value,
                    "window_limit": 64
                },
                "root_event": {"seq": 1, "record": {"event_hash": "x"}},
                "journal_window": {"from_seq": 1, "to_seq": 1, "entries": []},
                "live_wait": {},
                "terminal_state": "completed"
            })));
        } else {
            panic!("expected trace get control message");
        }
    });

    let url = format!(
        "http://{addr}/api/debug/trace?schema=demiurge/ChatEvent@1&correlate_by=$value.request_id&value=42&window_limit=64"
    );
    let response = reqwest::get(url).await.expect("http get");
    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.expect("decode json");
    assert_eq!(body["terminal_state"], "completed");
    assert_eq!(body["query"]["window_limit"], 64);
    assert_eq!(body["query"]["value"], 42);

    control.await.expect("control task");
    server.abort();
    let _ = server.await;
}
