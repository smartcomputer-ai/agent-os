/**
 * Manifest feature type definitions for AIR definitions
 */

/** API returns kinds with "def" prefix */
export type ApiDefKind =
  | "defschema"
  | "defmodule"
  | "defplan"
  | "defeffect"
  | "defcap"
  | "defpolicy";

/** Display-friendly kind (without prefix) */
export type DefKind =
  | "schema"
  | "module"
  | "plan"
  | "effect"
  | "cap"
  | "policy";

/** All API def kinds for iteration */
export const API_DEF_KINDS: ApiDefKind[] = [
  "defschema",
  "defmodule",
  "defplan",
  "defeffect",
  "defcap",
  "defpolicy",
];

/** Display-friendly kinds for iteration */
export const DEF_KINDS: DefKind[] = [
  "schema",
  "module",
  "plan",
  "effect",
  "cap",
  "policy",
];

/** Convert API kind to display kind */
export function toDisplayKind(apiKind: string): DefKind {
  if (apiKind.startsWith("def")) {
    return apiKind.slice(3) as DefKind;
  }
  return apiKind as DefKind;
}

/** Convert display kind to API kind */
export function toApiKind(displayKind: string): ApiDefKind {
  if (displayKind.startsWith("def")) {
    return displayKind as ApiDefKind;
  }
  return `def${displayKind}` as ApiDefKind;
}

/** Color classes for each def kind (works with both API and display kinds) */
export const KIND_STYLES: Record<string, string> = {
  schema: "bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20",
  defschema: "bg-blue-500/10 text-blue-600 dark:text-blue-400 border-blue-500/20",
  module: "bg-purple-500/10 text-purple-600 dark:text-purple-400 border-purple-500/20",
  defmodule: "bg-purple-500/10 text-purple-600 dark:text-purple-400 border-purple-500/20",
  plan: "bg-green-500/10 text-green-600 dark:text-green-400 border-green-500/20",
  defplan: "bg-green-500/10 text-green-600 dark:text-green-400 border-green-500/20",
  effect: "bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20",
  defeffect: "bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20",
  cap: "bg-yellow-500/10 text-yellow-600 dark:text-yellow-400 border-yellow-500/20",
  defcap: "bg-yellow-500/10 text-yellow-600 dark:text-yellow-400 border-yellow-500/20",
  policy: "bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20",
  defpolicy: "bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20",
};

/** Plan step operations */
export type PlanStepOp =
  | "raise_event"
  | "emit_effect"
  | "await_receipt"
  | "await_event"
  | "assign"
  | "end";

export const STEP_OP_STYLES: Record<PlanStepOp, string> = {
  raise_event:
    "bg-emerald-500/10 text-emerald-600 dark:text-emerald-400 border-emerald-500/20",
  emit_effect:
    "bg-amber-500/10 text-amber-600 dark:text-amber-400 border-amber-500/20",
  await_receipt:
    "bg-sky-500/10 text-sky-600 dark:text-sky-400 border-sky-500/20",
  await_event:
    "bg-violet-500/10 text-violet-600 dark:text-violet-400 border-violet-500/20",
  assign: "bg-slate-500/10 text-slate-600 dark:text-slate-400 border-slate-500/20",
  end: "bg-zinc-500/10 text-zinc-600 dark:text-zinc-400 border-zinc-500/20",
};

/** Base interface for all defs */
export interface BaseDef {
  $kind: string;
  name: string;
}

/** Schema definition */
export interface SchemaDef extends BaseDef {
  $kind: "defschema";
  type: unknown;
}

/** Module definition */
export interface ModuleDef extends BaseDef {
  $kind: "defmodule";
  module_kind: "workflow" | "pure";
  wasm_hash: string;
  key_schema?: string;
  abi: {
    workflow?: {
      state: string;
      event: string;
      context?: string;
      annotations?: string;
      effects_emitted?: string[];
      cap_slots?: Record<string, string>;
    };
    pure?: {
      input: string;
      output: string;
      context?: string;
    };
  };
}

