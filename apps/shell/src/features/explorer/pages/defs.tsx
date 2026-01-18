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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const defsByKind = {
  schemas: [
    { name: "sys/Workspace@1", scope: "core", updated: "2h ago" },
    { name: "sys/Plan@1", scope: "core", updated: "5h ago" },
    { name: "gov/Proposal@1", scope: "governance", updated: "1d ago" },
  ],
  modules: [
    { name: "world-reducer", scope: "core", updated: "3h ago" },
    { name: "governance-reducer", scope: "governance", updated: "1d ago" },
    { name: "workspace-reducer", scope: "storage", updated: "2d ago" },
  ],
  policies: [
    { name: "cap-governance", scope: "governance", updated: "6h ago" },
    { name: "cap-workspace", scope: "storage", updated: "1d ago" },
    { name: "cap-audit", scope: "audit", updated: "3d ago" },
  ],
};

const scopes = [
  "core",
  "governance",
  "storage",
  "audit",
  "workspace",
  "system",
];

export function DefsPage() {
  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="space-y-2">
        <Badge variant="secondary">Explorer</Badge>
        <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
          Defs browser
        </h1>
        <p className="max-w-2xl text-muted-foreground">
          Browse definitions by kind, prefix, and scope. Pin the ones you use
          most often.
        </p>
      </header>

      <div className="flex flex-wrap items-center gap-2">
        <Input
          placeholder="Filter defs by name, kind, or hash"
          className="w-full min-w-[240px] sm:w-72"
        />
        <Button variant="outline">Advanced filters</Button>
        <Button variant="secondary">Create def</Button>
      </div>

      <div className="grid gap-4 lg:grid-cols-[1.4fr_0.8fr]">
        <Tabs defaultValue="schemas" className="space-y-4">
          <TabsList>
            <TabsTrigger value="schemas">Schemas</TabsTrigger>
            <TabsTrigger value="modules">Modules</TabsTrigger>
            <TabsTrigger value="policies">Policies</TabsTrigger>
          </TabsList>

          {(["schemas", "modules", "policies"] as const).map((kind) => (
            <TabsContent key={kind} value={kind}>
              <Card className="bg-card/80">
                <CardHeader>
                  <CardTitle className="text-lg">{kind}</CardTitle>
                  <CardDescription>
                    {defsByKind[kind].length} results
                  </CardDescription>
                </CardHeader>
                <CardContent>
                  <Table>
                    <TableHeader>
                      <TableRow>
                        <TableHead>Name</TableHead>
                        <TableHead>Scope</TableHead>
                        <TableHead className="text-right">Updated</TableHead>
                      </TableRow>
                    </TableHeader>
                    <TableBody>
                      {defsByKind[kind].map((row) => (
                        <TableRow key={row.name}>
                          <TableCell className="font-medium">
                            {row.name}
                          </TableCell>
                          <TableCell>
                            <Badge variant="outline">{row.scope}</Badge>
                          </TableCell>
                          <TableCell className="text-right text-muted-foreground">
                            {row.updated}
                          </TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </CardContent>
              </Card>
            </TabsContent>
          ))}
        </Tabs>

        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Scopes</CardTitle>
            <CardDescription>
              Slice defs by namespace and policy group.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <ScrollArea className="h-56 pr-3">
              <div className="flex flex-wrap gap-2">
                {scopes.map((scope) => (
                  <Badge key={scope} variant="secondary">
                    {scope}
                  </Badge>
                ))}
              </div>
            </ScrollArea>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
