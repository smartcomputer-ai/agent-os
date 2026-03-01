import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";

export function GovernanceDraftPage() {
  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <Badge variant="secondary">Governance</Badge>
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            Draft proposal
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Compose a patch document, run shadow, and collect approvals before
            apply.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">Validate</Button>
          <Button variant="secondary">Run shadow</Button>
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[0.8fr_1.2fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Proposal metadata</CardTitle>
            <CardDescription>Identity and routing.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <div className="space-y-2">
              <label className="text-xs font-medium text-muted-foreground">
                Title
              </label>
              <Input placeholder="Enable workspace sync gating" />
            </div>
            <div className="space-y-2">
              <label className="text-xs font-medium text-muted-foreground">
                Owner
              </label>
              <Input placeholder="core-team" />
            </div>
            <div className="space-y-2">
              <label className="text-xs font-medium text-muted-foreground">
                Rollout window
              </label>
              <Input placeholder="2025-01-12 10:00 UTC" />
            </div>
          </CardContent>
        </Card>

        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Patch document</CardTitle>
            <CardDescription>JSON patch editor placeholder.</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="min-h-[360px] rounded-xl border border-dashed bg-muted/40 p-4 text-sm text-muted-foreground">
              Draft editor placeholder. This will host schema validation, diff
              preview, and shadow results.
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
