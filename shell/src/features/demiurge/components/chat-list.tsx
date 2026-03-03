import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { displayKeyFromBase64 } from "@/sdk/cbor";
import { cn } from "@/lib/utils";

interface StateCell {
  key_b64: string;
  last_active_ns: number;
  size: number;
  state_hash_hex: string;
}

interface ChatListProps {
  chats: StateCell[];
  selectedChatId: string | null;
  onChatSelect: (chatId: string) => void;
}

export function ChatList({ chats, selectedChatId, onChatSelect }: ChatListProps) {
  if (chats.length === 0) {
    return (
      <div className="text-center py-8 text-muted-foreground text-sm">
        No chats yet. Create one to get started.
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {chats.map((chat) => {
        const chatId = displayKeyFromBase64(chat.key_b64);
        const lastActiveDate = new Date(chat.last_active_ns / 1_000_000);
        const timeAgo = getTimeAgo(lastActiveDate);
        const isSelected = chatId === selectedChatId;

        return (
          <Card
            key={chat.key_b64}
            className={cn(
              "cursor-pointer transition-colors hover:bg-accent",
              isSelected && "bg-accent border-primary"
            )}
            onClick={() => onChatSelect(chatId)}
          >
            <CardContent className="p-3">
              <div className="space-y-2">
                <div className="font-medium text-sm truncate">
                  {chatId}
                </div>
                <div className="flex items-center gap-2">
                  <Badge variant="secondary" className="text-xs">
                    {chat.size}b
                  </Badge>
                  <span className="text-xs text-muted-foreground">
                    {timeAgo}
                  </span>
                </div>
              </div>
            </CardContent>
          </Card>
        );
      })}
    </div>
  );
}

function getTimeAgo(date: Date): string {
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
