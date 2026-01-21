import type { MessageBlob } from "../types";

export function createMessageBlob(
  text: string,
  role: "user" | "assistant",
): MessageBlob {
  return {
    role,
    content: [{ type: "text", text }],
  };
}

export function encodeMessageBlob(blob: MessageBlob): ArrayBuffer {
  const json = JSON.stringify(blob);
  return new TextEncoder().encode(json).buffer;
}

export function generateChatId(): string {
  return `chat-${Date.now()}-${Math.random().toString(36).slice(2, 9)}`;
}

export function formatTimestamp(ms: number | null): string {
  if (!ms) return "Unknown";
  const date = new Date(ms);
  const now = new Date();
  const diffMs = now.getTime() - date.getTime();
  const diffMins = Math.floor(diffMs / 60000);

  if (diffMins < 1) return "Just now";
  if (diffMins < 60) return `${diffMins}m ago`;
  const diffHours = Math.floor(diffMins / 60);
  if (diffHours < 24) return `${diffHours}h ago`;
  const diffDays = Math.floor(diffHours / 24);
  return `${diffDays}d ago`;
}
