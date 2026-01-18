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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const workspaces = [
  { id: "core-runtime", status: "synced", head: "bafy...aa01", files: 148 },
  { id: "governance-lab", status: "dirty", head: "bafy...b221", files: 92 },
  { id: "workspace-dev", status: "synced", head: "bafy...ff90", files: 210 },
];

const syncRuns = [
  { workspace: "core-runtime", time: "12m ago", result: "success" },
  { workspace: "governance-lab", time: "35m ago", result: "warning" },
  { workspace: "workspace-dev", time: "2h ago", result: "success" },
];

export function WorkspacesIndexPage() {
  return (
    <div className="min-h-[calc(100dvh-7.5rem)] space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            Workspaces
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Manage versioned trees, review sync status, and open specific
            workspace snapshots.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">Sync all</Button>
          <Button variant="secondary">New workspace</Button>
        </div>
      </header>

      <div className="flex flex-wrap items-center gap-2">
        <Input
          placeholder="Search workspaces"
          className="w-full min-w-[240px] sm:w-72"
        />
        <Button variant="outline">Filters</Button>
      </div>

      <div className="grid gap-4 lg:grid-cols-[1.3fr_0.7fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Workspace list</CardTitle>
            <CardDescription>Latest synced heads and file counts.</CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>ID</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead>Head</TableHead>
                  <TableHead className="text-right">Files</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {workspaces.map((row) => (
                  <TableRow key={row.id}>
                    <TableCell className="font-medium">{row.id}</TableCell>
                    <TableCell>
                      <Badge
                        variant={row.status === "synced" ? "secondary" : "outline"}
                      >
                        {row.status}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-muted-foreground">
                      {row.head}
                    </TableCell>
                    <TableCell className="text-right text-muted-foreground">
                      {row.files}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>

        <div className="space-y-4">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Workspace health</CardTitle>
              <CardDescription>Quick signals to monitor.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3 text-sm">
              <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                <span>Clean trees</span>
                <Badge variant="secondary">8</Badge>
              </div>
              <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                <span>Pending syncs</span>
                <Badge variant="outline">2</Badge>
              </div>
              <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                <span>Large blobs</span>
                <Badge variant="outline">5</Badge>
              </div>
            </CardContent>
          </Card>

          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Recent syncs</CardTitle>
              <CardDescription>Last few push/pull runs.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              {syncRuns.map((run) => (
                <div
                  key={run.workspace}
                  className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2"
                >
                  <span className="font-medium text-foreground">
                    {run.workspace}
                  </span>
                  <span className="text-muted-foreground">{run.time}</span>
                  <Badge
                    variant={run.result === "success" ? "secondary" : "outline"}
                  >
                    {run.result}
                  </Badge>
                </div>
              ))}
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
