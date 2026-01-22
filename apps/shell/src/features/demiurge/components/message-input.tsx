import { useState, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { workspaceList, workspaceReadRef } from "@/sdk/endpoints";
import { useBlobPut, useEventsPost } from "@/sdk/mutations";
import { createMessageBlob, encodeMessageBlob } from "../lib/message-utils";
import type { ChatSettings } from "../types";
import { cn } from "@/lib/utils";

const TOOL_WORKSPACE = "demiurge";
const TOOL_FOLDER = "tools";

async function loadToolRefs(): Promise<string[] | null> {
  try {
    const list = await workspaceList({
      workspace: TOOL_WORKSPACE,
      path: TOOL_FOLDER,
      limit: 100,
    });
    const refs: string[] = [];
    for (const entry of list.entries) {
      if (entry.kind !== "file") continue;
      const ref = await workspaceReadRef({
        workspace: TOOL_WORKSPACE,
        path: entry.path,
      });
      if (ref?.hash) {
        refs.push(ref.hash);
      }
    }
    return refs.length ? refs : null;
  } catch (error) {
    console.warn("Failed to load tool refs from workspace:", error);
    return null;
  }
}

interface MessageInputProps {
  chatId: string;
  lastRequestId: number;
  onMessageSent?: () => void;
  disabled?: boolean;
  settings: ChatSettings;
  variant?: "floating" | "fixed";
}

export function MessageInput({
  chatId,
  lastRequestId,
  onMessageSent,
  disabled,
  settings,
  variant = "fixed",
}: MessageInputProps) {
  const [message, setMessage] = useState("");
  const [isSending, setIsSending] = useState(false);

  const blobPutMutation = useBlobPut();
  const eventsPostMutation = useEventsPost();

  const handleSend = async () => {
    if (!message.trim() || isSending || disabled) return;

    setIsSending(true);
    try {
      const blob = createMessageBlob(message.trim(), "user");
      const blobBytes = encodeMessageBlob(blob);
      const base64Data = btoa(String.fromCharCode(...new Uint8Array(blobBytes)));

      const blobResult = await blobPutMutation.mutateAsync({
        data_b64: base64Data,
      });

      const requestId = lastRequestId + 1;
      const toolRefs = await loadToolRefs();
      const toolChoice = toolRefs ? { $tag: "Auto" as const } : null;

      await eventsPostMutation.mutateAsync({
        schema: "demiurge/ChatEvent@1",
        value: {
          $tag: "UserMessage",
          $value: {
            chat_id: chatId,
            request_id: requestId,
            text: message.trim(),
            message_ref: blobResult.hash,
            model: settings.model,
            provider: settings.provider,
            max_tokens: settings.max_tokens,
            tool_refs: toolRefs,
            tool_choice: toolChoice,
          },
        },
      });

      setMessage("");
      onMessageSent?.();
    } catch (error) {
      console.error("Failed to send message:", error);
    } finally {
      setIsSending(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div
      className={cn(
        "relative rounded-2xl bg-background shadow-lg border",
        variant === "floating" && "shadow-xl"
      )}
    >
      <Textarea
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Type a message... (Enter to send, Shift+Enter for new line)"
        disabled={disabled || isSending}
        className={cn(
          "resize-none border-0 focus-visible:ring-0 rounded-2xl pr-20",
          variant === "floating" ? "min-h-30" : "min-h-15 max-h-50"
        )}
      />
      <Button
        onClick={handleSend}
        disabled={!message.trim() || disabled || isSending}
        className="absolute bottom-2 right-2 rounded-xl"
        size="sm"
      >
        {isSending ? "Sending..." : "Send"}
      </Button>
    </div>
  );
}
