import { useParams, Navigate } from "react-router-dom";
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
import { CopyableHash } from "../components/copyable-hash";
import {
  isModuleDef,
  isSchemaDef,
  isEffectDef,
  isCapDef,
  isPolicyDef,
  type DefKind,
  type ModuleDef,
  type SchemaDef,
  type EffectDef,
  type CapDef,
  type PolicyDef,
} from "../types";
import { Braces, Copy, Hash, Info } from "lucide-react";

export function DefDetailPage() {
  const { kind, name } = useParams();

  // Redirect plans to the plan detail page
  if (kind === "plan" && name) {
    return <Navigate to={`/manifest/plans/${encodeURIComponent(name)}`} replace />;
  }

  if (!kind || !name) {
    return (
      <div className="py-12 text-center text-muted-foreground">
        Invalid definition path
      </div>
    );
  }

  return <DefDetailContent kind={kind} name={name} />;
}

function DefDetailContent({ kind, name }: { kind: string; name: string }) {
  const { data, isLoading, error } = useDefsGet({ kind, name });

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
        <div className="grid gap-4 lg:grid-cols-[1.4fr_0.6fr]">
          <Card className="bg-card/80">
            <CardHeader>
              <div className="h-5 w-32 bg-muted animate-pulse rounded" />
            </CardHeader>
            <CardContent>
              <div className="h-64 bg-muted animate-pulse rounded" />
            </CardContent>
          </Card>
          <Card className="bg-card/80">
            <CardHeader>
              <div className="h-5 w-24 bg-muted animate-pulse rounded" />
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
    );
  }

  if (error) {
    return (
      <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center text-destructive">
              Failed to load definition: {error.message}
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  const def = data?.def;

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-start sm:justify-between">
        <div className="space-y-2">
          <KindBadge kind={kind as DefKind} />
          <h1 className="text-2xl font-semibold tracking-tight text-foreground font-mono break-all">
            {name}
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            {getKindDescription(kind)}
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={copyToClipboard}>
          <Copy className="w-4 h-4 mr-1" />
          Copy name
        </Button>
      </header>

      <div className="grid gap-4 lg:grid-cols-[1.4fr_0.6fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg flex items-center gap-2">
              <Braces className="w-5 h-5 text-muted-foreground" />
              Definition content
            </CardTitle>
            <CardDescription>Full JSON representation</CardDescription>
          </CardHeader>
          <CardContent>
            <JsonViewer data={def} className="max-h-[600px] overflow-y-auto" />
          </CardContent>
        </Card>

        <div className="space-y-4">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg flex items-center gap-2">
                <Info className="w-5 h-5 text-muted-foreground" />
                Summary
              </CardTitle>
              <CardDescription>Key properties</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3 text-sm">
              <DefSummary def={def} kind={kind} name={name} hash={data?.hash} />
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}

function DefSummary({
  def,
  kind,
  name,
  hash,
}: {
  def: unknown;
  kind: string;
  name: string;
  hash?: string;
}) {
  return (
    <div className="space-y-2">
      {hash && (
        <div className="flex items-center justify-between gap-2">
          <span className="text-muted-foreground flex items-center gap-1.5 shrink-0">
            <Hash className="w-3.5 h-3.5" />
            Hash
          </span>
          <CopyableHash hash={hash} />
        </div>
      )}
      <SummaryRow label="Kind">
        <KindBadge kind={kind as DefKind} />
      </SummaryRow>
      <SummaryRow label="Name">
        <span className="font-mono text-xs">{name}</span>
      </SummaryRow>

      {isModuleDef(def) && <ModuleSummary def={def} />}
      {isSchemaDef(def) && <SchemaSummary def={def} />}
      {isEffectDef(def) && <EffectSummary def={def} />}
      {isCapDef(def) && <CapSummary def={def} />}
      {isPolicyDef(def) && <PolicySummary def={def} />}
    </div>
  );
}

function ModuleSummary({ def }: { def: ModuleDef }) {
  return (
    <>
      <SummaryRow label="Module kind">
        <Badge variant="outline">{def.module_kind}</Badge>
      </SummaryRow>
      {def.wasm_hash && (
        <SummaryRow label="WASM hash">
          <CopyableHash hash={def.wasm_hash} truncate={20} />
        </SummaryRow>
      )}
      {def.abi?.workflow && (
        <>
          <SummaryRow label="State schema">
            <span className="font-mono text-xs">{def.abi.workflow.state}</span>
          </SummaryRow>
          <SummaryRow label="Event schema">
            <span className="font-mono text-xs">{def.abi.workflow.event}</span>
          </SummaryRow>
        </>
      )}
      {def.abi?.pure && (
        <>
          <SummaryRow label="Input schema">
            <span className="font-mono text-xs">{def.abi.pure.input}</span>
          </SummaryRow>
          <SummaryRow label="Output schema">
            <span className="font-mono text-xs">{def.abi.pure.output}</span>
          </SummaryRow>
        </>
      )}
    </>
  );
}

function SchemaSummary({ def }: { def: SchemaDef }) {
  const typeKind = def.type && typeof def.type === "object"
    ? Object.keys(def.type)[0]
    : "unknown";
  return (
    <SummaryRow label="Type kind">
      <Badge variant="outline">{typeKind}</Badge>
    </SummaryRow>
  );
}

function EffectSummary({ def }: { def: EffectDef }) {
  return (
    <>
      <SummaryRow label="Effect kind">
        <Badge variant="outline">{def.kind}</Badge>
      </SummaryRow>
      <SummaryRow label="Cap type">
        <span className="font-mono text-xs">{def.cap_type}</span>
      </SummaryRow>
      <SummaryRow label="Params schema">
        <span className="font-mono text-xs">{def.params_schema}</span>
      </SummaryRow>
      <SummaryRow label="Receipt schema">
        <span className="font-mono text-xs">{def.receipt_schema}</span>
      </SummaryRow>
    </>
  );
}

function CapSummary({ def }: { def: CapDef }) {
  return (
    <>
      <SummaryRow label="Cap type">
        <Badge variant="outline">{def.cap_type}</Badge>
      </SummaryRow>
      <SummaryRow label="Enforcer">
        <span className="font-mono text-xs">{def.enforcer.module}</span>
      </SummaryRow>
    </>
  );
}

function PolicySummary({ def }: { def: PolicyDef }) {
  const allowCount = def.rules.filter((r) => r.decision === "allow").length;
  const denyCount = def.rules.filter((r) => r.decision === "deny").length;
  return (
    <>
      <SummaryRow label="Total rules">
        <span className="font-mono">{def.rules.length}</span>
      </SummaryRow>
      <SummaryRow label="Allow rules">
        <span className="font-mono text-green-600 dark:text-green-400">{allowCount}</span>
      </SummaryRow>
      <SummaryRow label="Deny rules">
        <span className="font-mono text-red-600 dark:text-red-400">{denyCount}</span>
      </SummaryRow>
    </>
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

function getKindDescription(kind: string): string {
  switch (kind) {
    case "schema":
      return "Type definition for values in the system.";
    case "module":
      return "WASM workflow or pure function with ABI.";
    case "effect":
      return "External action definition.";
    case "cap":
      return "Capability type with parameter constraints.";
    case "policy":
      return "Access control rules for effects.";
    default:
      return "Definition details.";
  }
}
