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
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";

export function PlanDiagramPage() {
  const { name } = useParams();
  const planName = name ?? "unnamed-plan";

  const nodes = [
    { step: "Validate intent", type: "predicate", status: "ready" },
    { step: "HTTP fetch", type: "effect", status: "pending" },
    { step: "Emit receipt", type: "event", status: "queued" },
  ];

  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <Badge variant="secondary">Plan</Badge>
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            {planName}
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Visualize plan topology, effect edges, and receipt flow in a single
            view.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">View receipts</Button>
          <Button variant="secondary">Open trace</Button>
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[1.3fr_0.7fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">DAG canvas</CardTitle>
            <CardDescription>Node graph placeholder.</CardDescription>
          </CardHeader>
          <CardContent>
            <div className="flex min-h-[320px] items-center justify-center rounded-xl border border-dashed bg-muted/40 text-sm text-muted-foreground">
              Canvas rendering placeholder. Nodes, edges, and receipts will
              render here.
            </div>
          </CardContent>
        </Card>

        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Node status</CardTitle>
            <CardDescription>Execution checkpoints.</CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Step</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead className="text-right">State</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {nodes.map((node) => (
                  <TableRow key={node.step}>
                    <TableCell className="font-medium">{node.step}</TableCell>
                    <TableCell className="text-muted-foreground">
                      {node.type}
                    </TableCell>
                    <TableCell className="text-right">
                      <Badge
                        variant={
                          node.status === "ready" ? "secondary" : "outline"
                        }
                      >
                        {node.status}
                      </Badge>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
