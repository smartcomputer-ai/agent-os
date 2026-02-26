/**
 * TypeScript types for AIR Manifest structure.
 * Based on spec/03-air.md manifest schema.
 */

/** Named reference in manifest catalogs */
export interface NamedRef {
  name: string;
  hash?: string;
}

/** Event routing rule: event schema → workflow module */
export interface RoutingEvent {
  event: string;
  workflow: string;
  key_field?: string;
}

/** Inbox routing: external source → workflow */
export interface RoutingInbox {
  source: string;
  workflow: string;
}

/** Routing configuration */
export interface Routing {
  events: RoutingEvent[];
  inboxes: RoutingInbox[];
}

/** Plan trigger: event → plan with optional correlation */
export interface Trigger {
  event: string;
  plan: string;
  correlate_by?: string;
}

/** Capability grant with instantiated params */
export interface CapGrant {
  name: string;
  cap: string;
  params?: unknown;
}

/** Default configuration */
export interface Defaults {
  policy?: string;
  cap_grants?: CapGrant[];
}

/** Module binding slot configuration */
export interface ModuleBinding {
  slots: Record<string, string>;
}

/** Full manifest structure */
export interface Manifest {
  $kind: "manifest";
  air_version: string;

  // Definition catalogs
  schemas: NamedRef[];
  modules: NamedRef[];
  plans: NamedRef[];
  effects: NamedRef[];
  caps: NamedRef[];
  policies: NamedRef[];
  secrets?: NamedRef[];

  // Runtime configuration
  defaults?: Defaults;
  module_bindings?: Record<string, ModuleBinding>;

  // Event flow
  routing?: Routing;
  triggers?: Trigger[];
}

// ============================================
// Enriched types (with fetched def details)
// ============================================

/** Module ABI for workflows */
export interface WorkflowAbi {
  state: string;
  event: string;
  context?: string;
  annotations?: string;
  effects_emitted?: string[];
  cap_slots?: Record<string, string>;
}

/** Module ABI for pure functions */
export interface PureAbi {
  input: string;
  output: string;
  context?: string;
}

/** Module definition with ABI */
export interface ModuleDef {
  $kind: "defmodule";
  name: string;
  module_kind: "workflow" | "pure";
  wasm_hash?: string;
  key_schema?: string;
  abi: {
    workflow?: WorkflowAbi;
    pure?: PureAbi;
  };
}

/** Plan step (simplified) */
export interface PlanStep {
  id: string;
  op: "emit_effect" | "await_receipt" | "raise_event" | "await_event" | "assign" | "end";
  kind?: string;
  event?: string;
  cap?: string;
}

/** Plan edge */
export interface PlanEdge {
  from: string;
  to: string;
  when?: unknown;
}

/** Plan definition */
export interface PlanDef {
  $kind: "defplan";
  name: string;
  input: string;
  output?: string;
  locals?: Record<string, string>;
  steps: PlanStep[];
  edges: PlanEdge[];
  required_caps?: string[];
  allowed_effects?: string[];
  invariants?: unknown[];
}

// ============================================
// Helper functions
// ============================================

/** Check if a def name is in the sys/ namespace */
export function isSysNamespace(name: string): boolean {
  return name.startsWith("sys/");
}

/** Extract namespace from a def name (e.g., "demo/Foo@1" → "demo") */
export function getNamespace(name: string): string {
  const slashIndex = name.indexOf("/");
  return slashIndex > 0 ? name.slice(0, slashIndex) : "";
}

/** Get display name without version (e.g., "demo/Foo@1" → "demo/Foo") */
export function getNameWithoutVersion(name: string): string {
  const atIndex = name.indexOf("@");
  return atIndex > 0 ? name.slice(0, atIndex) : name;
}

/** Build a map of triggers by plan name for quick lookup */
export function buildTriggersByPlan(triggers: Trigger[]): Map<string, Trigger> {
  const map = new Map<string, Trigger>();
  for (const trigger of triggers) {
    map.set(trigger.plan, trigger);
  }
  return map;
}

/** Build a map of routing by workflow name */
export function buildRoutingByWorkflow(routing: Routing): Map<string, RoutingEvent[]> {
  const map = new Map<string, RoutingEvent[]>();
  for (const event of routing.events) {
    const existing = map.get(event.workflow) || [];
    existing.push(event);
    map.set(event.workflow, existing);
  }
  return map;
}
