import { useNavigate } from "react-router-dom";
import { ArrowRight, Workflow } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { TreeNode, TreeLeaf } from "./tree-node";
import type { Routing } from "../../lib/manifest-types";

interface EventFlowSectionProps {
  routing?: Routing;
}

export function EventFlowSection({ routing }: EventFlowSectionProps) {
  const navigate = useNavigate();

  const subscriptions = routing?.subscriptions || [];
  const inboxes = routing?.inboxes || [];

  const totalCount = subscriptions.length + inboxes.length;

  const handleDefClick = (kind: string, name: string) => {
    navigate(`/manifest/defs/${kind}/${encodeURIComponent(name)}`);
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
      {/* Subscriptions: events → modules */}
      {subscriptions.length > 0 && (
        <TreeNode
          label="Subscriptions"
          metadata="events → modules"
          badge={
            <span className="text-xs text-muted-foreground">{subscriptions.length}</span>
          }
          defaultExpanded
          level={1}
        >
          {subscriptions.map((sub, i) => (
            <TreeLeaf
              key={`${sub.event}-${sub.module}-${i}`}
              level={2}
              label={
                <span className="flex items-center gap-1.5">
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDefClick("defschema", sub.event);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {sub.event}
                  </button>
                  <ArrowRight className="w-3 h-3 text-muted-foreground" />
                  <button
                    type="button"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleDefClick("defmodule", sub.module);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {sub.module}
                  </button>
                  {sub.key_field && (
                    <span className="text-muted-foreground text-xs">
                      (key: {sub.key_field})
                    </span>
                  )}
                </span>
              }
            />
          ))}
        </TreeNode>
      )}

      {/* Inboxes: external sources → workflows */}
      {inboxes.length > 0 && (
        <TreeNode
          label="Inboxes"
          metadata="sources → workflows"
          badge={
            <span className="text-xs text-muted-foreground">{inboxes.length}</span>
          }
          defaultExpanded
          level={1}
        >
          {inboxes.map((inbox, i) => (
            <TreeLeaf
              key={`${inbox.source}-${inbox.workflow}-${i}`}
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
                      handleDefClick("defmodule", inbox.workflow);
                    }}
                    className="text-primary hover:underline font-mono text-xs"
                  >
                    {inbox.workflow}
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
