import type {
  DefsGetPath,
  DefsListQuery,
  JournalTailQuery,
  ManifestQuery,
  StateCellsPath,
  StateGetPath,
  StateGetQuery,
  GovGetPath,
  GovListQuery,
  WorkspaceAnnotationsGetQuery,
  WorkspaceListQuery,
  WorkspaceReadBytesQuery,
  WorkspaceReadRefQuery,
  WorkspaceResolveQuery,
} from "./apiTypes";
import type { DebugTraceQuery } from "./endpoints";

export const queryKeys = {
  blobGet: (hash: string) => ["blob_get", hash] as const,
  health: () => ["health"] as const,
  info: () => ["info"] as const,
  defsList: (params?: DefsListQuery) => ["defs_list", params ?? {}] as const,
  defsGet: (path: DefsGetPath) =>
    ["defs_get", path.kind, path.name] as const,
  govList: (params?: GovListQuery) => ["gov_list", params ?? {}] as const,
  govGet: (path: GovGetPath) => ["gov_get", path.proposal_id] as const,
  journalHead: () => ["journal_head"] as const,
  journalTail: (params?: JournalTailQuery) =>
    ["journal_tail", params ?? {}] as const,
  debugTrace: (params: DebugTraceQuery) =>
    ["debug_trace", params.event_hash, params.window_limit ?? null] as const,
  manifest: (params?: ManifestQuery) => ["manifest", params ?? {}] as const,
  stateGet: (path: StateGetPath, query?: StateGetQuery) =>
    ["state_get", path.reducer, query ?? {}] as const,
  stateCells: (path: StateCellsPath) => ["state_cells", path.reducer] as const,
  workspaceAnnotationsGet: (params?: WorkspaceAnnotationsGetQuery) =>
    ["workspace_annotations_get", params ?? {}] as const,
  workspaceList: (params?: WorkspaceListQuery) =>
    ["workspace_list", params ?? {}] as const,
  workspaceReadBytes: (params: WorkspaceReadBytesQuery) =>
    ["workspace_read_bytes", params] as const,
  workspaceReadRef: (params: WorkspaceReadRefQuery) =>
    ["workspace_read_ref", params] as const,
  workspaceResolve: (params: WorkspaceResolveQuery) =>
    ["workspace_resolve", params.workspace, params.version ?? null] as const,
  chatState: (chatId: string) =>
    ["state_get", "demiurge/Demiurge@1", { key: chatId }] as const,
  chatList: () =>
    ["state_cells", "demiurge/Demiurge@1"] as const,
};
