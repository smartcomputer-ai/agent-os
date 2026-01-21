import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useChatState } from "@/sdk/queries";
import { useEventsPost } from "@/sdk/mutations";
import { decodeCborFromBase64 } from "@/sdk/cbor";
import { ChatLayout } from "../components/chat-layout";
import { ChatMainView } from "../components/chat-main-view";
import { ChatSettingsDialog } from "../components/chat-settings-dialog";
import { generateChatId } from "../lib/message-utils";
import { DEFAULT_CHAT_SETTINGS, type ChatState, type ChatSettings } from "../types";

export function UnifiedChatPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const selectedChatId = searchParams.get("id");

  const [settings, setSettings] = useState<ChatSettings>(DEFAULT_CHAT_SETTINGS);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [isCreating, setIsCreating] = useState(false);

  const eventsPostMutation = useEventsPost();

  const {
    data: chatData,
    isLoading,
    error,
  } = useChatState(selectedChatId ?? "", {
    enabled: !!selectedChatId,
  });

  let chatState: ChatState | undefined;
  let decodeError: Error | undefined;
  if (chatData?.state_b64) {
    try {
      chatState = decodeCborFromBase64<ChatState>(chatData.state_b64);
    } catch (err) {
      decodeError =
        err instanceof Error ? err : new Error("Failed to decode chat state");
    }
  }

  const handleChatSelect = (chatId: string) => {
    navigate(`/chat?id=${chatId}`);
  };

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

      navigate(`/chat?id=${chatId}`);
    } catch (error) {
      console.error("Failed to create chat:", error);
    } finally {
      setIsCreating(false);
    }
  };

  return (
    <>
      <ChatLayout
        selectedChatId={selectedChatId}
        onChatSelect={handleChatSelect}
        onNewChat={handleNewChat}
        onSettingsOpen={() => setSettingsOpen(true)}
      >
        <ChatMainView
          chatId={selectedChatId}
          chatState={chatState}
          isLoading={isLoading}
          error={error || decodeError}
          settings={settings}
        />
      </ChatLayout>

      <ChatSettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        settings={settings}
        onSettingsChange={setSettings}
      />
    </>
  );
}
