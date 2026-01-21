import { Link } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { displayKeyFromBase64 } from "@/sdk/cbor";

interface StateCell {
  key_b64: string;
  last_active_ns: number;
  size: number;
  state_hash_hex: string;
}

interface ChatListProps {
  chats: StateCell[];
}

export function ChatList({ chats }: ChatListProps) {
  if (chats.length === 0) {
    return (
      <Card className="bg-card/80">
        <CardContent className="py-12">
          <div className="text-center space-y-2">
            <div className="text-muted-foreground">No chats yet</div>
            <div className="text-sm text-muted-foreground">
              Create a new chat to get started
            </div>
          </div>
        </CardContent>
      </Card>
    );
  }

  return (
    <div className="space-y-3">
      {chats.map((chat) => {
        const chatId = displayKeyFromBase64(chat.key_b64);
        const lastActiveDate = new Date(chat.last_active_ns / 1_000_000);
        const timeAgo = getTimeAgo(lastActiveDate);

        return (
          <Link key={chat.key_b64} to={`/chat/${chatId}`}>
            <Card className="bg-card/80 hover:bg-card transition-colors cursor-pointer">
              <CardContent className="p-4">
                <div className="flex items-start justify-between gap-3">
                  <div className="flex-1 min-w-0">
                    <div className="font-medium text-foreground truncate">
                      {chatId}
                    </div>
                    <div className="flex items-center gap-2 mt-2">
                      <Badge variant="secondary">{chat.size} bytes</Badge>
                      <span className="text-xs text-muted-foreground">
                        {timeAgo}
                      </span>
                    </div>
                  </div>
                </div>
              </CardContent>
            </Card>
          </Link>
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
