import { useEffect } from "react";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import type { ChatSettings } from "../types";

interface ChatSettingsProps {
  settings: ChatSettings;
  onSettingsChange: (settings: ChatSettings) => void;
}

const STORAGE_KEY = "demiurge-chat-settings";

export function ChatSettingsComponent({
  settings,
  onSettingsChange,
}: ChatSettingsProps) {
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
    <Card className="bg-card/80">
      <CardHeader>
        <CardTitle className="text-lg">Settings</CardTitle>
        <CardDescription>Configure model and generation parameters</CardDescription>
      </CardHeader>
      <CardContent className="space-y-4">
        <div className="space-y-2">
          <label htmlFor="model" className="text-sm font-medium">
            Model
          </label>
          <select
            id="model"
            value={settings.model}
            onChange={(e) => handleChange("model", e.target.value)}
            className="w-full h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <option value="gpt-4o-mini">GPT-4o Mini</option>
            <option value="gpt-4o">GPT-4o</option>
            <option value="gpt-4-turbo">GPT-4 Turbo</option>
            <option value="claude-3-5-sonnet-20241022">Claude 3.5 Sonnet</option>
            <option value="claude-3-5-haiku-20241022">Claude 3.5 Haiku</option>
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
            className="w-full h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <option value="openai">OpenAI</option>
            <option value="anthropic">Anthropic</option>
          </select>
        </div>

        <div className="space-y-2">
          <label htmlFor="max-tokens" className="text-sm font-medium">
            Max Tokens
          </label>
          <Input
            id="max-tokens"
            type="number"
            min={1}
            max={32000}
            value={settings.max_tokens}
            onChange={(e) =>
              handleChange("max_tokens", parseInt(e.target.value) || 1024)
            }
          />
        </div>

        <div className="pt-2 space-y-2 text-xs text-muted-foreground">
          <div className="flex justify-between">
            <span>Current model:</span>
            <span className="font-medium text-foreground">{settings.model}</span>
          </div>
          <div className="flex justify-between">
            <span>Provider:</span>
            <span className="font-medium text-foreground">{settings.provider}</span>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}