/** Plan step */
export interface PlanStep {
  id: string;
  op: PlanStepOp;
  kind?: string;
  event?: string;
  params?: unknown;
  cap?: string;
  bind?: unknown;
  for?: unknown;
  expr?: unknown;
  var?: string;
  value?: unknown;
}

/** Plan edge */
export interface PlanEdge {
  from: string;
  to: string;
  when?: unknown;
}

/** Plan definition */
export interface PlanDef extends BaseDef {
  $kind: "defplan";
  input: string;
  output?: string;
  locals?: Record<string, string>;
  steps: PlanStep[];
  edges: PlanEdge[];
  required_caps?: string[];
  allowed_effects?: string[];
  invariants?: unknown[];
}

/** Effect definition */
export interface EffectDef extends BaseDef {
  $kind: "defeffect";
  kind: string;
  params_schema: string;
  receipt_schema: string;
  cap_type: string;
  origin_scope?: "workflow" | "plan" | "both";
}

/** Capability definition */
export interface CapDef extends BaseDef {
  $kind: "defcap";
  cap_type: string;
  schema: unknown;
  enforcer: {
    module: string;
  };
}

/** Policy rule */
export interface PolicyRule {
  when: {
    effect_kind?: string;
    cap_name?: string;
    cap_type?: string;
    origin_kind?: "plan" | "workflow";
    origin_name?: string;
  };
  decision: "allow" | "deny";
}

/** Policy definition */
export interface PolicyDef extends BaseDef {
  $kind: "defpolicy";
  rules: PolicyRule[];
}

export type AnyDef =
  | SchemaDef
  | ModuleDef
  | PlanDef
  | EffectDef
  | CapDef
  | PolicyDef;

/** Type guard for plan def */
export function isPlanDef(def: unknown): def is PlanDef {
  return (
    typeof def === "object" &&
    def !== null &&
    "$kind" in def &&
    (def as BaseDef).$kind === "defplan"
  );
}

/** Type guard for module def */
export function isModuleDef(def: unknown): def is ModuleDef {
  return (
    typeof def === "object" &&
    def !== null &&
    "$kind" in def &&
    (def as BaseDef).$kind === "defmodule"
  );
}

/** Type guard for schema def */
export function isSchemaDef(def: unknown): def is SchemaDef {
  return (
    typeof def === "object" &&
    def !== null &&
    "$kind" in def &&
    (def as BaseDef).$kind === "defschema"
  );
}

/** Type guard for effect def */
export function isEffectDef(def: unknown): def is EffectDef {
  return (
    typeof def === "object" &&
    def !== null &&
    "$kind" in def &&
    (def as BaseDef).$kind === "defeffect"
  );
}

/** Type guard for cap def */
export function isCapDef(def: unknown): def is CapDef {
  return (
    typeof def === "object" &&
    def !== null &&
    "$kind" in def &&
    (def as BaseDef).$kind === "defcap"
  );
}

/** Type guard for policy def */
export function isPolicyDef(def: unknown): def is PolicyDef {
  return (
    typeof def === "object" &&
    def !== null &&
    "$kind" in def &&
    (def as BaseDef).$kind === "defpolicy"
  );
}

/** Map API kind strings to def $kind values */
export const API_KIND_TO_DEF_KIND: Record<string, string> = {
  schema: "defschema",
  module: "defmodule",
  plan: "defplan",
  effect: "defeffect",
  cap: "defcap",
  policy: "defpolicy",
};

/** Map def $kind values to display names */
export const DEF_KIND_LABELS: Record<string, string> = {
  defschema: "Schema",
  defmodule: "Module",
  defplan: "Plan",
  defeffect: "Effect",
  defcap: "Capability",
  defpolicy: "Policy",
  schema: "Schema",
  module: "Module",
  plan: "Plan",
  effect: "Effect",
  cap: "Capability",
  policy: "Policy",
};
