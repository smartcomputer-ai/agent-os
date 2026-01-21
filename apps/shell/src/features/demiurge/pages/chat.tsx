import { useState, useEffect, useRef } from "react";
import { useParams, Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useChatState } from "@/sdk/queries";
import { ChatMessage } from "../components/chat-message";
import { MessageInput } from "../components/message-input";
import { ChatSettingsComponent } from "../components/chat-settings";
import { DEFAULT_CHAT_SETTINGS, type ChatState, type ChatSettings } from "../types";

export function ChatPage() {
  const { chatId } = useParams();
  const [settings, setSettings] = useState<ChatSettings>(DEFAULT_CHAT_SETTINGS);
  const scrollRef = useRef<HTMLDivElement>(null);

  const {
    data: chatData,
    isLoading,
    error,
  } = useChatState(chatId ?? "", {
    enabled: !!chatId,
  });

  const chatState = chatData?.state_b64
    ? (JSON.parse(atob(chatData.state_b64)) as ChatState)
    : undefined;

  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [chatState?.messages]);

  if (!chatId) {
    return (
      <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center space-y-2">
              <div className="text-destructive">Chat ID not provided</div>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (error) {
    return (
      <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center space-y-2">
              <div className="text-destructive">Failed to load chat</div>
              <div className="text-sm text-muted-foreground">{error.message}</div>
              <div className="pt-4">
                <Link to="/chat">
                  <Button variant="outline">Back to chats</Button>
                </Link>
              </div>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  if (isLoading) {
    return (
      <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center text-muted-foreground">
              Loading chat...
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  const hasAssistantPending =
    chatState &&
    chatState.messages.length > 0 &&
    chatState.messages[chatState.messages.length - 1].role === "User";

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <div className="flex items-center gap-2">
            <Link to="/chat">
              <Button variant="outline" size="sm">
                Back
              </Button>
            </Link>
            <Badge variant="secondary">Chat</Badge>
          </div>
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            {chatState?.title || chatId}
          </h1>
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[1fr_320px]">
        <div className="space-y-4">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Messages</CardTitle>
              <CardDescription>
                {chatState?.messages.length ?? 0} messages in this conversation
              </CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <ScrollArea
                ref={scrollRef}
                className="h-[500px] pr-4"
              >
                {chatState?.messages.length === 0 ? (
                  <div className="text-center text-muted-foreground py-12">
                    No messages yet. Start the conversation below.
                  </div>
                ) : (
                  <div className="space-y-3">
                    {chatState?.messages.map((message, index) => (
                      <ChatMessage
                        key={`${message.request_id}-${message.role}`}
                        message={message}
                        isLatest={index === chatState.messages.length - 1}
                      />
                    ))}
                    {hasAssistantPending && (
                      <div className="flex gap-3 p-3 rounded-lg bg-muted mr-12">
                        <div className="flex-1">
                          <div className="text-xs text-muted-foreground mb-1">
                            Assistant
                          </div>
                          <div className="text-sm text-muted-foreground italic">
                            Thinking...
                          </div>
                        </div>
                      </div>
                    )}
                  </div>
                )}
              </ScrollArea>

              <MessageInput
                chatId={chatId}
                lastRequestId={chatState?.last_request_id ?? 0}
                settings={settings}
                disabled={hasAssistantPending}
              />
            </CardContent>
          </Card>
        </div>

        <div className="space-y-4">
          <ChatSettingsComponent
            settings={settings}
            onSettingsChange={setSettings}
          />

          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Info</CardTitle>
              <CardDescription>Chat details</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              <div className="flex justify-between">
                <span className="text-muted-foreground">Chat ID:</span>
                <span className="font-mono text-xs">{chatId}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Messages:</span>
                <span>{chatState?.messages.length ?? 0}</span>
              </div>
              <div className="flex justify-between">
                <span className="text-muted-foreground">Last request:</span>
                <span>{chatState?.last_request_id ?? 0}</span>
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
