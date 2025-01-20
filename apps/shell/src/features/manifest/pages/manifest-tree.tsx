import { useState } from "react";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { useHealth, useManifest } from "@/sdk/queries";
import { Hash, Layers, Library, Loader2, Braces, Compass, Info } from "lucide-react";
import {
  ModulesSection,
  PlansSection,
  EventFlowSection,
  DefaultsSection,
  SupportingDefsSection,
} from "../components/manifest-tree";
import { CopyableHash } from "../components/copyable-hash";
import { JsonViewer } from "../components/json-viewer";
import type { Manifest } from "../lib/manifest-types";

export function ManifestTreePage() {
  const { data: health } = useHealth();
  const { data: manifestData, isLoading, error } = useManifest();
  const [viewMode, setViewMode] = useState<"structured" | "json">("structured");

  // Cast to our typed manifest
  const manifest = manifestData as Manifest | undefined;

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      {/* Header */}
      <header className="space-y-2">
        <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
          Manifest
        </h1>
        <p className="max-w-2xl text-muted-foreground">
          The manifest defines how modules, plans, and events are wired together
          in this world.
        </p>
      </header>

      {/* Error state */}
      {error && (
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="text-center space-y-2">
              <div className="text-destructive">Failed to load manifest</div>
              <div className="text-sm text-muted-foreground">
                {error.message}
              </div>
              <div className="text-xs text-muted-foreground">
                Make sure an AOS server is running and accessible.
              </div>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Loading state */}
      {isLoading && (
        <Card className="bg-card/80">
          <CardContent className="py-12">
            <div className="flex items-center justify-center gap-2">
              <Loader2 className="w-4 h-4 animate-spin text-muted-foreground" />
              <span className="text-muted-foreground">Loading manifest...</span>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Manifest content */}
      {manifest && !isLoading && (
        <div className="grid gap-4 lg:grid-cols-[1.4fr_0.6fr] min-w-0">
          {/* Main content */}
          <div className="space-y-4 min-w-0 overflow-hidden">
            <Card className="bg-card/80">
              <CardHeader>
                <div className="flex items-center justify-between">
                  <div>
                    <CardTitle className="text-lg flex items-center gap-2">
                      {viewMode === "structured" ? (
                        <Compass className="w-5 h-5 text-muted-foreground" />
                      ) : (
                        <Braces className="w-5 h-5 text-muted-foreground" />
                      )}
                      {viewMode === "structured" ? "Structure" : "Raw JSON"}
                    </CardTitle>
                    <CardDescription>
                      {viewMode === "structured"
                        ? "Modules, plans, and event flow"
                        : "Full JSON representation"}
                    </CardDescription>
                  </div>
                  <div className="flex rounded-lg border border-border p-0.5 bg-muted/30">
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
                {viewMode === "structured" ? (
                  <div className="space-y-1">
                    {/* Modules */}
                    <ModulesSection
                      modules={manifest.modules || []}
                      routing={manifest.routing}
                    />

                    <Separator className="my-3" />

                    {/* Plans */}
                    <PlansSection
                      plans={manifest.plans || []}
                      triggers={manifest.triggers}
                    />

                    <Separator className="my-3" />

                    {/* Event Flow */}
                    <EventFlowSection
                      routing={manifest.routing}
                      triggers={manifest.triggers}
                    />

                    <Separator className="my-3" />

                    {/* Defaults */}
                    <DefaultsSection defaults={manifest.defaults} />
                  </div>
                ) : (
                  <JsonViewer data={manifest} className="max-h-150 overflow-y-auto" />
                )}
              </CardContent>
            </Card>
          </div>

          {/* Sidebar */}
          <div className="space-y-4">
            {/* Summary */}
            <Card className="bg-card/80">
              <CardHeader>
                <CardTitle className="text-lg flex items-center gap-2">
                  <Info className="w-5 h-5 text-muted-foreground" />
                  Summary
                </CardTitle>
              </CardHeader>
              <CardContent className="space-y-3 text-sm">
                {health?.manifest_hash && (
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-muted-foreground flex items-center gap-1.5">
                      <Hash className="w-3.5 h-3.5" />
                      Hash
                    </span>
                    <CopyableHash hash={health.manifest_hash} />
                  </div>
                )}
                {health?.journal_height !== undefined && (
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-muted-foreground flex items-center gap-1.5">
                      <Layers className="w-3.5 h-3.5" />
                      Journal
                    </span>
                    <span className="font-mono">{health.journal_height}</span>
                  </div>
                )}
                <div className="flex items-center justify-between gap-2">
                  <span className="text-muted-foreground">Modules</span>
                  <span className="font-mono">{manifest.modules?.length ?? 0}</span>
                </div>
                <div className="flex items-center justify-between gap-2">
                  <span className="text-muted-foreground">Plans</span>
                  <span className="font-mono">{manifest.plans?.length ?? 0}</span>
                </div>
                <div className="flex items-center justify-between gap-2">
                  <span className="text-muted-foreground">Schemas</span>
                  <span className="font-mono">{manifest.schemas?.length ?? 0}</span>
                </div>
                <div className="flex items-center justify-between gap-2">
                  <span className="text-muted-foreground">Effects</span>
                  <span className="font-mono">{manifest.effects?.length ?? 0}</span>
                </div>
              </CardContent>
            </Card>

            {/* Supporting Defs */}
            <Card className="bg-card/80">
              <SupportingDefsSection
                schemas={manifest.schemas || []}
                effects={manifest.effects || []}
                caps={manifest.caps || []}
                policies={manifest.policies || []}
              />
            </Card>
          </div>
        </div>
      )}
    </div>
  );
}
