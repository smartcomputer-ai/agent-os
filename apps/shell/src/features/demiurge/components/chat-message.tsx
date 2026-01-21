import { useMemo } from "react";
import { useBlobGet } from "@/sdk/queries";
import { cn } from "@/lib/utils";
import type { ChatMessage as ChatMessageType, MessageBlob } from "../types";

interface ChatMessageProps {
  message: ChatMessageType;
  isLatest?: boolean;
}

export function ChatMessage({ message }: ChatMessageProps) {
  const isUser = message.role.$tag === "User";
  const { data: blobData } = useBlobGet(message.message_ref ?? "", {
    enabled: !!message.message_ref,
  });

  const messageText = useMemo(() => {
    if (blobData) {
      try {
        const blob = JSON.parse(
          new TextDecoder().decode(blobData),
        ) as MessageBlob;
        const textPart = blob.content.find((p) => p.type === "text");
        if (textPart && textPart.type === "text") {
          return textPart.text;
        }
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
