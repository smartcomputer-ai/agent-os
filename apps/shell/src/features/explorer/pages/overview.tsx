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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

const quickLinks = [
  {
    title: "Manifest",
    description: "Schemas, modules, plans, effects, policies.",
    to: "/explorer/manifest",
    meta: "12 sections",
  },
  {
    title: "Defs",
    description: "Typed definitions with scope and bindings.",
    to: "/explorer/defs",
    meta: "64 defs",
  },
  {
    title: "Plan diagrams",
    description: "DAG layouts with effect edges.",
    to: "/explorer/plans/example",
    meta: "7 active plans",
  },
];

const activity = [
  { event: "Manifest indexed", target: "sys/Workspace@1", time: "2m ago" },
  { event: "Plan shadowed", target: "gov/approve", time: "12m ago" },
  { event: "Defs cached", target: "schemas/core", time: "28m ago" },
];

const pinnedDefs = [
  { name: "sys/Workspace@1", kind: "schema" },
  { name: "sys/Plan@1", kind: "schema" },
  { name: "gov/Proposal@1", kind: "schema" },
];

export function ExplorerOverview() {
  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="space-y-2">
        <Badge variant="secondary">Read only</Badge>
        <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
          World Explorer
        </h1>
        <p className="max-w-2xl text-muted-foreground">
          A focused snapshot of the manifest, definitions, and plan topology
          powering this world.
        </p>
      </header>

      <div className="grid gap-4 md:grid-cols-3">
        {quickLinks.map((link) => (
          <Card key={link.title} className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">{link.title}</CardTitle>
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

      <div className="grid gap-4 lg:grid-cols-[1.2fr_0.8fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Recent activity</CardTitle>
            <CardDescription>Latest explorer snapshots.</CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Event</TableHead>
                  <TableHead>Target</TableHead>
                  <TableHead className="text-right">When</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {activity.map((row) => (
                  <TableRow key={row.event}>
                    <TableCell className="font-medium">{row.event}</TableCell>
                    <TableCell className="text-muted-foreground">
                      {row.target}
                    </TableCell>
                    <TableCell className="text-right text-muted-foreground">
                      {row.time}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Pinned defs</CardTitle>
            <CardDescription>Jump back into high traffic nodes.</CardDescription>
          </CardHeader>
          <CardContent className="space-y-2">
            {pinnedDefs.map((def) => (
              <div
                key={def.name}
                className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2 text-sm"
              >
                <span className="font-medium text-foreground">{def.name}</span>
                <Badge variant="outline">{def.kind}</Badge>
              </div>
            ))}
          </CardContent>
          <CardFooter>
            <Button asChild variant="outline" className="w-full">
              <Link to="/explorer/defs">Browse all defs</Link>
            </Button>
          </CardFooter>
        </Card>
      </div>
    </div>
  );
}
