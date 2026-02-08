import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { useBlobGet } from "@/sdk/queries";
import { useDebugTrace } from "@/sdk/queries";
import { cn } from "@/lib/utils";
import type { ChatMessage as ChatMessageType } from "../types";

interface ChatMessageProps {
  chatId: string;
  message: ChatMessageType;
  isLatest?: boolean;
}

function extractTextFromContent(content: unknown): string[] {
  if (typeof content === "string") return [content];
  if (!Array.isArray(content)) return [];
  const parts: string[] = [];
  for (const part of content) {
    if (!part || typeof part !== "object") continue;
    const type = (part as { type?: string }).type;
    if (type !== "text" && type !== "input_text" && type !== "output_text") continue;
    const text = (part as { text?: string }).text;
    if (typeof text === "string" && text.length > 0) {
      parts.push(text);
    }
  }
  return parts;
}

function extractTextFromBlob(blob: unknown): string | null {
  if (!blob) return null;
  if (Array.isArray(blob)) {
    const parts: string[] = [];
    for (const item of blob) {
      if (!item || typeof item !== "object") continue;
      const type = (item as { type?: string }).type;
      if (type === "function_call_output") {
        const output = (item as { output?: unknown }).output;
        if (typeof output === "string" && output.length > 0) {
          parts.push(output);
        }
        continue;
      }
      const content = (item as { content?: unknown }).content;
      parts.push(...extractTextFromContent(content));
    }
    return parts.length ? parts.join("\n\n") : null;
  }
  if (typeof blob === "object") {
    const type = (blob as { type?: string }).type;
    if (type === "function_call_output") {
      const output = (blob as { output?: unknown }).output;
      if (typeof output === "string" && output.length > 0) {
        return output;
      }
    }
    const content = (blob as { content?: unknown }).content;
    const parts = extractTextFromContent(content);
    return parts.length ? parts.join("\n\n") : null;
  }
  return null;
}

function lower(value: unknown): string {
  return typeof value === "string" ? value.toLowerCase() : "";
}

function findHint(trace: Record<string, unknown>): string | null {
  const terminalState = lower(trace.terminal_state);
  const liveWait =
    (trace.live_wait as Record<string, unknown> | undefined) ?? {};
  const entries =
    ((trace.journal_window as Record<string, unknown> | undefined)?.entries as
      | unknown[]
      | undefined) ?? [];

  for (const raw of entries) {
    if (!raw || typeof raw !== "object") continue;
    const entry = raw as Record<string, unknown>;
    const kind = lower(entry.kind);
    const record = (entry.record as Record<string, unknown> | undefined) ?? {};
    const decision = lower(record.decision);
    if (kind === "cap_decision" && decision === "deny") {
      return "Capability denied this effect intent.";
    }
    if (kind === "policy_decision" && decision === "deny") {
      return "Policy denied this effect intent.";
    }
    if (kind === "effect_receipt") {
      const status = lower(record.status);
      if (status === "timeout") return "Adapter timed out while executing effect.";
      if (status === "error") return "Adapter returned an error receipt.";
    }
  }

  if (terminalState === "waiting_receipt") {
    const pending =
      ((liveWait.pending_plan_receipts as unknown[] | undefined)?.length ?? 0) +
      ((liveWait.pending_reducer_receipts as unknown[] | undefined)?.length ?? 0) +
      ((liveWait.queued_effects as unknown[] | undefined)?.length ?? 0);
    if (pending > 0) {
      return "Plan is waiting for effect receipts.";
    }
  }
  if (terminalState === "waiting_event") {
    return "Plan is waiting for a follow-up domain event.";
  }
  if (terminalState === "failed") {
    return "Trace ended in failed state. Check timeline entries below.";
  }
  return null;
}

function firstIntentHash(liveWait: Record<string, unknown>): string | null {
  const read = (list: unknown): string | null => {
    if (!Array.isArray(list)) return null;
    for (const item of list) {
      if (!item || typeof item !== "object") continue;
      const hash = (item as Record<string, unknown>).intent_hash;
      if (typeof hash === "string" && hash.length > 0) return hash;
      const hashes = (item as Record<string, unknown>).intent_hashes;
      if (Array.isArray(hashes) && typeof hashes[0] === "string") {
        return hashes[0];
      }
    }
    return null;
  };
  return (
    read(liveWait.pending_plan_receipts) ??
    read(liveWait.pending_reducer_receipts) ??
    read(liveWait.plan_waiting_receipts)
  );
}

function CopyValueButton({ label, value }: { label: string; value: string | null }) {
  if (!value) return null;
  return (
    <Button
      size="sm"
      variant="outline"
      onClick={() => {
        if (navigator.clipboard) {
          void navigator.clipboard.writeText(value);
        }
      }}
      className="text-[11px] h-6"
    >
      Copy {label}
    </Button>
  );
}

