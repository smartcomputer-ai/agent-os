import { useState, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { useBlobPut, useEventsPost } from "@/sdk/mutations";
import { buildRunHistoryPayload, encodeJsonBlob } from "../lib/message-utils";
import type { ChatMessage, ChatSettings, SessionIngress } from "../types";
import { cn } from "@/lib/utils";

const MAX_TOKENS_CAP = 4096;

interface MessageInputProps {
  chatId: string;
  messages: ChatMessage[];
  onMessageSent?: () => void;
  disabled?: boolean;
  settings: ChatSettings;
  variant?: "floating" | "fixed";
}

export function MessageInput({
  chatId,
  messages,
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
      const historyPayload = buildRunHistoryPayload(messages, message.trim());
      const blobBytes = encodeJsonBlob(historyPayload);
      const base64Data = btoa(String.fromCharCode(...new Uint8Array(blobBytes)));

      const blobResult = await blobPutMutation.mutateAsync({
        data_b64: base64Data,
      });

      const ingress: SessionIngress = {
        session_id: chatId,
        observed_at_ns: Date.now(),
        ingress: {
          $tag: "RunRequested",
          $value: {
            input_ref: blobResult.hash,
            run_overrides: {
              provider: settings.provider,
              model: settings.model,
              max_tokens: Math.min(settings.max_tokens, MAX_TOKENS_CAP),
            },
          },
        },
      };

      await eventsPostMutation.mutateAsync({
        schema: "aos.agent/SessionIngress@1",
        value: ingress,
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
