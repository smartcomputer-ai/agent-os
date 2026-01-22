import { useMemo } from "react";
import { useBlobGet } from "@/sdk/queries";
import { cn } from "@/lib/utils";
import type { ChatMessage as ChatMessageType } from "../types";

interface ChatMessageProps {
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

export function ChatMessage({ message }: ChatMessageProps) {
  const isUser = message.role.$tag === "User";
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
        {message.token_usage && (
          <div className="text-xs text-muted-foreground mt-2">
            {message.token_usage.prompt + message.token_usage.completion} tokens
          </div>
        )}
      </div>
    </div>
  );
}
