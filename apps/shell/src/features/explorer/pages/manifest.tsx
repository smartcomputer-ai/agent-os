import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const schemaRows = [
  { name: "sys/Workspace@1", hash: "bafy...4ad2", scope: "core" },
  { name: "sys/Plan@1", hash: "bafy...9aa1", scope: "core" },
  { name: "gov/Proposal@1", hash: "bafy...2c91", scope: "governance" },
];

const moduleRows = [
  { name: "world-reducer", hash: "bafy...7d10", runtime: "wasm32" },
  { name: "governance-reducer", hash: "bafy...44ff", runtime: "wasm32" },
  { name: "workspace-reducer", hash: "bafy...83c0", runtime: "wasm32" },
];

const planRows = [
  { name: "governance/approve", effects: "http, llm", status: "active" },
  { name: "workspace/sync", effects: "blob, timer", status: "idle" },
  { name: "audit/replay", effects: "blob", status: "active" },
];

const policyRows = [
  { name: "cap-governance", boundTo: "gov/Proposal@1", status: "enforced" },
  { name: "cap-workspace", boundTo: "sys/Workspace@1", status: "enforced" },
  { name: "cap-audit", boundTo: "sys/Plan@1", status: "monitor" },
];

export function ManifestPage() {
  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <Badge variant="secondary">Explorer</Badge>
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            Manifest
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Structured catalog of schemas, modules, plans, effects, and policy
            bindings.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">Export snapshot</Button>
          <Button variant="secondary">View raw</Button>
        </div>
      </header>

      <Tabs defaultValue="schemas" className="space-y-4">
        <TabsList>
          <TabsTrigger value="schemas">Schemas</TabsTrigger>
          <TabsTrigger value="modules">Modules</TabsTrigger>
          <TabsTrigger value="plans">Plans</TabsTrigger>
          <TabsTrigger value="policies">Policies</TabsTrigger>
        </TabsList>

        <TabsContent value="schemas">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Schemas</CardTitle>
              <CardDescription>Typed definitions with hashes.</CardDescription>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Hash</TableHead>
                    <TableHead className="text-right">Scope</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {schemaRows.map((row) => (
                    <TableRow key={row.name}>
                      <TableCell className="font-medium">{row.name}</TableCell>
                      <TableCell className="text-muted-foreground">
                        {row.hash}
                      </TableCell>
                      <TableCell className="text-right">
                        <Badge variant="outline">{row.scope}</Badge>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="modules">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Modules</CardTitle>
              <CardDescription>Reducer bundles and runtimes.</CardDescription>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Hash</TableHead>
                    <TableHead className="text-right">Runtime</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {moduleRows.map((row) => (
                    <TableRow key={row.name}>
                      <TableCell className="font-medium">{row.name}</TableCell>
                      <TableCell className="text-muted-foreground">
                        {row.hash}
                      </TableCell>
                      <TableCell className="text-right text-muted-foreground">
                        {row.runtime}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="plans">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Plans</CardTitle>
              <CardDescription>Governed orchestration DAGs.</CardDescription>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Effects</TableHead>
                    <TableHead className="text-right">Status</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {planRows.map((row) => (
                    <TableRow key={row.name}>
                      <TableCell className="font-medium">{row.name}</TableCell>
                      <TableCell className="text-muted-foreground">
                        {row.effects}
                      </TableCell>
                      <TableCell className="text-right">
                        <Badge
                          variant={row.status === "active" ? "secondary" : "outline"}
                        >
                          {row.status}
                        </Badge>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </TabsContent>

        <TabsContent value="policies">
          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Policies</CardTitle>
              <CardDescription>Capability constraints and gates.</CardDescription>
            </CardHeader>
            <CardContent>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Name</TableHead>
                    <TableHead>Bound to</TableHead>
                    <TableHead className="text-right">Status</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {policyRows.map((row) => (
                    <TableRow key={row.name}>
                      <TableCell className="font-medium">{row.name}</TableCell>
                      <TableCell className="text-muted-foreground">
                        {row.boundTo}
                      </TableCell>
                      <TableCell className="text-right">
                        <Badge
                          variant={
                            row.status === "enforced" ? "secondary" : "outline"
                          }
                        >
                          {row.status}
                        </Badge>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
    </div>
  );
}
