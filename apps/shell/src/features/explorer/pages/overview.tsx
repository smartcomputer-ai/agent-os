import { useMemo } from "react";
import { Link } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { useHealth, useDefsList } from "@/sdk/queries";
import { DefsTable, DefsTableSkeleton } from "../components/defs-table";
import { KindBadge } from "../components/kind-badge";
import { API_DEF_KINDS, toDisplayKind } from "../types";
import { Database, FileCode, GitBranch, Hash, Layers } from "lucide-react";

export function ExplorerOverview() {
  const { data: health, isLoading: healthLoading } = useHealth();
  const { data: defsData, isLoading: defsLoading } = useDefsList();

  // Compute counts by kind (API returns "defschema", "defmodule", etc.)
  const counts = useMemo(() => {
    if (!defsData?.defs) return null;
    const map: Record<string, number> = {};
    for (const def of defsData.defs) {
      map[def.kind] = (map[def.kind] ?? 0) + 1;
    }
    return map;
  }, [defsData]);

  const totalDefs = defsData?.defs.length ?? 0;
  const planCount = counts?.defplan ?? 0;

  // Get first 8 defs for recent defs display
  const recentDefs = useMemo(() => {
    return (defsData?.defs ?? []).slice(0, 8);
  }, [defsData]);

  const quickLinks = [
    {
      title: "Manifest",
      description: "Schemas, modules, plans, effects, policies.",
      to: "/explorer/manifest",
      meta: counts ? `${Object.keys(counts).length} kinds` : "...",
      icon: Database,
    },
    {
      title: "Definitions",
      description: "Typed definitions with scope and bindings.",
      to: "/explorer/defs",
      meta: defsLoading ? "..." : `${totalDefs} defs`,
      icon: FileCode,
    },
    {
      title: "Plans",
      description: "DAG workflows with effect orchestration.",
      to: "/explorer/manifest?tab=defplan",
      meta: defsLoading ? "..." : `${planCount} plans`,
      icon: GitBranch,
    },
  ];

  return (
    <div className="min-h-[calc(100dvh-7.5rem)] space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="space-y-2">
        <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
          World Explorer
        </h1>
        <p className="max-w-2xl text-muted-foreground">
          Browse the manifest, definitions, and plan topology powering this
          world.
        </p>
      </header>

      {/* World stats */}
      <div className="flex flex-wrap items-center gap-4">
        <div className="flex items-center gap-2 text-sm">
          <Hash className="w-4 h-4 text-muted-foreground" />
          <span className="text-muted-foreground">Manifest:</span>
          {healthLoading ? (
            <div className="h-4 w-24 bg-muted animate-pulse rounded" />
          ) : health?.manifest_hash ? (
            <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded">
              {health.manifest_hash.slice(0, 16)}...
            </code>
          ) : (
            <span className="text-muted-foreground">-</span>
          )}
        </div>
        <div className="flex items-center gap-2 text-sm">
          <Layers className="w-4 h-4 text-muted-foreground" />
          <span className="text-muted-foreground">Journal:</span>
          {healthLoading ? (
            <div className="h-4 w-12 bg-muted animate-pulse rounded" />
          ) : (
            <span className="font-mono">{health?.journal_height ?? "-"}</span>
          )}
        </div>
      </div>

      {/* Quick links */}
      <div className="grid gap-4 md:grid-cols-3">
        {quickLinks.map((link) => (
          <Card key={link.title} className="bg-card/80">
            <CardHeader>
              <div className="flex items-center gap-2">
                <link.icon className="w-5 h-5 text-muted-foreground" />
                <CardTitle className="text-lg">{link.title}</CardTitle>
              </div>
              <CardDescription>{link.description}</CardDescription>
            </CardHeader>
            <CardContent>
              <Badge variant="outline">{link.meta}</Badge>
            </CardContent>
            <CardFooter>
              <Button asChild variant="secondary" className="w-full">
                <Link to={link.to}>Open {link.title}</Link>
              </Button>
            </CardFooter>
          </Card>
        ))}
      </div>

      {/* Def counts by kind + recent defs */}
      <div className="grid gap-4 lg:grid-cols-[1fr_1.5fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Definition counts</CardTitle>
            <CardDescription>Breakdown by kind</CardDescription>
          </CardHeader>
          <CardContent>
            {defsLoading ? (
              <div className="space-y-2">
                {Array.from({ length: 6 }).map((_, i) => (
                  <div
                    key={i}
                    className="flex items-center justify-between py-1"
                  >
                    <div className="h-5 w-20 bg-muted animate-pulse rounded-full" />
                    <div className="h-4 w-8 bg-muted animate-pulse rounded" />
                  </div>
                ))}
              </div>
            ) : (
              <div className="space-y-2">
                {API_DEF_KINDS.map((kind) => (
                  <div
                    key={kind}
                    className="flex items-center justify-between py-1"
                  >
                    <KindBadge kind={toDisplayKind(kind)} />
                    <span className="font-mono text-sm tabular-nums">
                      {counts?.[kind] ?? 0}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </CardContent>
          <CardFooter>
            <Button asChild variant="outline" className="w-full">
              <Link to="/explorer/defs">Browse all defs</Link>
            </Button>
          </CardFooter>
        </Card>

        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Recent definitions</CardTitle>
            <CardDescription>First definitions in manifest</CardDescription>
          </CardHeader>
          <CardContent>
            {defsLoading ? (
              <DefsTableSkeleton rows={5} />
            ) : (
              <DefsTable defs={recentDefs} />
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
