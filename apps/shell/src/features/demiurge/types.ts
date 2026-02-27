export type ReasoningEffort =
  | { $tag: "Low"; $value?: null }
  | { $tag: "Medium"; $value?: null }
  | { $tag: "High"; $value?: null };

export type SessionLifecycle =
  | { $tag: "Idle"; $value?: null }
  | { $tag: "Running"; $value?: null }
  | { $tag: "WaitingInput"; $value?: null }
  | { $tag: "Paused"; $value?: null }
  | { $tag: "Cancelling"; $value?: null }
  | { $tag: "Completed"; $value?: null }
  | { $tag: "Failed"; $value?: null }
  | { $tag: "Cancelled"; $value?: null };

export interface WorkspaceBinding {
  workspace: string;
  version?: number | null;
}

export interface SessionConfig {
  provider: string;
  model: string;
  reasoning_effort?: ReasoningEffort | null;
  max_tokens?: number | null;
  workspace_binding?: WorkspaceBinding | null;
  default_prompt_pack?: string | null;
  default_prompt_refs?: string[] | null;
  default_tool_catalog?: string | null;
  default_tool_refs?: string[] | null;
}

export interface SessionState {
  session_id: string;
  lifecycle: SessionLifecycle;
  in_flight_effects: number;
  created_at: number;
  updated_at: number;
  active_run_id?: unknown | null;
  active_run_config?: SessionConfig | null;
}

export interface DemiurgeState {
  session: SessionState;
  pending_tool_call?: unknown | null;
}

export type SessionIngressKind =
  | {
      $tag: "RunRequested";
      $value: {
        input_ref: string;
        run_overrides: SessionConfig | null;
      };
    }
  | { $tag: "Noop"; $value?: null };

export interface SessionIngress {
  session_id: string;
  observed_at_ns: number;
  ingress: SessionIngressKind;
}

export interface ChatSettings {
  model: string;
  provider: string;
  max_tokens: number;
}

export const DEFAULT_CHAT_SETTINGS: ChatSettings = {
  model: "gpt-5.2",
  provider: "openai-responses",
  max_tokens: 4096,
};

export interface ChatTokenUsage {
  prompt: number;
  completion: number;
  total?: number | null;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant";
  text: string;
  observed_at_ns: number;
  input_ref?: string | null;
  output_ref?: string | null;
  tool_calls_ref?: string | null;
  token_usage?: ChatTokenUsage | null;
  failed?: boolean;
}
