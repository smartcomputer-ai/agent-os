/**
 * TypeScript types for AIR Manifest structure.
 * Based on spec/03-air.md manifest schema.
 */

/** Named reference in manifest catalogs */
export interface NamedRef {
  name: string;
  hash?: string;
}

/** Routing subscription: event schema → workflow module */
export interface RoutingSubscription {
  event: string;
  module: string;
  key_field?: string;
}

/** Inbox routing: external source → workflow */
export interface RoutingInbox {
  source: string;
  workflow: string;
}

/** Routing configuration */
export interface Routing {
  subscriptions: RoutingSubscription[];
  inboxes: RoutingInbox[];
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
  effects: NamedRef[];
  caps: NamedRef[];
  policies: NamedRef[];
  secrets?: NamedRef[];

  // Runtime configuration
  defaults?: Defaults;
  module_bindings?: Record<string, ModuleBinding>;

  // Event flow
  routing?: Routing;
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

/** Build a map of subscriptions by module name */
export function buildSubscriptionsByModule(routing: Routing): Map<string, RoutingSubscription[]> {
  const map = new Map<string, RoutingSubscription[]>();
  for (const sub of routing.subscriptions) {
    const existing = map.get(sub.module) || [];
    existing.push(sub);
    map.set(sub.module, existing);
  }
  return map;
}
