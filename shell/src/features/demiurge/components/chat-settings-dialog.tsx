import { useEffect } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import type { ChatSettings } from "../types";

interface ChatSettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  settings: ChatSettings;
  onSettingsChange: (settings: ChatSettings) => void;
}

const STORAGE_KEY = "demiurge-chat-settings";

export function ChatSettingsDialog({
  open,
  onOpenChange,
  settings,
  onSettingsChange,
}: ChatSettingsDialogProps) {
  // Load from localStorage on mount
  useEffect(() => {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) {
      try {
        const parsed = JSON.parse(stored);
        onSettingsChange(parsed);
      } catch (e) {
        console.error("Failed to parse stored settings:", e);
      }
    }
  }, [onSettingsChange]);

  const handleChange = (key: keyof ChatSettings, value: string | number) => {
    const newSettings = { ...settings, [key]: value };
    onSettingsChange(newSettings);
    localStorage.setItem(STORAGE_KEY, JSON.stringify(newSettings));
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-md">
        <DialogHeader>
          <DialogTitle>Chat Settings</DialogTitle>
          <DialogDescription>
            Configure model and provider for your chats
          </DialogDescription>
        </DialogHeader>

        <div className="space-y-4 py-4">
          <div className="space-y-2">
            <label htmlFor="model" className="text-sm font-medium">
              Model
            </label>
            <select
              id="model"
              value={settings.model}
              onChange={(e) => handleChange("model", e.target.value)}
              className="w-full h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm"
            >
              <option value="gpt-5.2">GPT-5.2</option>
              <option value="gpt-5-mini">GPT-5 Mini</option>
              <option value="gpt-5.2-codex">GPT-5.2 Codex</option>
              <option value="claude-sonnet-4-5">Claude Sonnet 4.5</option>
              <option value="gpt-mock">GPT Mock</option>
            </select>
          </div>

          <div className="space-y-2">
            <label htmlFor="provider" className="text-sm font-medium">
              Provider
            </label>
            <select
              id="provider"
              value={settings.provider}
              onChange={(e) => handleChange("provider", e.target.value)}
              className="w-full h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm"
            >
              <option value="openai-responses">OpenAI (Responses)</option>
              <option value="anthropic">Anthropic</option>
              <option value="openai-compatible">OpenAI Compatible</option>
              <option value="mock">Mock</option>
            </select>
          </div>

          <div className="pt-2 space-y-2 text-xs text-muted-foreground border-t">
            <div className="flex justify-between">
              <span>Current model:</span>
              <span className="font-medium text-foreground">{settings.model}</span>
            </div>
            <div className="flex justify-between">
              <span>Provider:</span>
              <span className="font-medium text-foreground">{settings.provider}</span>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
