import { useQuery, type UseQueryOptions } from "@tanstack/react-query";
import type { ApiError } from "./http";
import { queryKeys } from "./queryKeys";
import * as endpoints from "./endpoints";
import type {
  DefsGetPath,
  DefsGetResponse,
  DefsListQuery,
  DefsListResponse,
  GovGetPath,
  GovGetResponse,
  GovListQuery,
  GovListResponse,
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
  WorkspaceListQuery,
  WorkspaceListResponse,
  WorkspaceReadBytesQuery,
  WorkspaceReadRefQuery,
  WorkspaceReadRefResponse,
  WorkspaceResolveQuery,
  WorkspaceResolveResponse,
} from "./apiTypes";

type QueryOptions<TData, TQueryKey extends readonly unknown[]> = Omit<
  UseQueryOptions<TData, ApiError, TData, TQueryKey>,
  "queryKey" | "queryFn"
>;


export function useBlobGet(
  hash: string,
  options?: QueryOptions<ArrayBuffer, ReturnType<typeof queryKeys.blobGet>>,
) {
  return useQuery({
    queryKey: queryKeys.blobGet(hash),
    queryFn: () => endpoints.blobGet(hash),
    ...options,
  });
}

export function useHealth(
  options?: QueryOptions<HealthResponse, ReturnType<typeof queryKeys.health>>,
) {
  return useQuery({
    queryKey: queryKeys.health(),
    queryFn: () => endpoints.health(),
    refetchInterval: (query) => (query.state.data?.ok ? 15000 : 2000),
    ...options,
  });
}

export function useInfo(
  options?: QueryOptions<InfoResponse, ReturnType<typeof queryKeys.info>>,
) {
  return useQuery({
    queryKey: queryKeys.info(),
    queryFn: () => endpoints.info(),
    refetchInterval: (query) => (query.state.data ? 30000 : 5000),
    ...options,
  });
}

export function useDefsList(
  params?: DefsListQuery,
  options?: QueryOptions<DefsListResponse, ReturnType<typeof queryKeys.defsList>>,
) {
  return useQuery({
    queryKey: queryKeys.defsList(params),
    queryFn: () => endpoints.defsList(params),
    ...options,
  });
}

export function useDefsGet(
  path: DefsGetPath,
  options?: QueryOptions<DefsGetResponse, ReturnType<typeof queryKeys.defsGet>>,
) {
  return useQuery({
    queryKey: queryKeys.defsGet(path),
    queryFn: () => endpoints.defsGet(path),
    ...options,
  });
}

export function useGovList(
  params?: GovListQuery,
  options?: QueryOptions<GovListResponse, ReturnType<typeof queryKeys.govList>>,
) {
  return useQuery({
    queryKey: queryKeys.govList(params),
    queryFn: () => endpoints.govList(params),
    ...options,
  });
}

export function useGovGet(
  path: GovGetPath,
  options?: QueryOptions<GovGetResponse, ReturnType<typeof queryKeys.govGet>>,
) {
  return useQuery({
    queryKey: queryKeys.govGet(path),
    queryFn: () => endpoints.govGet(path),
    ...options,
  });
}

export function useJournalHead(
  options?: QueryOptions<
    JournalHeadResponse,
    ReturnType<typeof queryKeys.journalHead>
  >,
) {
  return useQuery({
    queryKey: queryKeys.journalHead(),
    queryFn: () => endpoints.journalHead(),
    ...options,
  });
}

export function useJournalTail(
  params?: JournalTailQuery,
  options?: QueryOptions<
    JournalTailResponse,
    ReturnType<typeof queryKeys.journalTail>
  >,
) {
  return useQuery({
    queryKey: queryKeys.journalTail(params),
    queryFn: () => endpoints.journalTail(params),
    ...options,
  });
}

export function useManifest(
  params?: ManifestQuery,
  options?: QueryOptions<ManifestResponse, ReturnType<typeof queryKeys.manifest>>,
) {
  return useQuery({
    queryKey: queryKeys.manifest(params),
    queryFn: () => endpoints.manifest(params),
    ...options,
  });
}

export function useStateGet(
  path: StateGetPath,
  query?: StateGetQuery,
  options?: QueryOptions<StateGetResponse, ReturnType<typeof queryKeys.stateGet>>,
) {
  return useQuery({
    queryKey: queryKeys.stateGet(path, query),
    queryFn: () => endpoints.stateGet(path, query),
    ...options,
  });
}

export function useStateCells(
  path: StateCellsPath,
  options?: QueryOptions<
    StateCellsResponse,
    ReturnType<typeof queryKeys.stateCells>
  >,
) {
  return useQuery({
    queryKey: queryKeys.stateCells(path),
    queryFn: () => endpoints.stateCells(path),
    ...options,
  });
}

export function useWorkspaceAnnotationsGet(
  params?: WorkspaceAnnotationsGetQuery,
  options?: QueryOptions<
    WorkspaceAnnotationsGetResponse,
    ReturnType<typeof queryKeys.workspaceAnnotationsGet>
  >,
) {
  return useQuery({
    queryKey: queryKeys.workspaceAnnotationsGet(params),
    queryFn: () => endpoints.workspaceAnnotationsGet(params),
    ...options,
  });
}

export function useWorkspaceList(
  params?: WorkspaceListQuery,
  options?: QueryOptions<
    WorkspaceListResponse,
    ReturnType<typeof queryKeys.workspaceList>
  >,
) {
  return useQuery({
    queryKey: queryKeys.workspaceList(params),
    queryFn: () => endpoints.workspaceList(params),
    ...options,
  });
}

export function useWorkspaceReadBytes(
  params: WorkspaceReadBytesQuery,
  options?: QueryOptions<
    ArrayBuffer,
    ReturnType<typeof queryKeys.workspaceReadBytes>
  >,
) {
  return useQuery({
    queryKey: queryKeys.workspaceReadBytes(params),
    queryFn: () => endpoints.workspaceReadBytes(params),
    ...options,
  });
}

export function useWorkspaceReadRef(
  params: WorkspaceReadRefQuery,
  options?: QueryOptions<
    WorkspaceReadRefResponse,
    ReturnType<typeof queryKeys.workspaceReadRef>
  >,
) {
  return useQuery({
    queryKey: queryKeys.workspaceReadRef(params),
    queryFn: () => endpoints.workspaceReadRef(params),
    ...options,
  });
}

export function useWorkspaceResolve(
  params: WorkspaceResolveQuery,
  options?: QueryOptions<
    WorkspaceResolveResponse,
    ReturnType<typeof queryKeys.workspaceResolve>
  >,
) {
  return useQuery({
    queryKey: queryKeys.workspaceResolve(params),
    queryFn: () => endpoints.workspaceResolve(params),
    ...options,
  });
}

export function useChatState(
  chatId: string,
  options?: QueryOptions<StateGetResponse, ReturnType<typeof queryKeys.chatState>>,
) {
  return useQuery({
    queryKey: queryKeys.chatState(chatId),
    queryFn: () =>
      endpoints.stateGet(
        { reducer: "demiurge/Demiurge@1" },
        { key_b64: btoa(chatId) },
      ),
    refetchInterval: 3000,
    ...options,
  });
}

export function useChatList(
  options?: QueryOptions<StateCellsResponse, ReturnType<typeof queryKeys.chatList>>,
) {
  return useQuery({
    queryKey: queryKeys.chatList(),
    queryFn: () => endpoints.stateCells({ reducer: "demiurge/Demiurge@1" }),
    refetchInterval: 5000,
    ...options,
  });
}
