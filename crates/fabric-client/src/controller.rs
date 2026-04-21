use fabric_protocol::{
    ControllerExecRequest, ControllerInfoResponse, ControllerSessionListResponse,
    ControllerSessionOpenRequest, ControllerSessionOpenResponse, ControllerSessionSummary,
    ControllerSignalSessionRequest, FsApplyPatchRequest, FsApplyPatchResponse, FsEditFileRequest,
    FsEditFileResponse, FsExistsResponse, FsFileReadResponse, FsFileWriteRequest, FsGlobRequest,
    FsGlobResponse, FsGrepRequest, FsGrepResponse, FsListDirResponse, FsMkdirRequest, FsPathQuery,
    FsRemoveRequest, FsRemoveResponse, FsStatResponse, FsWriteResponse, HealthResponse,
    HostHeartbeatRequest, HostId, HostInventoryResponse, HostListResponse, HostRegisterRequest,
    HostRegisterResponse, HostSummary, SessionId, SessionLabelsPatchRequest, SessionLabelsResponse,
};

use crate::{
    common::{
        ExecEventClientStream, FabricClientError, FabricHttpClient, decode_error_response,
        decode_exec_stream, decode_json_response,
    },
    host,
};

#[derive(Debug, Clone)]
pub struct FabricControllerClient {
    inner: FabricHttpClient,
}

impl FabricControllerClient {
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

    pub async fn info(&self) -> Result<ControllerInfoResponse, FabricClientError> {
        let response = self
            .inner
            .apply_auth(
                self.inner
                    .http
                    .get(format!("{}/v1/controller/info", self.inner.base_url)),
            )
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn register_host(
        &self,
        request: &HostRegisterRequest,
    ) -> Result<HostRegisterResponse, FabricClientError> {
        let response = self
            .inner
            .apply_auth(
                self.inner
                    .http
                    .post(format!("{}/v1/hosts/register", self.inner.base_url)),
            )
            .json(request)
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn heartbeat_host(
        &self,
        host_id: &HostId,
        request: &HostHeartbeatRequest,
    ) -> Result<HostRegisterResponse, FabricClientError> {
        let response = self
            .inner
            .apply_auth(self.inner.http.post(format!(
                "{}/v1/hosts/{}/heartbeat",
                self.inner.base_url, host_id.0
            )))
            .json(request)
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn list_hosts(&self) -> Result<HostListResponse, FabricClientError> {
        let response = self
            .inner
            .apply_auth(
                self.inner
                    .http
                    .get(format!("{}/v1/hosts", self.inner.base_url)),
            )
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn host(&self, host_id: &HostId) -> Result<HostSummary, FabricClientError> {
        let response = self
            .inner
            .apply_auth(
                self.inner
                    .http
                    .get(format!("{}/v1/hosts/{}", self.inner.base_url, host_id.0)),
            )
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn host_inventory(
        &self,
        host_id: &HostId,
    ) -> Result<HostInventoryResponse, FabricClientError> {
        let response = self
            .inner
            .apply_auth(self.inner.http.get(format!(
                "{}/v1/hosts/{}/inventory",
                self.inner.base_url, host_id.0
            )))
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn open_session(
        &self,
        request: &ControllerSessionOpenRequest,
    ) -> Result<ControllerSessionOpenResponse, FabricClientError> {
        let response = self
            .inner
            .apply_auth(
                self.inner
                    .http
                    .post(format!("{}/v1/sessions", self.inner.base_url)),
            )
            .json(request)
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn session(
        &self,
        session_id: &SessionId,
    ) -> Result<ControllerSessionSummary, FabricClientError> {
        let response = self
            .inner
            .apply_auth(self.inner.http.get(format!(
                "{}/v1/sessions/{}",
                self.inner.base_url, session_id.0
            )))
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn list_sessions(
        &self,
        label_filters: &[(String, String)],
    ) -> Result<ControllerSessionListResponse, FabricClientError> {
        let labels = label_filters
            .iter()
            .map(|(key, value)| format!("{key}:{value}"))
            .collect::<Vec<_>>();
        let response = self
            .inner
            .apply_auth(
                self.inner
                    .http
                    .get(format!("{}/v1/sessions", self.inner.base_url)),
            )
            .query(
                &labels
                    .iter()
                    .map(|label| ("label", label))
                    .collect::<Vec<_>>(),
            )
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn patch_session_labels(
        &self,
        session_id: &SessionId,
        request: &SessionLabelsPatchRequest,
    ) -> Result<SessionLabelsResponse, FabricClientError> {
        let response = self
            .inner
            .apply_auth(self.inner.http.patch(format!(
                "{}/v1/sessions/{}/labels",
                self.inner.base_url, session_id.0
            )))
            .json(request)
            .send()
            .await?;

        decode_json_response(response).await
    }

    pub async fn exec_session_stream(
        &self,
        session_id: &SessionId,
        request: &ControllerExecRequest,
    ) -> Result<ExecEventClientStream, FabricClientError> {
        let response = self
            .inner
            .apply_auth(self.inner.http.post(format!(
                "{}/v1/sessions/{}/exec",
                self.inner.base_url, session_id.0
            )))
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
        request: &ControllerSignalSessionRequest,
    ) -> Result<ControllerSessionSummary, FabricClientError> {
        let response = self
            .inner
            .apply_auth(self.inner.http.post(format!(
                "{}/v1/sessions/{}/signal",
                self.inner.base_url, session_id.0
            )))
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
        host::read_file(&self.inner, session_id, query).await
    }

    pub async fn write_file(
        &self,
        session_id: &SessionId,
        request: &FsFileWriteRequest,
    ) -> Result<FsWriteResponse, FabricClientError> {
        host::write_file(&self.inner, session_id, request).await
    }

    pub async fn edit_file(
        &self,
        session_id: &SessionId,
        request: &FsEditFileRequest,
    ) -> Result<FsEditFileResponse, FabricClientError> {
        host::edit_file(&self.inner, session_id, request).await
    }

    pub async fn apply_patch(
        &self,
        session_id: &SessionId,
        request: &FsApplyPatchRequest,
    ) -> Result<FsApplyPatchResponse, FabricClientError> {
        host::apply_patch(&self.inner, session_id, request).await
    }

    pub async fn mkdir(
        &self,
        session_id: &SessionId,
        request: &FsMkdirRequest,
    ) -> Result<FsStatResponse, FabricClientError> {
        host::mkdir(&self.inner, session_id, request).await
    }

    pub async fn remove(
        &self,
        session_id: &SessionId,
        request: &FsRemoveRequest,
    ) -> Result<FsRemoveResponse, FabricClientError> {
        host::remove(&self.inner, session_id, request).await
    }

    pub async fn exists(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsExistsResponse, FabricClientError> {
        host::exists(&self.inner, session_id, query).await
    }

    pub async fn stat(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsStatResponse, FabricClientError> {
        host::stat(&self.inner, session_id, query).await
    }

    pub async fn list_dir(
        &self,
        session_id: &SessionId,
        query: &FsPathQuery,
    ) -> Result<FsListDirResponse, FabricClientError> {
        host::list_dir(&self.inner, session_id, query).await
    }

    pub async fn grep(
        &self,
        session_id: &SessionId,
        request: &FsGrepRequest,
    ) -> Result<FsGrepResponse, FabricClientError> {
        host::grep(&self.inner, session_id, request).await
    }

    pub async fn glob(
        &self,
        session_id: &SessionId,
        request: &FsGlobRequest,
    ) -> Result<FsGlobResponse, FabricClientError> {
        host::glob(&self.inner, session_id, request).await
    }
}
