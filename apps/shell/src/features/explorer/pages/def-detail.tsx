import { useParams } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";

export function DefDetailPage() {
  const { kind, name } = useParams();
  const defKind = kind ?? "def";
  const defName = name ?? "untitled";

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <Badge variant="secondary">{defKind}</Badge>
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            {defName}
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Deep dive into the definition, its bindings, and its usage across
            plans and policies.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="secondary">Open in manifest</Button>
          <Button variant="outline">Copy reference</Button>
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[1.3fr_0.7fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Definition summary</CardTitle>
            <CardDescription>Schema envelope and notes.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <div className="rounded-lg border border-dashed bg-muted/40 p-4 text-sm text-muted-foreground">
              Schema JSON placeholder. This section will render the canonical
              definition for {defKind}/{defName}.
            </div>
            <Separator />
            <Tabs defaultValue="bindings" className="space-y-4">
              <TabsList>
                <TabsTrigger value="bindings">Bindings</TabsTrigger>
                <TabsTrigger value="usage">Usage</TabsTrigger>
                <TabsTrigger value="history">History</TabsTrigger>
              </TabsList>
              <TabsContent value="bindings">
                <div className="rounded-lg border bg-background/60 p-3 text-sm text-muted-foreground">
                  Capability bindings and effect allowlists placeholder.
                </div>
              </TabsContent>
              <TabsContent value="usage">
                <div className="rounded-lg border bg-background/60 p-3 text-sm text-muted-foreground">
                  Plan references and reducer modules placeholder.
                </div>
              </TabsContent>
              <TabsContent value="history">
                <div className="rounded-lg border bg-background/60 p-3 text-sm text-muted-foreground">
                  Version timeline placeholder.
                </div>
              </TabsContent>
            </Tabs>
          </CardContent>
        </Card>

        <div className="space-y-4">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Metadata</CardTitle>
              <CardDescription>Identifiers and hashes.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              <div className="flex items-center justify-between">
                <span className="text-muted-foreground">Kind</span>
                <Badge variant="outline">{defKind}</Badge>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-muted-foreground">Name</span>
                <span className="font-medium text-foreground">{defName}</span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-muted-foreground">Hash</span>
                <span className="font-medium text-foreground">bafy...0000</span>
              </div>
            </CardContent>
          </Card>

          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Related plans</CardTitle>
              <CardDescription>Plans referencing this def.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              <div className="rounded-md border bg-background/60 px-3 py-2">
                governance/approve
              </div>
              <div className="rounded-md border bg-background/60 px-3 py-2">
                workspace/sync
              </div>
              <div className="rounded-md border bg-background/60 px-3 py-2">
                audit/replay
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
