import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { StepOpBadge } from "./kind-badge";
import { JsonInline } from "./json-viewer";
import type { PlanStep, PlanEdge } from "../types";
import { ArrowRight, GitBranch } from "lucide-react";

interface PlanStepsProps {
  steps: PlanStep[];
  edges: PlanEdge[];
}

export function PlanSteps({ steps, edges }: PlanStepsProps) {
  // Build edge map for quick lookup
  const edgeMap = new Map<string, PlanEdge[]>();
  for (const edge of edges) {
    const existing = edgeMap.get(edge.from) ?? [];
    existing.push(edge);
    edgeMap.set(edge.from, existing);
  }

  return (
    <div className="space-y-3">
      {steps.map((step, index) => {
        const outgoing = edgeMap.get(step.id) ?? [];
        return (
          <div key={step.id} className="group">
            <Card className="bg-card/60 border-border/50">
              <CardHeader className="pb-2">
                <div className="flex items-center justify-between gap-2">
                  <div className="flex items-center gap-2">
                    <span className="text-xs text-muted-foreground font-mono">
                      {index + 1}.
                    </span>
                    <StepOpBadge op={step.op} />
                    <CardTitle className="text-sm font-mono">{step.id}</CardTitle>
                  </div>
                </div>
              </CardHeader>
              <CardContent className="space-y-2 text-sm">
                <StepDetails step={step} />
                {outgoing.length > 0 && (
                  <div className="pt-2 border-t border-border/50">
                    <div className="flex items-center gap-1 text-xs text-muted-foreground mb-1">
                      <GitBranch className="w-3 h-3" />
                      <span>Edges</span>
                    </div>
                    <div className="space-y-1">
                      {outgoing.map((edge, i) => (
                        <EdgeDisplay key={i} edge={edge} />
                      ))}
                    </div>
                  </div>
                )}
              </CardContent>
            </Card>
          </div>
        );
      })}
    </div>
  );
}

function StepDetails({ step }: { step: PlanStep }) {
  switch (step.op) {
    case "emit_effect":
      return (
        <div className="space-y-1">
          {step.kind != null ? (
            <DetailRow label="effect">
              <Badge variant="outline" className="font-mono text-xs">
                {step.kind}
              </Badge>
            </DetailRow>
          ) : null}
          {step.cap != null ? (
            <DetailRow label="cap">
              <span className="font-mono text-xs">{step.cap}</span>
            </DetailRow>
          ) : null}
          {step.params != null ? (
            <DetailRow label="params">
              <JsonInline data={step.params} maxLength={80} />
            </DetailRow>
          ) : null}
          {step.bind != null ? (
            <DetailRow label="bind">
              <JsonInline data={step.bind} maxLength={40} />
            </DetailRow>
          ) : null}
        </div>
      );

    case "raise_event":
      return (
        <div className="space-y-1">
          {step.event != null ? (
            <DetailRow label="event">
              <span className="font-mono text-xs">{step.event}</span>
            </DetailRow>
          ) : null}
          {step.params != null ? (
            <DetailRow label="value">
              <JsonInline data={step.params} maxLength={80} />
            </DetailRow>
          ) : null}
        </div>
      );

    case "await_receipt":
      return (
        <div className="space-y-1">
          {step.for != null ? (
            <DetailRow label="for">
              <JsonInline data={step.for} maxLength={60} />
            </DetailRow>
          ) : null}
          {step.bind != null ? (
            <DetailRow label="bind">
              <JsonInline data={step.bind} maxLength={40} />
            </DetailRow>
          ) : null}
        </div>
      );

    case "await_event":
      return (
        <div className="space-y-1">
          {step.event != null ? (
            <DetailRow label="event">
              <span className="font-mono text-xs">{step.event}</span>
            </DetailRow>
          ) : null}
          {step.bind != null ? (
            <DetailRow label="bind">
              <JsonInline data={step.bind} maxLength={40} />
            </DetailRow>
          ) : null}
        </div>
      );

    case "assign":
      return (
        <div className="space-y-1">
          {step.var != null ? (
            <DetailRow label="var">
              <span className="font-mono text-xs">{step.var}</span>
            </DetailRow>
          ) : null}
          {step.expr != null ? (
            <DetailRow label="expr">
              <JsonInline data={step.expr} maxLength={80} />
            </DetailRow>
          ) : null}
          {step.value != null ? (
            <DetailRow label="value">
              <JsonInline data={step.value} maxLength={80} />
            </DetailRow>
          ) : null}
        </div>
      );

    case "end":
      return (
        <div className="text-muted-foreground text-xs italic">
          Terminal step
        </div>
      );

    default:
      return null;
  }
}

function DetailRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-start gap-2 overflow-hidden">
      <span className="text-muted-foreground text-xs w-12 shrink-0">{label}:</span>
      <div className="flex-1 min-w-0 overflow-hidden">{children}</div>
    </div>
  );
}

function EdgeDisplay({ edge }: { edge: PlanEdge }) {
  return (
    <div className="flex items-center gap-2 text-xs overflow-hidden">
      <ArrowRight className="w-3 h-3 text-muted-foreground shrink-0" />
      <span className="font-mono shrink-0">{edge.to}</span>
      {edge.when != null ? (
        <span className="text-muted-foreground min-w-0 overflow-hidden">
          when: <JsonInline data={edge.when} maxLength={40} />
        </span>
      ) : null}
    </div>
  );
}

interface PlanStepsSkeletonProps {
  count?: number;
}

export function PlanStepsSkeleton({ count = 4 }: PlanStepsSkeletonProps) {
  return (
    <div className="space-y-3">
      {Array.from({ length: count }).map((_, i) => (
        <Card key={i} className="bg-card/60">
          <CardHeader className="pb-2">
            <div className="flex items-center gap-2">
              <div className="h-5 w-20 bg-muted animate-pulse rounded-full" />
              <div className="h-4 w-32 bg-muted animate-pulse rounded" />
            </div>
          </CardHeader>
          <CardContent>
            <div className="space-y-2">
              <div className="h-3 w-48 bg-muted animate-pulse rounded" />
              <div className="h-3 w-36 bg-muted animate-pulse rounded" />
            </div>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
