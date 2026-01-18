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
import { ArrowLeft, Copy, FileCode, GitBranch, Shield } from "lucide-react";

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
        <Link
          to="/explorer/defs"
          className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="w-4 h-4" />
          Back to definitions
        </Link>
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
        <Link
          to="/explorer/defs"
          className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="w-4 h-4" />
          Back to definitions
        </Link>
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
      <Link
        to="/explorer/defs"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="w-4 h-4" />
        Back to definitions
      </Link>

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

      <div className="grid gap-4 lg:grid-cols-[1.3fr_0.7fr]">
        {/* Main content: Steps */}
        <div className="space-y-4">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg flex items-center gap-2">
                <GitBranch className="w-5 h-5 text-muted-foreground" />
                Steps
              </CardTitle>
              <CardDescription>
                Execution flow with edges and conditions
              </CardDescription>
            </CardHeader>
            <CardContent>
              {plan.steps.length === 0 ? (
                <div className="py-8 text-center text-muted-foreground">
                  No steps defined
                </div>
              ) : (
                <PlanSteps steps={plan.steps} edges={plan.edges} />
              )}
            </CardContent>
          </Card>

          {/* Raw JSON */}
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Raw definition</CardTitle>
              <CardDescription>Full JSON representation</CardDescription>
            </CardHeader>
            <CardContent>
              <JsonViewer data={plan} className="max-h-96 overflow-y-auto" />
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
                <CardTitle className="text-lg">Allowed effects</CardTitle>
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
              <CardTitle className="text-lg">Summary</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
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
      to={`/explorer/defs/schema/${encodeURIComponent(schema)}`}
      className="font-mono text-xs hover:underline underline-offset-4"
    >
      {schema}
    </Link>
  );
}
