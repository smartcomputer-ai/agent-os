import { Button } from "@/components/ui/button";
import {
  Sidebar,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
} from "@/components/ui/sidebar";
import { useChatList } from "@/sdk/queries";
import { ChatList } from "./chat-list";
import { Plus } from "lucide-react";

interface ChatSidebarProps {
  selectedChatId: string | null;
  onChatSelect: (chatId: string) => void;
  onNewChat: () => void;
}

export function ChatSidebar({
  selectedChatId,
  onChatSelect,
  onNewChat,
}: ChatSidebarProps) {
  const { data: chatsData, isLoading } = useChatList();
  const chats = chatsData?.cells ?? [];

  return (
    <Sidebar collapsible="none" className="pt-24 h-screen pl-4 sm:pl-6">
      <SidebarHeader className="shrink-0">
        <Button onClick={onNewChat} className="w-full">
          <Plus className="size-4 mr-2" />
          New Chat
        </Button>
      </SidebarHeader>

      <SidebarContent className="flex-1 overflow-auto">
        <SidebarGroup>
          <SidebarGroupContent>
            {isLoading ? (
              <div className="text-center text-muted-foreground py-8 text-sm">
                Loading chats...
              </div>
            ) : (
              <ChatList
                chats={chats}
                selectedChatId={selectedChatId}
                onChatSelect={onChatSelect}
              />
            )}
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>
    </Sidebar>
  );
}
