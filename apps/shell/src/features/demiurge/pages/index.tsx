import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useChatList } from "@/sdk/queries";
import { useEventsPost } from "@/sdk/mutations";
import { ChatList } from "../components/chat-list";
import { generateChatId } from "../lib/message-utils";

export function ChatsIndexPage() {
  const navigate = useNavigate();
  const { data: chatsData, isLoading, error } = useChatList();
  const eventsPostMutation = useEventsPost();
  const [isCreating, setIsCreating] = useState(false);

  const handleNewChat = async () => {
    setIsCreating(true);
    try {
      const chatId = generateChatId();
      const now = Date.now();

      await eventsPostMutation.mutateAsync({
        schema: "demiurge/ChatEvent@1",
        value: {
          $tag: "ChatCreated",
          $value: {
            chat_id: chatId,
            title: `Chat ${new Date(now).toLocaleString()}`,
            created_at_ms: now,
          },
        },
      });

      navigate(`/chat/${chatId}`);
    } catch (error) {
      console.error("Failed to create chat:", error);
    } finally {
      setIsCreating(false);
    }
  };

  const chats = chatsData?.cells ?? [];
  const totalSize = chats.reduce(
    (sum: number, chat) => sum + chat.size,
    0,
  );

  return (
    <div className="min-h-[calc(100dvh-7.5rem)] space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            Demiurge Chat
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Chat with AI agents using LLM capabilities. Create conversations and
            get intelligent responses.
          </p>
        </div>
        <Button onClick={handleNewChat} disabled={isCreating}>
          {isCreating ? "Creating..." : "New Chat"}
        </Button>
      </header>

      {error && (
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center space-y-2">
              <div className="text-destructive">Failed to load chats</div>
              <div className="text-sm text-muted-foreground">
                {error.message}
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {isLoading && (
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center text-muted-foreground">
              Loading chats...
            </div>
          </CardContent>
        </Card>
      )}

      {!isLoading && !error && (
        <div className="grid gap-4 lg:grid-cols-[1.3fr_0.7fr]">
          <div className="space-y-4">
            <Card className="bg-card/80">
              <CardHeader>
                <CardTitle className="text-lg">Your Chats</CardTitle>
                <CardDescription>
                  Recent conversations and message history
                </CardDescription>
              </CardHeader>
              <CardContent>
                <ChatList chats={chats} />
              </CardContent>
            </Card>
          </div>

          <div className="space-y-4">
            <Card className="bg-card/80">
              <CardHeader>
                <CardTitle className="text-lg">Stats</CardTitle>
                <CardDescription>Quick overview</CardDescription>
              </CardHeader>
              <CardContent className="space-y-3 text-sm">
                <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                  <span>Total chats</span>
                  <Badge variant="secondary">{chats.length}</Badge>
                </div>
                <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                  <span>Total size</span>
                  <Badge variant="outline">{totalSize} bytes</Badge>
                </div>
              </CardContent>
            </Card>

            <Card className="bg-card/80">
              <CardHeader>
                <CardTitle className="text-lg">About</CardTitle>
                <CardDescription>Feature information</CardDescription>
              </CardHeader>
              <CardContent className="space-y-2 text-sm text-muted-foreground">
                <p>
                  Demiurge provides chat-based interaction with AI models. Each
                  chat maintains its own conversation history.
                </p>
                <p>Messages are stored in the content-addressable store.</p>
              </CardContent>
            </Card>
          </div>
        </div>
      )}
    </div>
  );
}
