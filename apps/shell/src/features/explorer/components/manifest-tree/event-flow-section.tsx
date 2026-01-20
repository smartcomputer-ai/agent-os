import { useNavigate } from "react-router-dom";
import { ArrowRight, Workflow } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { TreeNode, TreeLeaf } from "./tree-node";
import type { Routing, Trigger } from "../../lib/manifest-types";

interface EventFlowSectionProps {
  routing?: Routing;
  triggers?: Trigger[];
}

export function EventFlowSection({ routing, triggers }: EventFlowSectionProps) {
  const navigate = useNavigate();

  const routingEvents = routing?.events || [];
  const inboxes = routing?.inboxes || [];
  const triggersList = triggers || [];

  const totalCount = routingEvents.length + inboxes.length + triggersList.length;

  const handleDefClick = (kind: string, name: string) => {
    navigate(`/explorer/defs/${kind}/${encodeURIComponent(name)}`);
  };

  const handlePlanClick = (name: string) => {
    navigate(`/explorer/plans/${encodeURIComponent(name)}`);
  };

  if (totalCount === 0) {
    return (
      <TreeNode
        label="Event Flow"
        icon={<Workflow className="w-3.5 h-3.5 text-cyan-500" />}
        badge={<Badge variant="secondary" className="text-xs px-1.5 py-0">0</Badge>}
        level={0}
      />
    );
  }

  return (
    <TreeNode
      label="Event Flow"
      icon={<Workflow className="w-3.5 h-3.5 text-cyan-500" />}
      badge={<Badge variant="secondary" className="text-xs px-1.5 py-0">{totalCount}</Badge>}
      defaultExpanded
      level={0}
    >
      {/* Routing: events → reducers */}
      {routingEvents.length > 0 && (
        <TreeNode
          label="Routing"
          metadata="events → reducers"
          badge={
            <span className="text-xs text-muted-foreground">{routingEvents.length}</span>
          }
          defaultExpanded
          level={1}
        >
          {routingEvents.map((r, i) => (
            <TreeLeaf
              key={`${r.event}-${r.reducer}-${i}`}
              level={2}
              label={
                <span className="flex items-center gap-1.5">
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDefClick("defschema", r.event);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {r.event}
                  </button>
                  <ArrowRight className="w-3 h-3 text-muted-foreground" />
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDefClick("defmodule", r.reducer);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {r.reducer}
                  </button>
                  {r.key_field && (
                    <span className="text-muted-foreground text-xs">
                      (key: {r.key_field})
                    </span>
                  )}
                </span>
              }
            />
          ))}
        </TreeNode>
      )}

      {/* Triggers: events → plans */}
      {triggersList.length > 0 && (
        <TreeNode
          label="Triggers"
          metadata="events → plans"
          badge={
            <span className="text-xs text-muted-foreground">{triggersList.length}</span>
          }
          defaultExpanded
          level={1}
        >
          {triggersList.map((t, i) => (
            <TreeLeaf
              key={`${t.event}-${t.plan}-${i}`}
              level={2}
              label={
                <span className="flex items-center gap-1.5">
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDefClick("defschema", t.event);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {t.event}
                  </button>
                  <ArrowRight className="w-3 h-3 text-muted-foreground" />
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      handlePlanClick(t.plan);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {t.plan}
                  </button>
                  {t.correlate_by && (
                    <span className="text-muted-foreground text-xs">
                      (correlate: {t.correlate_by})
                    </span>
                  )}
                </span>
              }
            />
          ))}
        </TreeNode>
      )}

      {/* Inboxes: external sources → reducers */}
      {inboxes.length > 0 && (
        <TreeNode
          label="Inboxes"
          metadata="sources → reducers"
          badge={
            <span className="text-xs text-muted-foreground">{inboxes.length}</span>
          }
          defaultExpanded
          level={1}
        >
          {inboxes.map((inbox, i) => (
            <TreeLeaf
              key={`${inbox.source}-${inbox.reducer}-${i}`}
              level={2}
              label={
                <span className="flex items-center gap-1.5">
                  <span className="font-mono text-xs text-foreground">
                    {inbox.source}
                  </span>
                  <ArrowRight className="w-3 h-3 text-muted-foreground" />
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDefClick("defmodule", inbox.reducer);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {inbox.reducer}
                  </button>
                </span>
              }
            />
          ))}
        </TreeNode>
      )}
    </TreeNode>
  );
}