export function ChatMessage({ chatId, message }: ChatMessageProps) {
  const isUser = message.role.$tag === "User";
  const [debugOpen, setDebugOpen] = useState(false);
  const { data: blobData } = useBlobGet(message.message_ref ?? "", {
    enabled: !!message.message_ref,
  });

  const messageText = useMemo(() => {
    if (blobData) {
      try {
        const blob = JSON.parse(new TextDecoder().decode(blobData)) as unknown;
        const text = extractTextFromBlob(blob);
        if (text) return text;
      } catch (e) {
        console.error("Failed to decode message blob:", e);
      }
    }
    return message.text;
  }, [blobData, message.text]);

  const correlateBy = message.message_ref
    ? "$value.message_ref"
    : "$value.request_id";
  const correlateValue = message.message_ref ?? message.request_id;
  const traceQuery = useDebugTrace(
    {
      schema: "demiurge/ChatEvent@1",
      correlate_by: correlateBy,
      value: JSON.stringify(correlateValue),
      window_limit: 300,
    },
    {
      enabled: debugOpen,
      refetchInterval: (query) => {
        const terminal = lower(query.state.data?.terminal_state);
        return terminal === "waiting_receipt" || terminal === "waiting_event"
          ? 2000
          : false;
      },
    },
  );
  const trace = (traceQuery.data ?? {}) as Record<string, unknown>;
  const root = (trace.root as Record<string, unknown> | undefined) ?? {};
  const liveWait = (trace.live_wait as Record<string, unknown> | undefined) ?? {};
  const journalEntries =
    ((trace.journal_window as Record<string, unknown> | undefined)?.entries as
      | unknown[]
      | undefined) ?? [];
  const hint = findHint(trace);
  const eventHash =
    (root.event_hash as string | undefined) ??
    (trace.query as Record<string, unknown> | undefined)?.event_hash?.toString();
  const intentHash = firstIntentHash(liveWait);

  return (
    <div
      className={cn(
        "flex gap-3 p-3 rounded-lg",
        isUser ? "bg-primary/10 ml-12" : "bg-card/80 mr-12",
      )}
    >
      <div className="flex-1">
        <div className="text-xs text-muted-foreground mb-1">
          {isUser ? "You" : "Assistant"}
        </div>
        <div className="text-sm whitespace-pre-wrap">{messageText}</div>
        <div className="mt-2 flex items-center gap-2">
          <Button
            size="sm"
            variant="outline"
            onClick={() => setDebugOpen((v) => !v)}
            className="text-[11px] h-6"
          >
            {debugOpen ? "Hide Debug" : "Debug"}
          </Button>
          <span className="text-[11px] text-muted-foreground">
            chat={chatId} request={message.request_id}
          </span>
        </div>
        {debugOpen && (
          <div className="mt-2 rounded border bg-muted/20 p-2 space-y-2">
            {traceQuery.isLoading && (
              <div className="text-xs text-muted-foreground">Loading trace...</div>
            )}
            {traceQuery.error && (
              <div className="text-xs text-destructive">
                trace error: {traceQuery.error.message}
              </div>
            )}
            {traceQuery.data && (
              <>
                <div className="text-xs">
                  terminal={String(trace.terminal_state ?? "unknown")} schema=
                  {String(root.schema ?? "unknown")} seq={String(root.seq ?? 0)}
                </div>
                {hint && <div className="text-xs text-amber-700">{hint}</div>}
                <div className="flex flex-wrap gap-2">
                  <CopyValueButton label="event hash" value={eventHash ?? null} />
                  <CopyValueButton label="intent hash" value={intentHash} />
                  <CopyValueButton label="output ref" value={message.message_ref} />
                </div>
                <div className="text-xs text-muted-foreground">
                  waits plan_receipts=
                  {Array.isArray(liveWait.pending_plan_receipts)
                    ? liveWait.pending_plan_receipts.length
                    : 0}{" "}
                  waiting_events=
                  {Array.isArray(liveWait.plan_waiting_events)
                    ? liveWait.plan_waiting_events.length
                    : 0}{" "}
                  reducer_receipts=
                  {Array.isArray(liveWait.pending_reducer_receipts)
                    ? liveWait.pending_reducer_receipts.length
                    : 0}{" "}
                  queued_effects=
                  {Array.isArray(liveWait.queued_effects)
                    ? liveWait.queued_effects.length
                    : 0}
                </div>
                <div className="max-h-40 overflow-auto rounded border bg-background p-2 text-[11px] font-mono">
                  {journalEntries.slice(-20).map((entry, idx) => {
                    if (!entry || typeof entry !== "object") return null;
                    const e = entry as Record<string, unknown>;
                    return (
                      <div key={`${idx}-${String(e.seq)}-${String(e.kind)}`}>
                        #{String(e.seq ?? 0)} {String(e.kind ?? "unknown")}
                      </div>
                    );
                  })}
                </div>
              </>
            )}
          </div>
        )}
        {message.token_usage && (
          <div className="text-xs text-muted-foreground mt-2">
            {message.token_usage.prompt + message.token_usage.completion} tokens
          </div>
        )}
      </div>
    </div>
  );
}
