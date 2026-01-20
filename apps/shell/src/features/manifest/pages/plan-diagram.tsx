import { useState, lazy, Suspense } from "react";
import { Link, useParams } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useDefsGet } from "@/sdk/queries";
import { KindBadge } from "../components/kind-badge";
import { JsonViewer } from "../components/json-viewer";
import { PlanSteps, PlanStepsSkeleton } from "../components/plan-steps";
import { isPlanDef } from "../types";
import { Braces, Copy, FileCode, GitBranch, Hash, Info, Network, Shield, Zap } from "lucide-react";

// Lazy load the DAG component to reduce initial bundle size
const PlanDag = lazy(() =>
  import("../components/plan-dag").then((m) => ({ default: m.PlanDag }))
);
import { CopyableHash } from "../components/copyable-hash";

export function PlanDiagramPage() {
  const { name } = useParams();

  if (!name) {
    return (
      <div className="py-12 text-center text-muted-foreground">
        Invalid plan path
      </div>
    );
  }

  return <PlanDetailContent name={name} />;
}

function PlanDetailContent({ name }: { name: string }) {
  const { data, isLoading, error } = useDefsGet({ kind: "plan", name });
  const [viewMode, setViewMode] = useState<"diagram" | "structured" | "json">("structured");

  const copyToClipboard = () => {
    navigator.clipboard.writeText(name);
  };

  if (isLoading) {
    return (
      <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
        <header className="space-y-2">
          <div className="h-5 w-16 bg-muted animate-pulse rounded-full" />
          <div className="h-8 w-64 bg-muted animate-pulse rounded" />
          <div className="h-4 w-96 bg-muted animate-pulse rounded" />
        </header>
        <div className="grid gap-4 lg:grid-cols-[1.3fr_0.7fr]">
          <Card className="bg-card/80">
            <CardHeader>
              <div className="h-5 w-24 bg-muted animate-pulse rounded" />
            </CardHeader>
            <CardContent>
              <PlanStepsSkeleton count={4} />
            </CardContent>
          </Card>
          <div className="space-y-4">
            <Card className="bg-card/80">
              <CardHeader>
                <div className="h-5 w-20 bg-muted animate-pulse rounded" />
              </CardHeader>
              <CardContent>
                <div className="space-y-2">
                  <div className="h-4 w-full bg-muted animate-pulse rounded" />
                  <div className="h-4 w-full bg-muted animate-pulse rounded" />
                </div>
              </CardContent>
            </Card>
          </div>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center text-destructive">
              Failed to load plan: {error.message}
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  const def = data?.def;
  const plan = isPlanDef(def) ? def : null;

  if (!plan) {
    return (
      <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center text-muted-foreground">
              Invalid plan data
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="space-y-2">
          <KindBadge kind="plan" />
          <h1 className="text-2xl font-semibold tracking-tight text-foreground font-mono break-all">
            {name}
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            DAG orchestration workflow with {plan.steps.length} step
            {plan.steps.length === 1 ? "" : "s"} and {plan.edges.length} edge
            {plan.edges.length === 1 ? "" : "s"}.
          </p>
        </div>
        <div className="flex gap-2 shrink-0">
          <Button variant="outline" size="sm" onClick={copyToClipboard}>
            <Copy className="w-4 h-4 mr-1" />
            Copy name
          </Button>
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[1.4fr_0.6fr] min-w-0">
        {/* Main content: Steps or JSON */}
        <div className="space-y-4 min-w-0 overflow-hidden">
          <Card className="bg-card/80">
            <CardHeader>
              <div className="flex items-center justify-between">
                <div>
                  <CardTitle className="text-lg flex items-center gap-2">
                    {viewMode === "diagram" ? (
                      <Network className="w-5 h-5 text-muted-foreground" />
                    ) : viewMode === "structured" ? (
                      <GitBranch className="w-5 h-5 text-muted-foreground" />
                    ) : (
                      <Braces className="w-5 h-5 text-muted-foreground" />
                    )}
                    {viewMode === "diagram"
                      ? "DAG Diagram"
                      : viewMode === "structured"
                        ? "Steps"
                        : "Raw JSON"}
                  </CardTitle>
                  <CardDescription>
                    {viewMode === "diagram"
                      ? "Interactive workflow visualization"
                      : viewMode === "structured"
                        ? "Execution flow with edges and conditions"
                        : "Full JSON representation"}
                  </CardDescription>
                </div>
                <div className="flex rounded-lg border border-border p-0.5 bg-muted/30">
                  <button
                    onClick={() => setViewMode("diagram")}
                    className={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
                      viewMode === "diagram"
                        ? "bg-background text-foreground shadow-sm"
                        : "text-muted-foreground hover:text-foreground"
                    }`}
                  >
                    Diagram
                  </button>
                  <button
                    onClick={() => setViewMode("structured")}
                    className={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
                      viewMode === "structured"
                        ? "bg-background text-foreground shadow-sm"
                        : "text-muted-foreground hover:text-foreground"
                    }`}
                  >
                    Structured
                  </button>
                  <button
                    onClick={() => setViewMode("json")}
                    className={`px-3 py-1 text-xs font-medium rounded-md transition-colors ${
                      viewMode === "json"
                        ? "bg-background text-foreground shadow-sm"
                        : "text-muted-foreground hover:text-foreground"
                    }`}
                  >
                    JSON
                  </button>
                </div>
              </div>
            </CardHeader>
            <CardContent>
              {viewMode === "diagram" ? (
                plan.steps.length === 0 ? (
                  <div className="py-8 text-center text-muted-foreground">
                    No steps defined
                  </div>
                ) : (
                  <Suspense
                    fallback={
                      <div className="h-125 flex items-center justify-center text-muted-foreground">
                        Loading diagram...
                      </div>
                    }
                  >
                    <PlanDag steps={plan.steps} edges={plan.edges} />
                  </Suspense>
                )
              ) : viewMode === "structured" ? (
                plan.steps.length === 0 ? (
                  <div className="py-8 text-center text-muted-foreground">
                    No steps defined
                  </div>
                ) : (
                  <PlanSteps steps={plan.steps} edges={plan.edges} />
                )
              ) : (
                <JsonViewer data={plan} className="max-h-150 overflow-y-auto" />
              )}
            </CardContent>
          </Card>
        </div>

        {/* Sidebar */}
        <div className="space-y-4">
          {/* I/O Schemas */}
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg flex items-center gap-2">
                <FileCode className="w-5 h-5 text-muted-foreground" />
                Input / Output
              </CardTitle>
              <CardDescription>Schema references</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3 text-sm">
              <SummaryRow label="Input">
                <SchemaLink schema={plan.input} />
              </SummaryRow>
              {plan.output && (
                <SummaryRow label="Output">
                  <SchemaLink schema={plan.output} />
                </SummaryRow>
              )}
              {plan.locals && Object.keys(plan.locals).length > 0 && (
                <div className="pt-2 border-t border-border/50">
                  <span className="text-xs text-muted-foreground uppercase tracking-wider">
                    Local variables
                  </span>
                  <div className="mt-2 space-y-1">
                    {Object.entries(plan.locals).map(([varName, schema]) => (
                      <div
                        key={varName}
                        className="flex items-center justify-between gap-2"
                      >
                        <code className="font-mono text-xs">{varName}</code>
                        <span className="font-mono text-xs text-muted-foreground truncate">
                          {schema}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </CardContent>
          </Card>

          {/* Capabilities */}
          {plan.required_caps && plan.required_caps.length > 0 && (
            <Card className="bg-card/80">
              <CardHeader>
                <CardTitle className="text-lg flex items-center gap-2">
                  <Shield className="w-5 h-5 text-muted-foreground" />
                  Required capabilities
                </CardTitle>
                <CardDescription>
                  Capability grants needed to execute
                </CardDescription>
              </CardHeader>
              <CardContent>
                <div className="flex flex-wrap gap-2">
                  {plan.required_caps.map((cap) => (
                    <Badge key={cap} variant="outline" className="font-mono text-xs">
                      {cap}
                    </Badge>
                  ))}
                </div>
              </CardContent>
            </Card>
          )}

          {/* Allowed effects */}
          {plan.allowed_effects && plan.allowed_effects.length > 0 && (
            <Card className="bg-card/80">
              <CardHeader>
                <CardTitle className="text-lg flex items-center gap-2">
                  <Zap className="w-5 h-5 text-muted-foreground" />
                  Allowed effects
                </CardTitle>
                <CardDescription>Effect kinds this plan can emit</CardDescription>
              </CardHeader>
              <CardContent>
                <div className="flex flex-wrap gap-2">
                  {plan.allowed_effects.map((effect) => (
                    <Badge
                      key={effect}
                      variant="secondary"
                      className="font-mono text-xs"
                    >
                      {effect}
                    </Badge>
                  ))}
                </div>
              </CardContent>
            </Card>
          )}

          {/* Summary stats */}
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg flex items-center gap-2">
                <Info className="w-5 h-5 text-muted-foreground" />
                Summary
              </CardTitle>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              {data?.hash && (
                <div className="flex items-center justify-between gap-2">
                  <span className="text-muted-foreground flex items-center gap-1.5 shrink-0">
                    <Hash className="w-3.5 h-3.5" />
                    Hash
                  </span>
                  <CopyableHash hash={data.hash} />
                </div>
              )}
              <SummaryRow label="Steps">
                <span className="font-mono">{plan.steps.length}</span>
              </SummaryRow>
              <SummaryRow label="Edges">
                <span className="font-mono">{plan.edges.length}</span>
              </SummaryRow>
              <SummaryRow label="Conditional edges">
                <span className="font-mono">
                  {plan.edges.filter((e) => e.when).length}
                </span>
              </SummaryRow>
              {plan.invariants && (
                <SummaryRow label="Invariants">
                  <span className="font-mono">{plan.invariants.length}</span>
                </SummaryRow>
              )}
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}

function SummaryRow({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="flex items-center justify-between gap-2">
      <span className="text-muted-foreground shrink-0">{label}</span>
      <div className="text-right min-w-0 truncate">{children}</div>
    </div>
  );
}

function SchemaLink({ schema }: { schema: string }) {
  return (
    <Link
      to={`/manifest/defs/schema/${encodeURIComponent(schema)}`}
      className="font-mono text-xs hover:underline underline-offset-4"
    >
      {schema}
    </Link>
  );
}
