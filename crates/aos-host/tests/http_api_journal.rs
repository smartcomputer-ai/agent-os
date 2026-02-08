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

    let app = axum::Router::new().nest("/api", api::router()).with_state(state);
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

    let url = format!(
        "http://{addr}/api/journal?from=0&limit=10&kinds=domain_event,effect_receipt"
    );
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
