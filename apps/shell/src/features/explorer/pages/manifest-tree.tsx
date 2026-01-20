import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import { useHealth, useManifest } from "@/sdk/queries";
import { Hash, Loader2 } from "lucide-react";
import {
  ModulesSection,
  PlansSection,
  EventFlowSection,
  DefaultsSection,
  SupportingDefsSection,
} from "../components/manifest-tree";
import type { Manifest } from "../lib/manifest-types";

export function ManifestTreePage() {
  const { data: health } = useHealth();
  const { data: manifestData, isLoading, error } = useManifest();

  // Cast to our typed manifest
  const manifest = manifestData as Manifest | undefined;

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      {/* Header */}
      <header className="space-y-2">
        <Badge variant="secondary">Explorer</Badge>
        <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
          Manifest
        </h1>
        <p className="max-w-2xl text-muted-foreground">
          The manifest defines how modules, plans, and events are wired together
          in this world.
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

      {/* Manifest tree */}
      {manifest && !isLoading && (
        <Card className="bg-card/80">
          <CardContent className="py-4">
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

              <Separator className="my-3" />

              {/* Supporting Defs */}
              <SupportingDefsSection
                schemas={manifest.schemas || []}
                effects={manifest.effects || []}
                caps={manifest.caps || []}
                policies={manifest.policies || []}
              />
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
