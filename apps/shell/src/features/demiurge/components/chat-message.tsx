import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import type { ChatMessage as ChatMessageType } from "../types";

interface ChatMessageProps {
  message: ChatMessageType;
  isLatest?: boolean;
}

function formatTokenUsage(message: ChatMessageType): string | null {
  if (!message.token_usage) return null;
  const total =
    message.token_usage.total ??
    message.token_usage.prompt + message.token_usage.completion;
  return `${total} tokens`;
}

export function ChatMessage({ message, isLatest = false }: ChatMessageProps) {
  const isUser = message.role === "user";
  const usage = formatTokenUsage(message);

  return (
    <div
      className={cn(
        "flex gap-3 rounded-lg p-3",
        isUser ? "bg-primary/10 ml-12" : "bg-card/80 mr-12",
      )}
      data-latest={isLatest ? "true" : "false"}
    >
      <div className="flex-1">
        <div className="mb-1 flex items-center justify-between gap-2">
          <div className="text-xs text-muted-foreground">
            {isUser ? "You" : "Assistant"}
          </div>
          <div className="flex items-center gap-2">
            {message.failed && (
              <Badge variant="destructive" className="text-[10px]">
                failed
              </Badge>
            )}
            {usage && (
              <span className="text-[11px] text-muted-foreground">{usage}</span>
            )}
          </div>
        </div>
        <div className="text-sm whitespace-pre-wrap">{message.text}</div>
      </div>
    </div>
  );
}
