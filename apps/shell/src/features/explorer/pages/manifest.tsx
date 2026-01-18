import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { useHealth, useDefsList } from "@/sdk/queries";
import { DefsTable, DefsTableSkeleton } from "../components/defs-table";
import { API_DEF_KINDS, DEF_KIND_LABELS, toDisplayKind } from "../types";
import { Hash } from "lucide-react";

export function ManifestPage() {
  const [searchParams, setSearchParams] = useSearchParams();
  const activeTab = searchParams.get("tab") ?? "defschema";

  const { data: health } = useHealth();
  const { data: defsData, isLoading, error } = useDefsList();

  // Group defs by kind (API returns kinds like "defschema", "defmodule", etc.)
  const defsByKind = useMemo(() => {
    if (!defsData?.defs) return {};
    const map: Record<string, typeof defsData.defs> = {};
    for (const def of defsData.defs) {
      if (!map[def.kind]) {
        map[def.kind] = [];
      }
      map[def.kind].push(def);
    }
    return map;
  }, [defsData]);

  // Get counts for tab badges
  const counts = useMemo(() => {
    const map: Record<string, number> = {};
    for (const [kind, defs] of Object.entries(defsByKind)) {
      map[kind] = defs.length;
    }
    return map;
  }, [defsByKind]);

  const handleTabChange = (value: string) => {
    setSearchParams({ tab: value });
  };

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="space-y-2">
        <Badge variant="secondary">Explorer</Badge>
        <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
          Manifest
        </h1>
        <p className="max-w-2xl text-muted-foreground">
          Structured catalog of schemas, modules, plans, effects, capabilities,
          and policies.
        </p>
        {health?.manifest_hash && (
          <div className="flex items-center gap-2 text-sm pt-1">
            <Hash className="w-4 h-4 text-muted-foreground" />
            <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded">
              {health.manifest_hash}
            </code>
          </div>
        )}
      </header>

      {error ? (
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center space-y-2">
              <div className="text-destructive">
                Failed to load definitions
              </div>
              <div className="text-sm text-muted-foreground">
                {error.message}
              </div>
              <div className="text-xs text-muted-foreground">
                Make sure an AOS server is running and accessible.
              </div>
            </div>
          </CardContent>
        </Card>
      ) : !isLoading && defsData?.defs.length === 0 ? (
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center space-y-2">
              <div className="text-muted-foreground">
                No definitions found
              </div>
              <div className="text-xs text-muted-foreground">
                This world's manifest appears to be empty.
              </div>
            </div>
          </CardContent>
        </Card>
      ) : (
        <Tabs value={activeTab} onValueChange={handleTabChange} className="space-y-4">
          <TabsList className="flex-wrap h-auto gap-1">
            {API_DEF_KINDS.map((kind) => (
              <TabsTrigger key={kind} value={kind} className="gap-1.5">
                {DEF_KIND_LABELS[kind]}
                <span className="text-xs text-muted-foreground tabular-nums">
                  {isLoading ? "..." : counts[kind] ?? 0}
                </span>
              </TabsTrigger>
            ))}
          </TabsList>

          {API_DEF_KINDS.map((kind) => (
            <TabsContent key={kind} value={kind}>
              <Card className="bg-card/80">
                <CardHeader>
                  <CardTitle className="text-lg">{DEF_KIND_LABELS[kind]}</CardTitle>
                  <CardDescription>{getKindDescription(toDisplayKind(kind))}</CardDescription>
                </CardHeader>
                <CardContent>
                  {isLoading ? (
                    <DefsTableSkeleton rows={8} showKind={false} />
                  ) : (
                    <DefsTable defs={defsByKind[kind] ?? []} showKind={false} />
                  )}
                </CardContent>
              </Card>
            </TabsContent>
          ))}
        </Tabs>
      )}
    </div>
  );
}

function getKindDescription(kind: string): string {
  switch (kind) {
    case "schema":
      return "Type definitions for all values in the system.";
    case "module":
      return "WASM reducers and pure functions with their ABIs.";
    case "plan":
      return "DAG orchestrations for external effects.";
    case "effect":
      return "External action definitions (HTTP, LLM, storage).";
    case "cap":
      return "Capability types with parameter constraints.";
    case "policy":
      return "Access control rules for effects.";
    default:
      return "";
  }
}
