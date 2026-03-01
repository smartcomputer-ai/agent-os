import { useState, useMemo } from "react";
import { Badge } from "@/components/ui/badge";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";
import { useDefsList } from "@/sdk/queries";
import { DefsTable, DefsTableSkeleton } from "../components/defs-table";
import { KindBadge } from "../components/kind-badge";
import { API_DEF_KINDS, DEF_KIND_LABELS, toDisplayKind } from "../types";
import { Search, FolderTree, LayoutGrid } from "lucide-react";

export function DefsPage() {
  const [searchTerm, setSearchTerm] = useState("");
  const [activeTab, setActiveTab] = useState<string>("all");

  const { data, isLoading, error } = useDefsList();

  // Filter defs based on search and active tab
  const filteredDefs = useMemo(() => {
    if (!data?.defs) return [];

    let filtered = data.defs;

    // Filter by kind if not "all" (API returns "defschema", etc.)
    if (activeTab !== "all") {
      filtered = filtered.filter((def) => def.kind === activeTab);
    }

    // Filter by search term (case-insensitive name match)
    if (searchTerm.trim()) {
      const term = searchTerm.toLowerCase();
      filtered = filtered.filter((def) =>
        def.name.toLowerCase().includes(term)
      );
    }

    return filtered;
  }, [data, activeTab, searchTerm]);

  // Compute counts for tab badges (API returns "defschema", etc.)
  const counts = useMemo(() => {
    if (!data?.defs) return {};
    const map: Record<string, number> = { all: data.defs.length };
    for (const def of data.defs) {
      map[def.kind] = (map[def.kind] ?? 0) + 1;
    }
    return map;
  }, [data]);

  // Extract unique namespaces for sidebar
  const namespaces = useMemo(() => {
    if (!data?.defs) return [];
    const nsSet = new Set<string>();
    for (const def of data.defs) {
      const match = def.name.match(/^([^/]+)\//);
      if (match) {
        nsSet.add(match[1]);
      }
    }
    return Array.from(nsSet).sort();
  }, [data]);

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="space-y-2">
        <h1 className="text-3xl font-semibold tracking-tight text-foreground">
          All Definitions
        </h1>
        <p className="max-w-2xl text-muted-foreground">
          Browse all definitions by kind. Click a definition to view its full
          content.
        </p>
      </header>

      {/* Search */}
      <div className="relative max-w-md">
        <Search className="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
        <Input
          placeholder="Filter by name..."
          value={searchTerm}
          onChange={(e) => setSearchTerm(e.target.value)}
          className="pl-9"
        />
      </div>

      <div className="grid gap-4 lg:grid-cols-[1.4fr_0.6fr]">
        {/* Main content with tabs */}
        <Tabs value={activeTab} onValueChange={setActiveTab} className="space-y-4">
          <TabsList className="flex-wrap h-auto gap-1">
            <TabsTrigger value="all" className="gap-1.5">
              All
              <span className="text-xs text-muted-foreground tabular-nums">
                {counts.all ?? 0}
              </span>
            </TabsTrigger>
            {API_DEF_KINDS.map((kind) => (
              <TabsTrigger key={kind} value={kind} className="gap-1.5">
                {DEF_KIND_LABELS[kind]}
                <span className="text-xs text-muted-foreground tabular-nums">
                  {counts[kind] ?? 0}
                </span>
              </TabsTrigger>
            ))}
          </TabsList>

          <TabsContent value={activeTab} className="mt-4">
            <Card className="bg-card/80">
              <CardHeader>
                <CardTitle className="text-lg">
                  {activeTab === "all" ? "All definitions" : DEF_KIND_LABELS[activeTab]}
                </CardTitle>
                <CardDescription>
                  {isLoading
                    ? "Loading..."
                    : `${filteredDefs.length} definition${filteredDefs.length === 1 ? "" : "s"}`}
                </CardDescription>
              </CardHeader>
              <CardContent>
                {error ? (
                  <div className="py-8 text-center text-destructive">
                    Failed to load definitions: {error.message}
                  </div>
                ) : isLoading ? (
                  <DefsTableSkeleton rows={10} showKind={activeTab === "all"} />
                ) : (
                  <DefsTable defs={filteredDefs} showKind={activeTab === "all"} />
                )}
              </CardContent>
            </Card>
          </TabsContent>
        </Tabs>

        {/* Sidebar: namespaces */}
        <div className="space-y-4">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg flex items-center gap-2">
                <FolderTree className="w-5 h-5 text-muted-foreground" />
                Namespaces
              </CardTitle>
              <CardDescription>Definition prefixes in this world</CardDescription>
            </CardHeader>
            <CardContent>
              {isLoading ? (
                <div className="space-y-2">
                  {Array.from({ length: 4 }).map((_, i) => (
                    <div
                      key={i}
                      className="h-6 w-20 bg-muted animate-pulse rounded-full"
                    />
                  ))}
                </div>
              ) : namespaces.length === 0 ? (
                <p className="text-sm text-muted-foreground">No namespaces found</p>
              ) : (
                <div className="flex flex-wrap gap-2">
                  {namespaces.map((ns) => (
                    <Badge
                      key={ns}
                      variant="secondary"
                      className="cursor-pointer hover:bg-secondary/80"
                      onClick={() => setSearchTerm(ns + "/")}
                    >
                      {ns}
                    </Badge>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>

          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg flex items-center gap-2">
                <LayoutGrid className="w-5 h-5 text-muted-foreground" />
                Kind summary
              </CardTitle>
              <CardDescription>Definitions by type</CardDescription>
            </CardHeader>
            <CardContent>
              {isLoading ? (
                <div className="space-y-2">
                  {Array.from({ length: 6 }).map((_, i) => (
                    <div key={i} className="flex justify-between">
                      <div className="h-5 w-16 bg-muted animate-pulse rounded-full" />
                      <div className="h-4 w-6 bg-muted animate-pulse rounded" />
                    </div>
                  ))}
                </div>
              ) : (
                <div className="space-y-2">
                  {API_DEF_KINDS.map((kind) => (
                    <button
                      key={kind}
                      onClick={() => setActiveTab(kind)}
                      className="w-full flex items-center justify-between py-1 hover:bg-muted/50 rounded px-1 -mx-1 transition-colors"
                    >
                      <KindBadge kind={toDisplayKind(kind)} />
                      <span className="font-mono text-sm tabular-nums text-muted-foreground">
                        {counts[kind] ?? 0}
                      </span>
                    </button>
                  ))}
                </div>
              )}
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
