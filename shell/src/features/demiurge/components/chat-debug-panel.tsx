import { useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { useDebugTrace, useJournalTail } from "@/sdk/queries";

function findString(node: unknown, key: string): string | undefined {
  if (node && typeof node === "object") {
    const obj = node as Record<string, unknown>;
    const direct = obj[key];
    if (typeof direct === "string") {
      return direct;
    }
    for (const value of Object.values(obj)) {
      const found = findString(value, key);
      if (found) {
        return found;
      }
    }
  }
  if (Array.isArray(node)) {
    for (const value of node) {
      const found = findString(value, key);
      if (found) {
        return found;
      }
    }
  }
  return undefined;
}

function getTraceEventHash(entries: unknown[]): string | undefined {
  const prioritySchemas = [
    "aos.agent/SessionIngress@1",
    "demiurge/ToolCallRequested@1",
  ];
  for (const schema of prioritySchemas) {
    for (let i = entries.length - 1; i >= 0; i -= 1) {
      const raw = entries[i];
      if (!raw || typeof raw !== "object") {
        continue;
      }
      const entry = raw as Record<string, unknown>;
      if (entry.kind !== "domain_event") {
        continue;
      }
      const record = entry.record;
      const entrySchema = findString(record, "schema");
      const eventHash = findString(record, "event_hash");
      if (entrySchema === schema && eventHash?.startsWith("sha256:")) {
        return eventHash;
      }
    }
  }
  return undefined;
}

function asArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function terminalIsWaiting(terminal: string): boolean {
  return terminal === "waiting_receipt" || terminal === "waiting_event";
}

export function ChatDebugPanel() {
  const [open, setOpen] = useState(false);
  const journalQuery = useJournalTail(
    { limit: 400 },
    { enabled: open, refetchInterval: 3000 },
  );

  const traceEventHash = useMemo(() => {
    const entries = journalQuery.data?.entries ?? [];
    return getTraceEventHash(entries);
  }, [journalQuery.data]);

  const traceQuery = useDebugTrace(
    { event_hash: traceEventHash ?? "", window_limit: 400 },
    {
      enabled: open && Boolean(traceEventHash),
      refetchInterval: (query) => {
        const terminal = (query.state.data?.terminal_state as string | undefined) ?? "unknown";
        return terminalIsWaiting(terminal) ? 2000 : false;
      },
    },
  );

  const terminalState = (traceQuery.data?.terminal_state as string | undefined) ?? "unknown";
  const root = (traceQuery.data?.root as Record<string, unknown> | undefined) ?? {};
  const journalEntries = asArray(
    (traceQuery.data?.journal_window as Record<string, unknown> | undefined)?.entries,
  );
  const liveWait = (traceQuery.data?.live_wait as Record<string, unknown> | undefined) ?? {};
  const pendingWorkflowReceipts = asArray(liveWait.pending_workflow_receipts).length;
  const queuedEffects = asArray(liveWait.queued_effects).length;
  const waitingWorkflowInstances = asArray(liveWait.workflow_instances).filter((instance) => {
    if (!instance || typeof instance !== "object") {
      return false;
    }
    const status = (instance as Record<string, unknown>).status;
    return status === "running" || status === "waiting";
  }).length;

  return (
    <div className="max-w-4xl mx-auto mt-2 mb-3">
      <div className="flex items-center justify-between rounded-md border bg-card/60 p-2">
        <div className="text-xs text-muted-foreground">
          Debug: terminal={terminalState}
          {traceEventHash ? ` event=${traceEventHash}` : " (no event hash yet)"}
        </div>
        <Button size="sm" variant="outline" onClick={() => setOpen((v) => !v)}>
          {open ? "Hide Debug" : "Show Debug"}
        </Button>
      </div>

      {open && (
        <div className="mt-2 rounded-md border bg-background p-3 text-xs space-y-2">
          {journalQuery.isLoading && <div>Loading journal...</div>}
          {journalQuery.error && (
            <div className="text-destructive">journal error: {journalQuery.error.message}</div>
          )}
          {!traceEventHash && !journalQuery.isLoading && (
            <div>No trace candidate event found yet.</div>
          )}
          {traceQuery.isLoading && traceEventHash && <div>Loading trace...</div>}
          {traceQuery.error && (
            <div className="text-destructive">trace error: {traceQuery.error.message}</div>
          )}

          {traceQuery.data && (
            <>
              <div>
                root schema={(root.schema as string | undefined) ?? "?"} seq=
                {(root.seq as number | undefined) ?? 0} key=
                {(root.key_b64 as string | undefined) ?? "-"}
              </div>
              <div>
                waits workflow_receipts={pendingWorkflowReceipts} queued_effects=
                {queuedEffects} workflow_instances={waitingWorkflowInstances}
              </div>
              <div className="max-h-48 overflow-auto rounded border bg-muted/30 p-2 font-mono">
                {journalEntries.slice(-30).map((raw, idx) => {
                  if (!raw || typeof raw !== "object") {
                    return null;
                  }
                  const entry = raw as Record<string, unknown>;
                  const seq = (entry.seq as number | undefined) ?? 0;
                  const kind = (entry.kind as string | undefined) ?? "unknown";
                  return (
                    <div key={`${seq}-${kind}-${idx}`}>
                      #{seq} {kind}
                    </div>
                  );
                })}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  );
}
