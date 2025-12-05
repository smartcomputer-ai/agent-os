use anyhow::Result;
use aos_host::control::{ControlClient, RequestEnvelope, ResponseEnvelope};
use serde_json::Value;

pub async fn send_req(
    client: &mut ControlClient,
    cmd: &str,
    payload: Value,
) -> Result<ResponseEnvelope> {
    let env = RequestEnvelope {
        v: 1,
        id: format!("gov-{cmd}"),
        cmd: cmd.into(),
        payload,
    };
    let resp = client.request(&env).await?;
    if !resp.ok {
        let msg = resp
            .error
            .as_ref()
            .map(|e| format!("{}: {}", e.code, e.message))
            .unwrap_or_else(|| "unknown error".into());
        anyhow::bail!("control {} failed: {}", cmd, msg);
    }
    Ok(resp)
}
