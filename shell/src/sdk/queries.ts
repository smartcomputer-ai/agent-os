import { useQuery, type UseQueryOptions } from "@tanstack/react-query";
import type { ApiError } from "./http";
import { queryKeys } from "./queryKeys";
import * as endpoints from "./endpoints";
import type { DebugTraceQuery, DebugTraceResponse } from "./endpoints";
import { decodeCborBytes, encodeCborTextToBase64 } from "./cbor";
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

const UTF8_DECODER = new TextDecoder();

type JsonRecord = Record<string, unknown>;

interface RunRequestRecord {
  seq: number;
  observed_at_ns: number;
  input_ref: string;
}

interface LlmIntentRecord {
  seq: number;
  intent_hash: string;
}

interface LlmReceiptRecord {
  seq: number;
  intent_hash: string;
  status: string;
  output_ref: string | null;
  tool_calls_ref: string | null;
  assistant_text: string | null;
  token_usage: {
    prompt: number;
    completion: number;
    total: number | null;
  } | null;
}

export interface ChatTranscriptMessage {
  id: string;
  role: "user" | "assistant";
  text: string;
  observed_at_ns: number;
  input_ref?: string | null;
  output_ref?: string | null;
  tool_calls_ref?: string | null;
  token_usage?: {
    prompt: number;
    completion: number;
    total?: number | null;
  } | null;
  failed?: boolean;
}

export interface ChatTranscript {
  messages: ChatTranscriptMessage[];
}

function asRecord(value: unknown): JsonRecord | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonRecord)
    : null;
}

