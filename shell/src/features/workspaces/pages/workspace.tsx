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
import { Input } from "@/components/ui/input";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Tabs,
  TabsContent,
  TabsList,
  TabsTrigger,
} from "@/components/ui/tabs";

export function WorkspacePage() {
  const { wsId } = useParams();
  const workspaceId = wsId ?? "workspace";

  const treeItems = [
    "src/app/shell-layout.tsx",
    "src/features/workspaces/pages/workspace.tsx",
    "spec/03-air.md",
    "spec/04-workflows.md",
    "modules/world/workflow.wasm",
  ];

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <Badge variant="secondary">Workspace</Badge>
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            {workspaceId}
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Inspect the file tree, review artifacts, and annotate snapshots.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">Open in editor</Button>
          <Button variant="secondary">Sync now</Button>
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[280px_1fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Explorer</CardTitle>
            <CardDescription>Tree view and quick access.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-3">
            <Input placeholder="Filter files" />
            <ScrollArea className="h-[340px] pr-3">
              <div className="space-y-2 text-sm">
                {treeItems.map((item) => (
                  <div
                    key={item}
                    className="rounded-md border bg-background/60 px-3 py-2"
                  >
                    {item}
                  </div>
                ))}
              </div>
            </ScrollArea>
          </CardContent>
        </Card>

        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">File preview</CardTitle>
            <CardDescription>Selected artifact snapshot.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <Tabs defaultValue="preview" className="space-y-3">
              <TabsList>
                <TabsTrigger value="preview">Preview</TabsTrigger>
                <TabsTrigger value="metadata">Metadata</TabsTrigger>
                <TabsTrigger value="history">History</TabsTrigger>
              </TabsList>
              <TabsContent value="preview">
                <div className="rounded-xl border border-dashed bg-muted/40 p-4 text-sm text-muted-foreground">
                  File preview placeholder. This panel renders the selected file
                  contents.
                </div>
              </TabsContent>
              <TabsContent value="metadata">
                <div className="grid gap-3 sm:grid-cols-2 text-sm">
                  <div className="rounded-lg border bg-background/60 p-3">
                    <div className="text-xs text-muted-foreground">Size</div>
                    <div className="font-medium text-foreground">42 KB</div>
                  </div>
                  <div className="rounded-lg border bg-background/60 p-3">
                    <div className="text-xs text-muted-foreground">Updated</div>
                    <div className="font-medium text-foreground">5m ago</div>
                  </div>
                  <div className="rounded-lg border bg-background/60 p-3">
                    <div className="text-xs text-muted-foreground">Type</div>
                    <div className="font-medium text-foreground">text/markdown</div>
                  </div>
                  <div className="rounded-lg border bg-background/60 p-3">
                    <div className="text-xs text-muted-foreground">Hash</div>
                    <div className="font-medium text-foreground">bafy...d4a0</div>
                  </div>
                </div>
              </TabsContent>
              <TabsContent value="history">
                <div className="space-y-2 text-sm">
                  <div className="rounded-md border bg-background/60 px-3 py-2">
                    Updated README and spec notes.
                  </div>
                  <div className="rounded-md border bg-background/60 px-3 py-2">
                    Synced workspace on 2025-01-08.
                  </div>
                  <div className="rounded-md border bg-background/60 px-3 py-2">
                    Added workflow build artifacts.
                  </div>
                </div>
              </TabsContent>
            </Tabs>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
