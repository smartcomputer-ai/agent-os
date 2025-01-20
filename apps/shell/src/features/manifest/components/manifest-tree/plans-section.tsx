import { useNavigate } from "react-router-dom";
import { GitBranch } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { useDefsGet } from "@/sdk/queries";
import { TreeNode, TreeProperty } from "./tree-node";
import type { NamedRef, Trigger, PlanDef } from "../../lib/manifest-types";

interface PlansSectionProps {
  plans: NamedRef[];
  triggers?: Trigger[];
}

export function PlansSection({ plans, triggers }: PlansSectionProps) {
  const navigate = useNavigate();

  // Build trigger lookup: plan name → trigger config
  const triggerByPlan = new Map<string, Trigger>();
  if (triggers) {
    for (const t of triggers) {
      triggerByPlan.set(t.plan, t);
    }
  }

  const handlePlanClick = (name: string) => {
    navigate(`/manifest/plans/${encodeURIComponent(name)}`);
  };

  const handleDefClick = (kind: string, name: string) => {
    navigate(`/manifest/defs/${kind}/${encodeURIComponent(name)}`);
  };

  if (plans.length === 0) {
    return (
      <TreeNode
        label="Plans"
        icon={<GitBranch className="w-3.5 h-3.5 text-green-500" />}
        badge={<Badge variant="secondary" className="text-xs px-1.5 py-0">0</Badge>}
        level={0}
      />
    );
  }

  return (
    <TreeNode
      label="Plans"
      icon={<GitBranch className="w-3.5 h-3.5 text-green-500" />}
      badge={<Badge variant="secondary" className="text-xs px-1.5 py-0">{plans.length}</Badge>}
      defaultExpanded
      level={0}
    >
      {plans.map((plan) => (
        <PlanNode
          key={plan.name}
          name={plan.name}
          trigger={triggerByPlan.get(plan.name)}
          onPlanClick={handlePlanClick}
          onDefClick={handleDefClick}
        />
      ))}
    </TreeNode>
  );
}

interface PlanNodeProps {
  name: string;
  trigger?: Trigger;
  onPlanClick: (name: string) => void;
  onDefClick: (kind: string, name: string) => void;
}

function PlanNode({ name, trigger, onPlanClick, onDefClick }: PlanNodeProps) {
  // Fetch plan details to show steps/effects/caps
  const { data } = useDefsGet({ kind: "defplan", name });
  const def = data?.def as PlanDef | undefined;

  const stepCount = def?.steps?.length ?? 0;
  const edgeCount = def?.edges?.length ?? 0;

  return (
    <TreeNode
      label={name}
      onLabelClick={() => onPlanClick(name)}
      badge={
        <span className="text-xs text-muted-foreground">
          {stepCount} step{stepCount !== 1 ? "s" : ""}
        </span>
      }
      defaultExpanded
      level={1}
    >
      {/* Trigger info */}
      {trigger && (
        <TreeProperty
          name="Trigger"
          value={
            <span>
              <button
                type="button"
                onClick={() => onDefClick("defschema", trigger.event)}
                className="text-primary hover:underline"
              >
                {trigger.event}
              </button>
              {trigger.correlate_by && (
                <span className="text-muted-foreground ml-1">
                  (correlate: {trigger.correlate_by})
                </span>
              )}
            </span>
          }
          level={2}
        />
      )}

      {/* Input schema */}
      {def?.input && (
        <TreeProperty
          name="Input"
          value={def.input}
          level={2}
          onValueClick={() => onDefClick("defschema", def.input)}
        />
      )}

      {/* Output schema */}
      {def?.output && (
        <TreeProperty
          name="Output"
          value={def.output}
          level={2}
          onValueClick={() => onDefClick("defschema", def.output!)}
        />
      )}

      {/* Steps/edges summary */}
      {stepCount > 0 && (
        <TreeProperty
          name="Structure"
          value={`${stepCount} steps, ${edgeCount} edges`}
          level={2}
        />
      )}

      {/* Allowed effects */}
      {def?.allowed_effects && def.allowed_effects.length > 0 && (
        <TreeProperty
          name="Effects"
          value={def.allowed_effects.join(", ")}
          level={2}
        />
      )}

      {/* Required caps */}
      {def?.required_caps && def.required_caps.length > 0 && (
        <TreeProperty
          name="Caps"
          value={def.required_caps.join(", ")}
          level={2}
        />
      )}
    </TreeNode>
  );
}
