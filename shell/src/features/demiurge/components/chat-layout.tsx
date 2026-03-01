import { ChatSidebar } from "./chat-sidebar";
import { SidebarProvider, SidebarInset, SidebarTrigger } from "@/components/ui/sidebar";
import { Button } from "@/components/ui/button";
import { Settings } from "lucide-react";

interface ChatLayoutProps {
  selectedChatId: string | null;
  onChatSelect: (chatId: string) => void;
  onNewChat: () => void;
  onSettingsOpen: () => void;
  children: React.ReactNode;
}

export function ChatLayout({
  selectedChatId,
  onChatSelect,
  onNewChat,
  onSettingsOpen,
  children,
}: ChatLayoutProps) {
  return (
    <div className="h-screen overflow-hidden -mx-4 sm:-mx-6 -mt-24 -mb-6">
      <SidebarProvider>
        <ChatSidebar
          selectedChatId={selectedChatId}
          onChatSelect={onChatSelect}
          onNewChat={onNewChat}
        />
        <SidebarInset className="h-screen bg-transparent relative">
          {/* Settings Button - Floating Top Left */}
          <div className="absolute top-24 left-4 z-10">
            <Button
              variant="ghost"
              size="icon"
              onClick={onSettingsOpen}
              title="Settings"
              className="size-9 bg-background/80 backdrop-blur shadow-sm hover:bg-background/95"
            >
              <Settings className="size-5" />
            </Button>
          </div>

          {/* Mobile Header with Menu Button */}
          <div className="md:hidden border-b p-4 flex items-center justify-between bg-background/80">
            <SidebarTrigger />
            <h1 className="text-lg font-semibold">Demiurge</h1>
            <Button variant="ghost" size="icon" onClick={onSettingsOpen}>
              <Settings className="size-5" />
            </Button>
          </div>

          {children}
        </SidebarInset>
      </SidebarProvider>
    </div>
  );
}
