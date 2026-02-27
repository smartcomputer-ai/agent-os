import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useChatState, useChatTranscript } from "@/sdk/queries";
import { useEventsPost } from "@/sdk/mutations";
import { decodeCborFromBase64 } from "@/sdk/cbor";
import { ChatLayout } from "../components/chat-layout";
import { ChatMainView } from "../components/chat-main-view";
import { ChatSettingsDialog } from "../components/chat-settings-dialog";
import { generateSessionId } from "../lib/message-utils";
import {
  DEFAULT_CHAT_SETTINGS,
  type ChatSettings,
  type DemiurgeState,
  type SessionIngress,
} from "../types";

export function UnifiedChatPage() {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const selectedChatId = searchParams.get("id");

  const [settings, setSettings] = useState<ChatSettings>(DEFAULT_CHAT_SETTINGS);
  const [settingsOpen, setSettingsOpen] = useState(false);

  const eventsPostMutation = useEventsPost();

  const {
    data: chatStateData,
    isLoading: chatStateLoading,
    error: chatStateError,
  } = useChatState(selectedChatId ?? "", {
    enabled: !!selectedChatId,
  });

  const {
    data: transcriptData,
    isLoading: transcriptLoading,
    error: transcriptError,
  } = useChatTranscript(selectedChatId ?? "");

  let chatState: DemiurgeState | undefined;
  let decodeError: Error | undefined;
  if (chatStateData?.state_b64) {
    try {
      chatState = decodeCborFromBase64<DemiurgeState>(chatStateData.state_b64);
    } catch (err) {
      decodeError =
        err instanceof Error ? err : new Error("Failed to decode session state");
    }
  }

  const handleChatSelect = (chatId: string) => {
    navigate(`/chat?id=${chatId}`);
  };

  const handleNewChat = async () => {
    try {
      const chatId = generateSessionId();
      const ingress: SessionIngress = {
        session_id: chatId,
        observed_at_ns: Date.now(),
        ingress: { $tag: "Noop" },
      };

      await eventsPostMutation.mutateAsync({
        schema: "aos.agent/SessionIngress@1",
        value: ingress,
      });

      navigate(`/chat?id=${chatId}`);
    } catch (error) {
      console.error("Failed to create chat:", error);
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
          sessionState={chatState?.session}
          messages={transcriptData?.messages ?? []}
          isLoading={chatStateLoading || transcriptLoading}
          error={chatStateError || transcriptError || decodeError}
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
