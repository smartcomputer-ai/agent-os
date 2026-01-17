import type { operations } from "./types";

export type BlobPutBody =
  operations["blob_put"]["requestBody"]["content"]["application/json"];
export type BlobPutResponse =
  operations["blob_put"]["responses"][200]["content"]["application/json"];

export type DefsListQuery = operations["defs_list"]["parameters"]["query"];
export type DefsListResponse =
  operations["defs_list"]["responses"][200]["content"]["application/json"];

export type DefsGetPath = operations["defs_get"]["parameters"]["path"];
export type DefsGetResponse =
  operations["defs_get"]["responses"][200]["content"]["application/json"];

export type EventsPostBody =
  operations["events_post"]["requestBody"]["content"]["application/json"];
export type EventsPostResponse =
  operations["events_post"]["responses"][200]["content"]["application/json"];

export type GovApplyBody =
  operations["gov_apply"]["requestBody"]["content"]["application/json"];
export type GovApplyResponse =
  operations["gov_apply"]["responses"][200]["content"]["application/json"];

export type GovApproveBody =
  operations["gov_approve"]["requestBody"]["content"]["application/json"];
export type GovApproveResponse =
  operations["gov_approve"]["responses"][200]["content"]["application/json"];

export type GovProposeBody =
  operations["gov_propose"]["requestBody"]["content"]["application/json"];
export type GovProposeResponse =
  operations["gov_propose"]["responses"][200]["content"]["application/json"];

export type GovShadowBody =
  operations["gov_shadow"]["requestBody"]["content"]["application/json"];
export type GovShadowResponse =
  operations["gov_shadow"]["responses"][200]["content"]["application/json"];

export type HealthResponse =
  operations["health"]["responses"][200]["content"]["application/json"];

export type InfoResponse =
  operations["info"]["responses"][200]["content"]["application/json"];

export type JournalTailQuery = operations["journal_tail"]["parameters"]["query"];
export type JournalTailResponse =
  operations["journal_tail"]["responses"][200]["content"]["application/json"];

export type JournalHeadResponse =
  operations["journal_head"]["responses"][200]["content"]["application/json"];

export type ManifestQuery = operations["manifest"]["parameters"]["query"];
export type ManifestResponse =
  operations["manifest"]["responses"][200]["content"]["application/json"];

export type StateGetPath = operations["state_get"]["parameters"]["path"];
export type StateGetQuery = operations["state_get"]["parameters"]["query"];
export type StateGetResponse =
  operations["state_get"]["responses"][200]["content"]["application/json"];

export type StateCellsPath = operations["state_cells"]["parameters"]["path"];
export type StateCellsResponse =
  operations["state_cells"]["responses"][200]["content"]["application/json"];

export type WorkspaceAnnotationsGetQuery =
  operations["workspace_annotations_get"]["parameters"]["query"];
export type WorkspaceAnnotationsGetResponse =
  operations["workspace_annotations_get"]["responses"][200]["content"]["application/json"];

export type WorkspaceAnnotationsSetBody =
  operations["workspace_annotations_set"]["requestBody"]["content"]["application/json"];
export type WorkspaceAnnotationsSetResponse =
  operations["workspace_annotations_set"]["responses"][200]["content"]["application/json"];

export type WorkspaceEmptyRootBody =
  operations["workspace_empty_root"]["requestBody"]["content"]["application/json"];
export type WorkspaceEmptyRootResponse =
  operations["workspace_empty_root"]["responses"][200]["content"]["application/json"];

export type WorkspaceListQuery = operations["workspace_list"]["parameters"]["query"];
export type WorkspaceListResponse =
  operations["workspace_list"]["responses"][200]["content"]["application/json"];

export type WorkspaceReadBytesQuery =
  operations["workspace_read_bytes"]["parameters"]["query"];

export type WorkspaceReadRefQuery =
  operations["workspace_read_ref"]["parameters"]["query"];
export type WorkspaceReadRefResponse =
  operations["workspace_read_ref"]["responses"][200]["content"]["application/json"];

export type WorkspaceRemoveBody =
  operations["workspace_remove"]["requestBody"]["content"]["application/json"];
export type WorkspaceRemoveResponse =
  operations["workspace_remove"]["responses"][200]["content"]["application/json"];

export type WorkspaceResolveQuery =
  operations["workspace_resolve"]["parameters"]["query"];
export type WorkspaceResolveResponse =
  operations["workspace_resolve"]["responses"][200]["content"]["application/json"];

export type WorkspaceWriteBytesBody =
  operations["workspace_write_bytes"]["requestBody"]["content"]["application/json"];
export type WorkspaceWriteBytesResponse =
  operations["workspace_write_bytes"]["responses"][200]["content"]["application/json"];