function asString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function asNumber(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function bytesFromUnknown(value: unknown): Uint8Array | null {
  if (Array.isArray(value)) {
    const numbers = value.filter((item): item is number => typeof item === "number");
    if (numbers.length !== value.length) {
      return null;
    }
    return new Uint8Array(numbers);
  }
  if (typeof value === "string") {
    try {
      const binary = atob(value);
      const bytes = new Uint8Array(binary.length);
      for (let i = 0; i < binary.length; i += 1) {
        bytes[i] = binary.charCodeAt(i);
      }
      return bytes;
    } catch {
      return null;
    }
  }
  return null;
}

function cborHashToHex(value: unknown): string | null {
  const bytes = bytesFromUnknown(value);
  if (!bytes) return null;
  let hex = "";
  for (const byte of bytes) {
    hex += byte.toString(16).padStart(2, "0");
  }
  return hex;
}

function decodeCborFromJournal<T>(value: unknown): T | null {
  const bytes = bytesFromUnknown(value);
  if (!bytes) return null;
  try {
    return decodeCborBytes<T>(bytes);
  } catch {
    return null;
  }
}

function decodeInstanceKey(value: unknown): string | null {
  const bytes = bytesFromUnknown(value);
  if (!bytes) return null;
  try {
    const decoded = decodeCborBytes<unknown>(bytes);
    return typeof decoded === "string" ? decoded : null;
  } catch {
    return null;
  }
}

function extractTextFromContent(content: unknown): string | null {
  if (typeof content === "string") {
    return content.trim() || null;
  }
  if (!Array.isArray(content)) {
    return null;
  }
  const parts: string[] = [];
  for (const part of content) {
    const partObj = asRecord(part);
    if (!partObj) continue;
    const text = asString(partObj.text);
    if (text) {
      parts.push(text);
    }
  }
  if (parts.length === 0) return null;
  return parts.join("\n\n");
}

function extractLatestUserText(rawBlob: string): string {
  const trimmed = rawBlob.trim();
  if (!trimmed) {
    return "";
  }

  try {
    const parsed = JSON.parse(trimmed) as unknown;
    if (Array.isArray(parsed)) {
      for (let i = parsed.length - 1; i >= 0; i -= 1) {
        const entry = asRecord(parsed[i]);
        if (!entry) continue;
        if (asString(entry.role) !== "user") continue;
        const text = extractTextFromContent(entry.content);
        if (text) return text;
      }
    }

    const single = asRecord(parsed);
    if (single) {
      const text = extractTextFromContent(single.content);
      if (text) return text;
    }
  } catch {
    return trimmed;
  }

  return trimmed;
}

function extractToolNames(rawBlob: string): string[] {
  try {
    const parsed = JSON.parse(rawBlob) as unknown;
    if (!Array.isArray(parsed)) return [];
    const names: string[] = [];
    for (const item of parsed) {
      const entry = asRecord(item);
      const name = asString(entry?.tool_name);
      if (name) names.push(name);
    }
    return names;
  } catch {
    return [];
  }
}

function extractAssistantMessage(
  rawBlob: string,
  toolBlob: string | null,
): { text: string; tool_calls_ref: string | null } {
  const trimmed = rawBlob.trim();
  if (!trimmed) {
    return { text: "", tool_calls_ref: null };
  }

  try {
    const parsed = JSON.parse(trimmed) as unknown;
    const envelope = asRecord(parsed);
    if (!envelope) {
      return { text: trimmed, tool_calls_ref: null };
    }

    const assistant_text = asString(envelope.assistant_text) ?? "";
    const tool_calls_ref = asString(envelope.tool_calls_ref);
    if (assistant_text.length > 0) {
      return { text: assistant_text, tool_calls_ref };
    }

    if (tool_calls_ref) {
      const names = toolBlob ? extractToolNames(toolBlob) : [];
      const text =
        names.length > 0
          ? `Requested tool calls: ${names.join(", ")}`
          : "Requested tool calls.";
      return { text, tool_calls_ref };
    }

    return { text: trimmed, tool_calls_ref: null };
  } catch {
    return { text: trimmed, tool_calls_ref: null };
  }
}

async function loadChatTranscript(chatId: string): Promise<ChatTranscript> {
  const journal = await endpoints.journalTail({ limit: 1200 });
  const runRequests: RunRequestRecord[] = [];
  const llmIntents: LlmIntentRecord[] = [];
  const llmReceiptsByHash = new Map<string, LlmReceiptRecord>();

  for (const rawEntry of journal.entries) {
    const entry = asRecord(rawEntry);
    if (!entry) continue;

    const kind = asString(entry.kind);
    const seq = asNumber(entry.seq);
    const record = asRecord(entry.record);
    if (!kind || seq == null || !record) continue;

    if (kind === "domain_event") {
      if (asString(record.schema) !== "aos.agent/SessionIngress@1") continue;
      const domain = decodeCborFromJournal<JsonRecord>(record.value);
      if (!domain) continue;
      if (asString(domain.session_id) !== chatId) continue;

      const ingress = asRecord(domain.ingress);
      if (!ingress || asString(ingress.$tag) !== "RunRequested") continue;
      const payload = asRecord(ingress.$value);
      const input_ref = asString(payload?.input_ref);
      if (!input_ref) continue;

      runRequests.push({
        seq,
        observed_at_ns: asNumber(domain.observed_at_ns) ?? seq,
        input_ref,
      });
      continue;
    }

    if (kind === "effect_intent") {
      if (asString(record.kind) !== "llm.generate") continue;
      const origin = asRecord(record.origin);
      if (!origin) continue;
      if (asString(origin.origin_kind) !== "workflow") continue;
      if (asString(origin.name) !== "demiurge/Demiurge@1") continue;

      const instance_key = decodeInstanceKey(origin.instance_key);
      if (instance_key !== chatId) continue;

      const intent_hash = cborHashToHex(record.intent_hash);
      if (!intent_hash) continue;
      llmIntents.push({ seq, intent_hash });
      continue;
    }

    if (kind === "effect_receipt") {
      const intent_hash = cborHashToHex(record.intent_hash);
      if (!intent_hash) continue;

      const payload = decodeCborFromJournal<JsonRecord>(record.payload_cbor);
      const tokenUsageRecord = asRecord(payload?.token_usage);
      const token_usage =
        tokenUsageRecord &&
        asNumber(tokenUsageRecord.prompt) != null &&
        asNumber(tokenUsageRecord.completion) != null
          ? {
              prompt: asNumber(tokenUsageRecord.prompt) ?? 0,
              completion: asNumber(tokenUsageRecord.completion) ?? 0,
              total: asNumber(tokenUsageRecord.total),
            }
          : null;

      llmReceiptsByHash.set(intent_hash, {
        seq,
        intent_hash,
        status: (asString(record.status) ?? "error").toLowerCase(),
        output_ref: asString(payload?.output_ref),
        tool_calls_ref: null,
        assistant_text: null,
        token_usage,
      });
    }
  }

  const orderedReceipts = llmIntents
    .map((intent) => llmReceiptsByHash.get(intent.intent_hash))
    .filter((receipt): receipt is LlmReceiptRecord => Boolean(receipt))
    .sort((a, b) => a.seq - b.seq);

  const refs = new Set<string>();
  for (const run of runRequests) refs.add(run.input_ref);
  for (const receipt of orderedReceipts) {
    if (receipt.output_ref) refs.add(receipt.output_ref);
  }

  const blobTextByRef = new Map<string, string>();
  await Promise.all(
    [...refs].map(async (ref) => {
      try {
        const bytes = new Uint8Array(await endpoints.blobGet(ref));
        blobTextByRef.set(ref, UTF8_DECODER.decode(bytes));
      } catch {
        // Keep transcript resilient when blobs are absent.
      }
    }),
  );

  const extraToolRefs = new Set<string>();
  for (const receipt of orderedReceipts) {
    if (!receipt.output_ref) continue;
    const rawOutput = blobTextByRef.get(receipt.output_ref);
    if (!rawOutput) continue;
    const parsed = extractAssistantMessage(rawOutput, null);
    receipt.assistant_text = parsed.text;
    receipt.tool_calls_ref = parsed.tool_calls_ref;
    if (parsed.tool_calls_ref) extraToolRefs.add(parsed.tool_calls_ref);
  }

  await Promise.all(
    [...extraToolRefs].map(async (ref) => {
      if (blobTextByRef.has(ref)) return;
      try {
        const bytes = new Uint8Array(await endpoints.blobGet(ref));
        blobTextByRef.set(ref, UTF8_DECODER.decode(bytes));
      } catch {
        // Ignore missing optional tool blobs.
      }
    }),
  );

  for (const receipt of orderedReceipts) {
    if (!receipt.output_ref) continue;
    const rawOutput = blobTextByRef.get(receipt.output_ref);
    if (!rawOutput) continue;
    const rawToolBlob = receipt.tool_calls_ref
      ? (blobTextByRef.get(receipt.tool_calls_ref) ?? null)
      : null;
    const parsed = extractAssistantMessage(rawOutput, rawToolBlob);
    receipt.assistant_text = parsed.text;
    receipt.tool_calls_ref = parsed.tool_calls_ref;
  }

  const sortedRuns = [...runRequests].sort((a, b) => a.seq - b.seq);
  const messages: ChatTranscriptMessage[] = [];
  for (let i = 0; i < sortedRuns.length; i += 1) {
    const run = sortedRuns[i];
    const nextRunSeq = sortedRuns[i + 1]?.seq ?? Number.POSITIVE_INFINITY;
    const rawInput = blobTextByRef.get(run.input_ref) ?? "";
    const userText = extractLatestUserText(rawInput);

    messages.push({
      id: `user-${run.seq}`,
      role: "user",
      text: userText,
      observed_at_ns: run.observed_at_ns,
      input_ref: run.input_ref,
    });

    const receipt = orderedReceipts.find(
      (candidate) => candidate.seq > run.seq && candidate.seq < nextRunSeq,
    );
    if (!receipt) continue;

    const assistantText = receipt.assistant_text ?? "";
    if (!assistantText && !receipt.output_ref) continue;
    messages.push({
      id: `assistant-${receipt.seq}`,
      role: "assistant",
      text: assistantText || "(empty response)",
      observed_at_ns: receipt.seq,
      output_ref: receipt.output_ref,
      tool_calls_ref: receipt.tool_calls_ref,
      token_usage: receipt.token_usage,
      failed: receipt.status !== "ok",
    });
  }

  return { messages };
}

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

export function useDebugTrace(
  params: DebugTraceQuery,
  options?: QueryOptions<
    DebugTraceResponse,
    ReturnType<typeof queryKeys.debugTrace>
  >,
) {
  return useQuery({
    queryKey: queryKeys.debugTrace(params),
    queryFn: () => endpoints.debugTrace(params),
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
        { workflow: "demiurge/Demiurge@1" },
        { key_b64: encodeCborTextToBase64(chatId) },
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
    queryFn: () => endpoints.stateCells({ workflow: "demiurge/Demiurge@1" }),
    refetchInterval: 5000,
    ...options,
  });
}

export function useChatTranscript(
  chatId: string,
  options?: QueryOptions<ChatTranscript, ReturnType<typeof queryKeys.chatTranscript>>,
) {
  return useQuery({
    queryKey: queryKeys.chatTranscript(chatId),
    queryFn: () => loadChatTranscript(chatId),
    enabled: Boolean(chatId),
    refetchInterval: 2000,
    ...options,
  });
}
