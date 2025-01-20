import { memo } from "react";
import { Handle, Position, type NodeProps } from "@xyflow/react";
import { StepOpBadge } from "./kind-badge";
import type { PlanStep } from "../types";

interface PlanDagNodeData {
  step: PlanStep;
}

export const PlanDagNode = memo(function PlanDagNode({
  data,
  selected,
}: NodeProps & { data: PlanDagNodeData }) {
  const { step } = data;

  return (
    <div
      className={`
        relative px-3 py-2 rounded-lg border bg-card shadow-sm min-w-[180px]
        transition-all duration-150
        ${selected ? "border-primary ring-2 ring-primary/20" : "border-border"}
        hover:border-primary/50
      `}
    >
      <Handle
        type="target"
        position={Position.Top}
        className="!bg-muted-foreground !border-background !w-2 !h-2"
      />

      <div className="flex flex-col gap-1.5">
        <div className="flex items-center gap-2">
          <StepOpBadge op={step.op} className="text-[9px]" />
        </div>
        <div className="font-mono text-xs text-foreground truncate">
          {step.id}
        </div>
        {step.op === "emit_effect" && step.kind && (
          <div className="text-[10px] text-muted-foreground truncate">
            {step.kind}
          </div>
        )}
        {step.op === "raise_event" && step.event && (
          <div className="text-[10px] text-muted-foreground truncate">
            {step.event}
          </div>
        )}
        {step.op === "await_event" && step.event && (
          <div className="text-[10px] text-muted-foreground truncate">
            {step.event}
          </div>
        )}
        {step.op === "await_receipt" && step.for && (
          <div className="text-[10px] text-muted-foreground truncate">
            for: {typeof step.for === "string" ? step.for : "..."}
          </div>
        )}
        {step.op === "assign" && step.var && (
          <div className="text-[10px] text-muted-foreground truncate">
            {step.var}
          </div>
        )}
      </div>

      <Handle
        type="source"
        position={Position.Bottom}
        className="!bg-muted-foreground !border-background !w-2 !h-2"
      />
    </div>
  );
});
