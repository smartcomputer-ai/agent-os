/**
 * Manifest feature type definitions for AIR definitions
 */

/** API returns kinds with "def" prefix */
export type ApiDefKind =
  | "defschema"
  | "defmodule"
  | "defeffect"
  | "defcap"
  | "defpolicy";

/** Display-friendly kind (without prefix) */
export type DefKind =
  | "schema"
  | "module"
  | "effect"
  | "cap"
  | "policy";

/** All API def kinds for iteration */
export const API_DEF_KINDS: ApiDefKind[] = [
  "defschema",
  "defmodule",
  "defeffect",
  "defcap",
  "defpolicy",
];

/** Display-friendly kinds for iteration */
export const DEF_KINDS: DefKind[] = [
  "schema",
  "module",
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
  effect: "bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20",
  defeffect: "bg-orange-500/10 text-orange-600 dark:text-orange-400 border-orange-500/20",
  cap: "bg-yellow-500/10 text-yellow-600 dark:text-yellow-400 border-yellow-500/20",
  defcap: "bg-yellow-500/10 text-yellow-600 dark:text-yellow-400 border-yellow-500/20",
  policy: "bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20",
  defpolicy: "bg-red-500/10 text-red-600 dark:text-red-400 border-red-500/20",
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

/** Effect definition */
export interface EffectDef extends BaseDef {
  $kind: "defeffect";
  kind: string;
  params_schema: string;
  receipt_schema: string;
  cap_type: string;
  origin_scope?: "workflow";
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
    origin_kind?: "workflow" | "system" | "governance";
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
  | EffectDef
  | CapDef
  | PolicyDef;

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
  effect: "defeffect",
  cap: "defcap",
  policy: "defpolicy",
};

/** Map def $kind values to display names */
export const DEF_KIND_LABELS: Record<string, string> = {
  defschema: "Schema",
  defmodule: "Module",
  defeffect: "Effect",
  defcap: "Capability",
  defpolicy: "Policy",
  schema: "Schema",
  module: "Module",
  effect: "Effect",
  cap: "Capability",
  policy: "Policy",
};
