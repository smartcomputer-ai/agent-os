import { useState, type KeyboardEvent } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { useBlobPut, useEventsPost } from "@/sdk/mutations";
import { createMessageBlob, encodeMessageBlob } from "../lib/message-utils";
import type { ChatSettings } from "../types";

interface MessageInputProps {
  chatId: string;
  lastRequestId: number;
  onMessageSent?: () => void;
  disabled?: boolean;
  settings: ChatSettings;
}

export function MessageInput({
  chatId,
  lastRequestId,
  onMessageSent,
  disabled,
  settings,
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
    <div className="flex gap-2">
      <Textarea
        value={message}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder="Type a message... (Enter to send, Shift+Enter for new line)"
        disabled={disabled || isSending}
        className="min-h-[60px] max-h-[200px] resize-none"
      />
      <Button
        onClick={handleSend}
        disabled={!message.trim() || disabled || isSending}
        className="self-end"
      >
        {isSending ? "Sending..." : "Send"}
      </Button>
    </div>
  );
}
