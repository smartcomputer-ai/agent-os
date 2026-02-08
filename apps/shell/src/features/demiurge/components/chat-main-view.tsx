import { useEffect, useRef } from "react";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Card, CardContent } from "@/components/ui/card";
import { ChatMessage } from "./chat-message";
import { MessageInput } from "./message-input";
import { ChatDebugPanel } from "./chat-debug-panel";
import type { ChatState, ChatSettings } from "../types";

interface ChatMainViewProps {
  chatId: string | null;
  chatState?: ChatState;
  isLoading: boolean;
  error?: Error | null;
  settings: ChatSettings;
}

export function ChatMainView({
  chatId,
  chatState,
  isLoading,
  error,
  settings,
}: ChatMainViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [chatState?.messages]);

  // No chat selected - show welcome with floating input
  if (!chatId) {
    return (
      <div className="flex-1 flex flex-col items-center justify-center p-8 relative">
        <div className="text-center space-y-4 mb-8">
          <h1 className="text-4xl font-semibold">Welcome to Demiurge</h1>
          <p className="text-muted-foreground max-w-md">
            Start a conversation with AI. Create a new chat or select one from the sidebar.
          </p>
        </div>

        {/* Floating Input */}
        <div className="absolute top-1/2 left-1/2 transform -translate-x-1/2 -translate-y-1/2 w-full max-w-3xl px-4">
          <Card className="shadow-lg bg-background/95 backdrop-blur supports-backdrop-filter:bg-background/80">
            <CardContent className="p-6">
              <MessageInput
                chatId=""
                lastRequestId={0}
                settings={settings}
                variant="floating"
                disabled={true}
              />
              <p className="text-center text-sm text-muted-foreground mt-4">
                Create a new chat to start messaging
              </p>
            </CardContent>
          </Card>
        </div>
      </div>
    );
  }

  // Error state
  if (error) {
    return (
      <div className="flex-1 flex items-center justify-center p-8 bg-transparent">
        <Card className="bg-background/95 backdrop-blur supports-backdrop-filter:bg-background/80">
          <CardContent className="py-12 px-8">
            <div className="text-center space-y-2">
              <div className="text-destructive">Failed to load chat</div>
              <div className="text-sm text-muted-foreground">{error.message}</div>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  // Loading state
  if (isLoading) {
    return (
      <div className="flex-1 flex items-center justify-center bg-transparent">
        <div className="text-muted-foreground">Loading chat...</div>
      </div>
    );
  }

  // Chat loaded - show messages + input
  const hasAssistantPending =
    chatState &&
    chatState.messages.length > 0 &&
    chatState.messages[chatState.messages.length - 1].role.$tag === "User";

  return (
    <div className="h-full w-full relative">
      {/* Messages - Full viewport height */}
      <div className="absolute inset-0">
        <ScrollArea ref={scrollRef} className="h-full w-full px-4 pb-10">
          {chatState?.messages.length === 0 ? (
            <div className="text-center text-muted-foreground py-12">
              No messages yet. Start the conversation below.
            </div>
          ) : (
            <div className="space-y-3 max-w-4xl mx-auto mt-28 mb-24">
              <ChatDebugPanel />
              {chatState?.messages.map((message, index) => (
                <ChatMessage
                  key={`${message.request_id}-${message.role.$tag}-${message.message_ref ?? "inline"}-${index}`}
                  message={message}
                  isLatest={index === chatState.messages.length - 1}
                />
              ))}
              {hasAssistantPending && (
                <div className="flex gap-3 p-3 rounded-lg bg-card/80 backdrop-blur supports-backdrop-filter:bg-card/60 mr-12">
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
      </div>

      {/* Floating Input */}
      <div className="absolute bottom-0 left-0 right-0 p-4 pointer-events-none">
        <div className="max-w-4xl mx-auto pointer-events-auto">
          <MessageInput
            chatId={chatId}
            lastRequestId={chatState?.last_request_id ?? 0}
            settings={settings}
            variant="fixed"
            disabled={hasAssistantPending}
          />
        </div>
      </div>
    </div>
  );
}
