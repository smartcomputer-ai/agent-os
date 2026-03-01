import type { ChatMessage, SessionLifecycle } from "../types";

export function encodeJsonBlob(value: unknown): ArrayBuffer {
  return new TextEncoder().encode(JSON.stringify(value)).buffer;
}

export function buildRunHistoryPayload(
  messages: ChatMessage[],
  nextUserText: string,
): Array<Record<string, unknown>> {
  const history = messages
    .filter((message) => message.role === "user" || message.role === "assistant")
    .map((message) => ({
      role: message.role,
      content: message.text,
    }));

  history.push({
    role: "user",
    content: nextUserText,
  });

  return history;
}

export function generateSessionId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `session-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

export function lifecycleTag(lifecycle: SessionLifecycle | undefined): string {
  return lifecycle?.$tag ?? "Idle";
}
