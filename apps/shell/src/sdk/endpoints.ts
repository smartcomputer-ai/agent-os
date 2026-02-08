import { apiRequestBinary, apiRequestJson } from "./http";
import type {
  BlobPutBody,
  BlobPutResponse,
  DefsGetPath,
  DefsGetResponse,
  DefsListQuery,
  DefsListResponse,
  EventsPostBody,
  EventsPostResponse,
  GovGetPath,
  GovGetResponse,
  GovApplyBody,
  GovApplyResponse,
  GovApproveBody,
  GovApproveResponse,
  GovListQuery,
  GovListResponse,
  GovProposeBody,
  GovProposeResponse,
  GovShadowBody,
  GovShadowResponse,
  HealthResponse,
  InfoResponse,
  JournalHeadResponse,
  JournalTailQuery,
  JournalTailResponse,
  ManifestQuery,
  ManifestResponse,
  StateCellsPath,
  StateCellsResponse,
  StateGetPath,
  StateGetQuery,
  StateGetResponse,
  WorkspaceAnnotationsGetQuery,
  WorkspaceAnnotationsGetResponse,
  WorkspaceAnnotationsSetBody,
  WorkspaceAnnotationsSetResponse,
  WorkspaceEmptyRootBody,
  WorkspaceEmptyRootResponse,
  WorkspaceListQuery,
  WorkspaceListResponse,
  WorkspaceReadBytesQuery,
  WorkspaceReadRefQuery,
  WorkspaceReadRefResponse,
  WorkspaceRemoveBody,
  WorkspaceRemoveResponse,
  WorkspaceResolveQuery,
  WorkspaceResolveResponse,
  WorkspaceWriteBytesBody,
  WorkspaceWriteBytesResponse,
} from "./apiTypes";

export interface DebugTraceQuery {
  event_hash?: string;
  schema?: string;
  correlate_by?: string;
  value?: string;
  window_limit?: number;
}

export type DebugTraceResponse = Record<string, unknown>;

export function blobPut(body: BlobPutBody): Promise<BlobPutResponse> {
  return apiRequestJson("post", "/api/blob", { body });
}

export function blobGet(hash: string): Promise<ArrayBuffer> {
  return apiRequestBinary("get", "/api/blob/{hash}", {
    pathParams: { hash },
  });
}

export function defsList(query?: DefsListQuery): Promise<DefsListResponse> {
  return apiRequestJson("get", "/api/defs", { query });
}

export function defsGet(path: DefsGetPath): Promise<DefsGetResponse> {
  return apiRequestJson("get", "/api/defs/{kind}/{name}", {
    pathParams: path,
  });
}

export function eventsPost(body: EventsPostBody): Promise<EventsPostResponse> {
  return apiRequestJson("post", "/api/events", { body });
}

export function govList(query?: GovListQuery): Promise<GovListResponse> {
  return apiRequestJson("get", "/api/gov", { query });
}

export function govGet(path: GovGetPath): Promise<GovGetResponse> {
  return apiRequestJson("get", "/api/gov/{proposal_id}", {
    pathParams: path,
  });
}

export function govApply(body: GovApplyBody): Promise<GovApplyResponse> {
  return apiRequestJson("post", "/api/gov/apply", { body });
}

export function govApprove(body: GovApproveBody): Promise<GovApproveResponse> {
  return apiRequestJson("post", "/api/gov/approve", { body });
}

export function govPropose(body: GovProposeBody): Promise<GovProposeResponse> {
  return apiRequestJson("post", "/api/gov/propose", { body });
}

export function govShadow(body: GovShadowBody): Promise<GovShadowResponse> {
  return apiRequestJson("post", "/api/gov/shadow", { body });
}

export function health(): Promise<HealthResponse> {
  return apiRequestJson("get", "/api/health");
}

export function info(): Promise<InfoResponse> {
  return apiRequestJson("get", "/api/info");
}

export function journalTail(
  query?: JournalTailQuery,
): Promise<JournalTailResponse> {
  return apiRequestJson("get", "/api/journal", { query });
}

export function journalHead(): Promise<JournalHeadResponse> {
  return apiRequestJson("get", "/api/journal/head");
}

export function debugTrace(
  query: DebugTraceQuery,
): Promise<DebugTraceResponse> {
  return apiRequestJson("get", "/api/debug/trace" as never, {
    query,
  } as never) as Promise<DebugTraceResponse>;
}

export function manifest(query?: ManifestQuery): Promise<ManifestResponse> {
  return apiRequestJson("get", "/api/manifest", { query });
}

export function stateGet(
  path: StateGetPath,
  query?: StateGetQuery,
): Promise<StateGetResponse> {
  return apiRequestJson("get", "/api/state/{reducer}", {
    pathParams: path,
    query,
  });
}

export function stateCells(path: StateCellsPath): Promise<StateCellsResponse> {
  return apiRequestJson("get", "/api/state/{reducer}/cells", {
    pathParams: path,
  });
}

export function workspaceAnnotationsGet(
  query?: WorkspaceAnnotationsGetQuery,
): Promise<WorkspaceAnnotationsGetResponse> {
  return apiRequestJson("get", "/api/workspace/annotations", { query });
}

export function workspaceAnnotationsSet(
  body: WorkspaceAnnotationsSetBody,
): Promise<WorkspaceAnnotationsSetResponse> {
  return apiRequestJson("post", "/api/workspace/annotations", { body });
}

export function workspaceEmptyRoot(
  body: WorkspaceEmptyRootBody,
): Promise<WorkspaceEmptyRootResponse> {
  return apiRequestJson("post", "/api/workspace/empty-root", { body });
}

export function workspaceList(
  query?: WorkspaceListQuery,
): Promise<WorkspaceListResponse> {
  return apiRequestJson("get", "/api/workspace/list", { query });
}

export function workspaceReadBytes(
  query: WorkspaceReadBytesQuery,
): Promise<ArrayBuffer> {
  return apiRequestBinary("get", "/api/workspace/read-bytes", { query });
}

export function workspaceReadRef(
  query: WorkspaceReadRefQuery,
): Promise<WorkspaceReadRefResponse> {
  return apiRequestJson("get", "/api/workspace/read-ref", { query });
}

export function workspaceRemove(
  body: WorkspaceRemoveBody,
): Promise<WorkspaceRemoveResponse> {
  return apiRequestJson("post", "/api/workspace/remove", { body });
}

export function workspaceResolve(
  query: WorkspaceResolveQuery,
): Promise<WorkspaceResolveResponse> {
  return apiRequestJson("get", "/api/workspace/resolve", { query });
}

export function workspaceWriteBytes(
  body: WorkspaceWriteBytesBody,
): Promise<WorkspaceWriteBytesResponse> {
  return apiRequestJson("post", "/api/workspace/write-bytes", { body });
}
