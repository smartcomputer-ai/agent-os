export type ChatRole =
  | { $tag: "User"; $value?: null }
  | { $tag: "Assistant"; $value?: null };

export interface TokenUsage {
  prompt: number;
  completion: number;
}

export interface ChatMessage {
  request_id: number;
  role: ChatRole;
  text: string | null;
  message_ref: string | null;
  token_usage: TokenUsage | null;
}

export interface ChatState {
  messages: ChatMessage[];
  last_request_id: number;
  title: string | null;
  created_at_ms: number | null;
}

export interface ChatCreatedEvent {
  chat_id: string;
  title: string;
  created_at_ms: number;
}

export interface UserMessageEvent {
  chat_id: string;
  request_id: number;
  text: string;
  message_ref: string;
  model: string;
  provider: string;
  max_tokens: number;
  tool_refs: string[] | null;
  tool_choice: LlmToolChoice | null;
}

export interface ChatSettings {
  model: string;
  provider: string;
  max_tokens: number;
}

export const DEFAULT_CHAT_SETTINGS: ChatSettings = {
  model: "gpt-4o-mini",
  provider: "openai-responses",
  max_tokens: 1024,
};

export type LlmToolChoice =
  | { $tag: "Auto"; $value?: null }
  | { $tag: "None"; $value?: null }
  | { $tag: "Required"; $value?: null }
  | { $tag: "Tool"; $value: { name: string } };

export interface MessageBlob {
  role: "user" | "assistant" | "system" | "tool";
  content: ContentPart[];
  tool_calls?: unknown[];
}

export type ContentPart = TextPart | ImagePart | AudioPart;

export interface TextPart {
  type: "text";
  text: string;
}

export interface ImagePart {
  type: "image";
  mime: string;
  bytes_ref: string;
}

export interface AudioPart {
  type: "audio";
  mime: string;
  bytes_ref: string;
}
