import { useNavigate } from "react-router-dom";
import { Box } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { useDefsGet } from "@/sdk/queries";
import { TreeNode, TreeProperty } from "./tree-node";
import type { NamedRef, Routing, ModuleDef, WorkflowAbi } from "../../lib/manifest-types";

interface ModulesSectionProps {
  modules: NamedRef[];
  routing?: Routing;
}

export function ModulesSection({ modules, routing }: ModulesSectionProps) {
  const navigate = useNavigate();

  // Build routing lookup: module name → events routed to it
  const routingByModule = new Map<string, string[]>();
  if (routing?.subscriptions) {
    for (const sub of routing.subscriptions) {
      const existing = routingByModule.get(sub.module) || [];
      existing.push(sub.event);
      routingByModule.set(sub.module, existing);
    }
  }

  const handleDefClick = (kind: string, name: string) => {
    navigate(`/manifest/defs/${kind}/${encodeURIComponent(name)}`);
  };

  if (modules.length === 0) {
    return (
      <TreeNode
        label="Modules"
        icon={<Box className="w-3.5 h-3.5 text-purple-500" />}
        badge={<Badge variant="secondary" className="text-xs px-1.5 py-0">0</Badge>}
        level={0}
      />
    );
  }

  return (
    <TreeNode
      label="Modules"
      icon={<Box className="w-3.5 h-3.5 text-purple-500" />}
      badge={<Badge variant="secondary" className="text-xs px-1.5 py-0">{modules.length}</Badge>}
      defaultExpanded
      level={0}
    >
      {modules.map((mod) => (
        <ModuleNode
          key={mod.name}
          name={mod.name}
          routedEvents={routingByModule.get(mod.name) || []}
          onDefClick={handleDefClick}
        />
      ))}
    </TreeNode>
  );
}

interface ModuleNodeProps {
  name: string;
  routedEvents: string[];
  onDefClick: (kind: string, name: string) => void;
}

function ModuleNode({ name, routedEvents, onDefClick }: ModuleNodeProps) {
  // Fetch module details to show ABI
  const { data } = useDefsGet({ kind: "defmodule", name });
  const def = data?.def as ModuleDef | undefined;
  const abi = def?.abi?.workflow as WorkflowAbi | undefined;

  return (
    <TreeNode
      label={name}
      onLabelClick={() => onDefClick("defmodule", name)}
      badge={
        def?.module_kind && (
          <Badge variant="outline" className="text-xs px-1 py-0">
            {def.module_kind}
          </Badge>
        )
      }
      defaultExpanded
      level={1}
    >
      {/* State schema */}
      {abi?.state && (
        <TreeProperty
          name="State"
          value={abi.state}
          level={2}
          onValueClick={() => onDefClick("defschema", abi.state)}
        />
      )}

      {/* Event schema */}
      {abi?.event && (
        <TreeProperty
          name="Event"
          value={abi.event}
          level={2}
          onValueClick={() => onDefClick("defschema", abi.event)}
        />
      )}

      {/* Key schema (for keyed workflows) */}
      {def?.key_schema && (
        <TreeProperty
          name="Key"
          value={def.key_schema}
          level={2}
          onValueClick={() => onDefClick("defschema", def.key_schema!)}
        />
      )}

      {/* Effects emitted */}
      {abi?.effects_emitted && abi.effects_emitted.length > 0 && (
        <TreeProperty
          name="Effects"
          value={abi.effects_emitted.join(", ")}
          level={2}
        />
      )}

      {/* Cap slots */}
      {abi?.cap_slots && Object.keys(abi.cap_slots).length > 0 && (
        <TreeProperty
          name="Cap slots"
          value={Object.keys(abi.cap_slots).join(", ")}
          level={2}
        />
      )}

      {/* Routed events */}
      {routedEvents.length > 0 && (
        <TreeProperty
          name="Routing"
          value={
            <span className="text-muted-foreground">
              {routedEvents.length} event{routedEvents.length !== 1 ? "s" : ""} →
            </span>
          }
          level={2}
        />
      )}
    </TreeNode>
  );
}
