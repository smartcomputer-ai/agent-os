use fabric_protocol::{
    ExecEvent, ExecRequest, FsApplyPatchRequest, FsApplyPatchResponse, FsEditFileRequest,
    FsEditFileResponse, FsExistsResponse, FsFileReadResponse, FsFileWriteRequest, FsGlobRequest,
    FsGlobResponse, FsGrepRequest, FsGrepResponse, FsListDirResponse, FsMkdirRequest, FsPathQuery,
    FsRemoveRequest, FsRemoveResponse, FsStatResponse, FsWriteResponse, HealthResponse,
    HostInfoResponse, HostInventoryResponse, SessionId, SessionOpenRequest, SessionOpenResponse,
    SessionStatusResponse, SignalSessionRequest,
};
use futures_util::StreamExt;

use crate::common::{
    ExecEventClientStream, FabricClientError, FabricHttpClient, decode_error_response,
    decode_exec_stream, decode_json_response,
};

#[derive(Debug, Clone)]
pub struct FabricHostClient {
    inner: FabricHttpClient,
}

impl FabricHostClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            inner: FabricHttpClient::new(base_url),
        }
    }

    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.inner = self.inner.with_bearer_token(token);
        self
    }

    pub async fn health(&self) -> Result<HealthResponse, FabricClientError> {
        self.inner.health().await
    }

    pub async fn info(&self) -> Result<HostInfoResponse, FabricClientError> {
        let response = self
            .inner
            .http
            .get(format!("{}/v1/host/info", self.inner.base_url))
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn inventory(&self) -> Result<HostInventoryResponse, FabricClientError> {
        let response = self
            .inner
            .http
            .get(format!("{}/v1/host/inventory", self.inner.base_url))
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn open_session(
        &self,
        request: &SessionOpenRequest,
    ) -> Result<SessionOpenResponse, FabricClientError> {
        let response = self
            .inner
            .http
            .post(format!("{}/v1/sessions", self.inner.base_url))
            .json(request)
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn session_status(
        &self,
        session_id: &SessionId,
    ) -> Result<SessionStatusResponse, FabricClientError> {
        let response = self
            .inner
            .http
            .get(format!(
                "{}/v1/sessions/{}",
                self.inner.base_url, session_id.0
            ))
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn exec_session(
        &self,
        request: &ExecRequest,
    ) -> Result<Vec<ExecEvent>, FabricClientError> {
        let mut stream = self.exec_session_stream(request).await?;
        let mut events = Vec::new();
        while let Some(event) = stream.next().await {
            events.push(event?);
        }
        Ok(events)
    }

    pub async fn exec_session_stream(
        &self,
        request: &ExecRequest,
    ) -> Result<ExecEventClientStream, FabricClientError> {
        let response = self
            .inner
            .http
            .post(format!(
                "{}/v1/sessions/{}/exec",
                self.inner.base_url, request.session_id.0
            ))
            .json(request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(decode_error_response(response).await);
        }

        Ok(decode_exec_stream(response))
    }

    pub async fn signal_session(
        &self,
        session_id: &SessionId,
        request: &SignalSessionRequest,
    ) -> Result<SessionStatusResponse, FabricClientError> {
        let response = self
            .inner
            .http
            .post(format!(
                "{}/v1/sessions/{}/signal",
                self.inner.base_url, session_id.0
            ))
            .json(request)
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn read_file(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsFileReadResponse, FabricClientError> {
        read_file(&self.inner, session_id, query).await
    }

    pub async fn write_file(
        &self,
        session_id: &SessionId,
        request: &FsFileWriteRequest,
    ) -> Result<FsWriteResponse, FabricClientError> {
        write_file(&self.inner, session_id, request).await
    }

    pub async fn edit_file(
        &self,
        session_id: &SessionId,
        request: &FsEditFileRequest,
    ) -> Result<FsEditFileResponse, FabricClientError> {
        edit_file(&self.inner, session_id, request).await
    }

    pub async fn apply_patch(
        &self,
        session_id: &SessionId,
        request: &FsApplyPatchRequest,
    ) -> Result<FsApplyPatchResponse, FabricClientError> {
        apply_patch(&self.inner, session_id, request).await
    }

    pub async fn mkdir(
        &self,
        session_id: &SessionId,
        request: &FsMkdirRequest,
    ) -> Result<FsStatResponse, FabricClientError> {
        mkdir(&self.inner, session_id, request).await
    }

    pub async fn remove(
        &self,
        session_id: &SessionId,
        request: &FsRemoveRequest,
    ) -> Result<FsRemoveResponse, FabricClientError> {
        remove(&self.inner, session_id, request).await
    }

    pub async fn exists(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsExistsResponse, FabricClientError> {
        exists(&self.inner, session_id, query).await
    }

    pub async fn stat(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsStatResponse, FabricClientError> {
        stat(&self.inner, session_id, query).await
    }

    pub async fn list_dir(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsListDirResponse, FabricClientError> {
        list_dir(&self.inner, session_id, query).await
    }

    pub async fn grep(
        &self,
        session_id: &SessionId,
        request: &FsGrepRequest,
    ) -> Result<FsGrepResponse, FabricClientError> {
        grep(&self.inner, session_id, request).await
    }

    pub async fn glob(
        &self,
        session_id: &SessionId,
        request: &FsGlobRequest,
    ) -> Result<FsGlobResponse, FabricClientError> {
        glob(&self.inner, session_id, request).await
    }
}

pub(crate) async fn read_file(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    query: &FsPathQuery,
) -> Result<FsFileReadResponse, FabricClientError> {
    let response = inner
        .http
        .get(format!(
            "{}/v1/sessions/{}/fs/file",
            inner.base_url, session_id.0
        ))
        .query(query)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn write_file(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    request: &FsFileWriteRequest,
) -> Result<FsWriteResponse, FabricClientError> {
    let response = inner
        .http
        .put(format!(
            "{}/v1/sessions/{}/fs/file",
            inner.base_url, session_id.0
        ))
        .json(request)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn edit_file(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    request: &FsEditFileRequest,
) -> Result<FsEditFileResponse, FabricClientError> {
    let response = inner
        .http
        .post(format!(
            "{}/v1/sessions/{}/fs/edit",
            inner.base_url, session_id.0
        ))
        .json(request)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn apply_patch(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    request: &FsApplyPatchRequest,
) -> Result<FsApplyPatchResponse, FabricClientError> {
    let response = inner
        .http
        .post(format!(
            "{}/v1/sessions/{}/fs/apply_patch",
            inner.base_url, session_id.0
        ))
        .json(request)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn mkdir(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    request: &FsMkdirRequest,
) -> Result<FsStatResponse, FabricClientError> {
    let response = inner
        .http
        .post(format!(
            "{}/v1/sessions/{}/fs/mkdir",
            inner.base_url, session_id.0
        ))
        .json(request)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn remove(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    request: &FsRemoveRequest,
) -> Result<FsRemoveResponse, FabricClientError> {
    let response = inner
        .http
        .post(format!(
            "{}/v1/sessions/{}/fs/remove",
            inner.base_url, session_id.0
        ))
        .json(request)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn exists(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    query: &FsPathQuery,
) -> Result<FsExistsResponse, FabricClientError> {
    let response = inner
        .http
        .get(format!(
            "{}/v1/sessions/{}/fs/exists",
            inner.base_url, session_id.0
        ))
        .query(query)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn stat(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    query: &FsPathQuery,
) -> Result<FsStatResponse, FabricClientError> {
    let response = inner
        .http
        .get(format!(
            "{}/v1/sessions/{}/fs/stat",
            inner.base_url, session_id.0
        ))
        .query(query)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn list_dir(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    query: &FsPathQuery,
) -> Result<FsListDirResponse, FabricClientError> {
    let response = inner
        .http
        .get(format!(
            "{}/v1/sessions/{}/fs/list_dir",
            inner.base_url, session_id.0
        ))
        .query(query)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn grep(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    request: &FsGrepRequest,
) -> Result<FsGrepResponse, FabricClientError> {
    let response = inner
        .http
        .post(format!(
            "{}/v1/sessions/{}/fs/grep",
            inner.base_url, session_id.0
        ))
        .json(request)
        .send()
        .await?;

    decode_json_response(response).await
}

pub(crate) async fn glob(
    inner: &FabricHttpClient,
    session_id: &SessionId,
    request: &FsGlobRequest,
) -> Result<FsGlobResponse, FabricClientError> {
    let response = inner
        .http
        .post(format!(
            "{}/v1/sessions/{}/fs/glob",
            inner.base_url, session_id.0
        ))
        .json(request)
        .send()
        .await?;

    decode_json_response(response).await
}
