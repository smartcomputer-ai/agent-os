import { useNavigate } from "react-router-dom";
import { Settings } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { TreeNode, TreeProperty } from "./tree-node";
import type { Defaults, CapGrant } from "../../lib/manifest-types";

interface DefaultsSectionProps {
  defaults?: Defaults;
}

export function DefaultsSection({ defaults }: DefaultsSectionProps) {
  const navigate = useNavigate();

  const hasPolicy = Boolean(defaults?.policy);
  const capGrants = defaults?.cap_grants || [];
  const hasContent = hasPolicy || capGrants.length > 0;

  const handleDefClick = (kind: string, name: string) => {
    navigate(`/explorer/defs/${kind}/${encodeURIComponent(name)}`);
  };

  if (!hasContent) {
    return (
      <TreeNode
        label="Defaults"
        icon={<Settings className="w-3.5 h-3.5 text-gray-500" />}
        badge={<span className="text-xs text-muted-foreground">not set</span>}
        level={0}
      />
    );
  }

  return (
    <TreeNode
      label="Defaults"
      icon={<Settings className="w-3.5 h-3.5 text-gray-500" />}
      defaultExpanded
      level={0}
    >
      {/* Default policy */}
      {defaults?.policy && (
        <TreeProperty
          name="Policy"
          value={defaults.policy}
          level={1}
          onValueClick={() => handleDefClick("defpolicy", defaults.policy!)}
        />
      )}

      {/* Cap grants */}
      {capGrants.length > 0 && (
        <TreeNode
          label="Cap Grants"
          badge={
            <Badge variant="secondary" className="text-xs px-1.5 py-0">
              {capGrants.length}
            </Badge>
          }
          defaultExpanded
          level={1}
        >
          {capGrants.map((grant) => (
            <CapGrantNode
              key={grant.name}
              grant={grant}
              onDefClick={handleDefClick}
            />
          ))}
        </TreeNode>
      )}
    </TreeNode>
  );
}

interface CapGrantNodeProps {
  grant: CapGrant;
  onDefClick: (kind: string, name: string) => void;
}

function CapGrantNode({ grant, onDefClick }: CapGrantNodeProps) {
  const paramsPreview = formatParamsPreview(grant.params);

  return (
    <TreeNode
      label={grant.name}
      badge={
        <Badge variant="outline" className="text-xs px-1 py-0">
          grant
        </Badge>
      }
      defaultExpanded
      level={2}
    >
      {/* Cap reference */}
      <TreeProperty
        name="Cap"
        value={grant.cap}
        level={3}
        onValueClick={() => onDefClick("defcap", grant.cap)}
      />

      {/* Params preview */}
      {paramsPreview && (
        <TreeProperty name="Params" value={paramsPreview} level={3} />
      )}
    </TreeNode>
  );
}

/**
 * Format cap grant params for display.
 * Handles the AIR value format (e.g., { record: { hosts: { set: [...] } } })
 */
function formatParamsPreview(params: unknown): string | null {
  if (!params) return null;

  try {
    // Handle AIR record format
    if (typeof params === "object" && params !== null && "record" in params) {
      const record = (params as { record: Record<string, unknown> }).record;
      const parts: string[] = [];

      for (const [key, value] of Object.entries(record)) {
        const formatted = formatValue(value);
        if (formatted) {
          parts.push(`${key}=${formatted}`);
        }
      }

      return parts.join(", ");
    }

    // Fallback to JSON
    return JSON.stringify(params);
  } catch {
    return "[complex]";
  }
}

function formatValue(value: unknown): string | null {
  if (!value || typeof value !== "object") {
    return String(value);
  }

  // Handle AIR set format: { set: [{ text: "..." }, ...] }
  if ("set" in value && Array.isArray((value as { set: unknown[] }).set)) {
    const items = (value as { set: unknown[] }).set
      .map((item) => {
        if (typeof item === "object" && item !== null && "text" in item) {
          return (item as { text: string }).text;
        }
        return String(item);
      })
      .slice(0, 3); // Limit to first 3

    const suffix =
      (value as { set: unknown[] }).set.length > 3
        ? `, +${(value as { set: unknown[] }).set.length - 3} more`
        : "";

    return `[${items.join(", ")}${suffix}]`;
  }

  // Handle AIR text format: { text: "..." }
  if ("text" in value) {
    return (value as { text: string }).text;
  }

  return null;
}
