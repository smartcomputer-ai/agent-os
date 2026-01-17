import {
  useMutation,
  useQueryClient,
  type QueryClient,
  type UseMutationOptions,
} from "@tanstack/react-query";
import type { ApiError } from "./http";
import * as endpoints from "./endpoints";
import { queryKeys } from "./queryKeys";
import type {
  BlobPutBody,
  BlobPutResponse,
  EventsPostBody,
  EventsPostResponse,
  GovApplyBody,
  GovApplyResponse,
  GovApproveBody,
  GovApproveResponse,
  GovProposeBody,
  GovProposeResponse,
  GovShadowBody,
  GovShadowResponse,
  WorkspaceAnnotationsSetBody,
  WorkspaceAnnotationsSetResponse,
  WorkspaceEmptyRootBody,
  WorkspaceEmptyRootResponse,
  WorkspaceRemoveBody,
  WorkspaceRemoveResponse,
  WorkspaceWriteBytesBody,
  WorkspaceWriteBytesResponse,
} from "./apiTypes";

type MutationOptions<TData, TVariables> = Omit<
  UseMutationOptions<TData, ApiError, TVariables>,
  "mutationFn"
>;

const workspaceQueryPrefixes = [
  ["workspace_list"],
  ["workspace_read_bytes"],
  ["workspace_read_ref"],
  ["workspace_annotations_get"],
  ["workspace_resolve"],
] as const;

const stateQueryPrefixes = [["state_get"], ["state_cells"]] as const;

const journalQueryPrefixes = [["journal_head"], ["journal_tail"]] as const;

function invalidateWorkspaceQueries(queryClient: QueryClient) {
  for (const key of workspaceQueryPrefixes) {
    queryClient.invalidateQueries({ queryKey: key });
  }
}

function invalidateStateQueries(queryClient: QueryClient) {
  for (const key of stateQueryPrefixes) {
    queryClient.invalidateQueries({ queryKey: key });
  }
}

function invalidateJournalQueries(queryClient: QueryClient) {
  for (const key of journalQueryPrefixes) {
    queryClient.invalidateQueries({ queryKey: key });
  }
}

function invalidateManifestQueries(queryClient: QueryClient) {
  queryClient.invalidateQueries({ queryKey: ["manifest"] });
}

function useMutationWithInvalidation<TData, TVariables>(
  mutationFn: (variables: TVariables) => Promise<TData>,
  invalidate: (
    queryClient: QueryClient,
    data: TData,
    variables: TVariables,
  ) => void,
  options?: MutationOptions<TData, TVariables>,
) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    ...options,
    onSuccess: (data, variables, context, mutation) => {
      invalidate(queryClient, data, variables);
      options?.onSuccess?.(data, variables, context, mutation);
    },
  });
}

export function useBlobPut(
  options?: MutationOptions<BlobPutResponse, BlobPutBody>,
) {
  return useMutationWithInvalidation(
    endpoints.blobPut,
    (queryClient, data) => {
      queryClient.invalidateQueries({
        queryKey: queryKeys.blobGet(data.hash),
      });
    },
    options,
  );
}

export function useEventsPost(
  options?: MutationOptions<EventsPostResponse, EventsPostBody>,
) {
  return useMutationWithInvalidation(
    endpoints.eventsPost,
    (queryClient) => {
      invalidateJournalQueries(queryClient);
      invalidateStateQueries(queryClient);
      invalidateManifestQueries(queryClient);
    },
    options,
  );
}

export function useGovApply(
  options?: MutationOptions<GovApplyResponse, GovApplyBody>,
) {
  return useMutationWithInvalidation(
    endpoints.govApply,
    (queryClient) => {
      invalidateJournalQueries(queryClient);
      invalidateStateQueries(queryClient);
      invalidateManifestQueries(queryClient);
    },
    options,
  );
}

export function useGovApprove(
  options?: MutationOptions<GovApproveResponse, GovApproveBody>,
) {
  return useMutationWithInvalidation(
    endpoints.govApprove,
    (queryClient) => {
      invalidateJournalQueries(queryClient);
      invalidateStateQueries(queryClient);
      invalidateManifestQueries(queryClient);
    },
    options,
  );
}

export function useGovPropose(
  options?: MutationOptions<GovProposeResponse, GovProposeBody>,
) {
  return useMutationWithInvalidation(
    endpoints.govPropose,
    (queryClient) => {
      invalidateJournalQueries(queryClient);
      invalidateStateQueries(queryClient);
      invalidateManifestQueries(queryClient);
    },
    options,
  );
}

export function useGovShadow(
  options?: MutationOptions<GovShadowResponse, GovShadowBody>,
) {
  return useMutationWithInvalidation(
    endpoints.govShadow,
    (queryClient) => {
      invalidateJournalQueries(queryClient);
      invalidateStateQueries(queryClient);
      invalidateManifestQueries(queryClient);
    },
    options,
  );
}

export function useWorkspaceAnnotationsSet(
  options?: MutationOptions<
    WorkspaceAnnotationsSetResponse,
    WorkspaceAnnotationsSetBody
  >,
) {
  return useMutationWithInvalidation(
    endpoints.workspaceAnnotationsSet,
    (queryClient) => {
      invalidateWorkspaceQueries(queryClient);
    },
    options,
  );
}

export function useWorkspaceEmptyRoot(
  options?: MutationOptions<WorkspaceEmptyRootResponse, WorkspaceEmptyRootBody>,
) {
  return useMutationWithInvalidation(
    endpoints.workspaceEmptyRoot,
    (queryClient) => {
      invalidateWorkspaceQueries(queryClient);
    },
    options,
  );
}

export function useWorkspaceRemove(
  options?: MutationOptions<WorkspaceRemoveResponse, WorkspaceRemoveBody>,
) {
  return useMutationWithInvalidation(
    endpoints.workspaceRemove,
    (queryClient) => {
      invalidateWorkspaceQueries(queryClient);
    },
    options,
  );
}

export function useWorkspaceWriteBytes(
  options?: MutationOptions<
    WorkspaceWriteBytesResponse,
    WorkspaceWriteBytesBody
  >,
) {
  return useMutationWithInvalidation(
    endpoints.workspaceWriteBytes,
    (queryClient) => {
      invalidateWorkspaceQueries(queryClient);
    },
    options,
  );
}
