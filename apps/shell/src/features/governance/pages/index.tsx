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

const proposals = [
  {
    id: "gov-2025-01",
    title: "Enable workspace sync gating",
    stage: "shadow",
    owner: "core-team",
  },
  {
    id: "gov-2025-02",
    title: "Approve adapter allowlist",
    stage: "review",
    owner: "policy",
  },
  {
    id: "gov-2025-03",
    title: "Rotate manifest keys",
    stage: "approved",
    owner: "security",
  },
];

export function GovernanceIndexPage() {
  return (
    <div className="space-y-6 animate-in fade-in-0 slide-in-from-bottom-2">
      <header className="flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="space-y-2">
          <Badge variant="secondary">Governance</Badge>
          <h1 className="text-3xl font-semibold tracking-tight text-foreground font-[var(--font-display)]">
            Proposals
          </h1>
          <p className="max-w-2xl text-muted-foreground">
            Track proposals through shadow, approval, and apply phases with a
            clean audit trail.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline">Open ledger</Button>
          <Button variant="secondary">New proposal</Button>
        </div>
      </header>

      <div className="grid gap-4 lg:grid-cols-[1.2fr_0.8fr]">
        <Card className="bg-card/80">
          <CardHeader>
            <CardTitle className="text-lg">Active proposals</CardTitle>
            <CardDescription>Latest governance items.</CardDescription>
          </CardHeader>
          <CardContent>
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>ID</TableHead>
                  <TableHead>Title</TableHead>
                  <TableHead>Owner</TableHead>
                  <TableHead className="text-right">Stage</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {proposals.map((row) => (
                  <TableRow key={row.id}>
                    <TableCell className="font-medium">{row.id}</TableCell>
                    <TableCell>{row.title}</TableCell>
                    <TableCell className="text-muted-foreground">
                      {row.owner}
                    </TableCell>
                    <TableCell className="text-right">
                      <Badge
                        variant={
                          row.stage === "approved" ? "secondary" : "outline"
                        }
                      >
                        {row.stage}
                      </Badge>
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
              <CardTitle className="text-lg">Stage counts</CardTitle>
              <CardDescription>Governance pipeline snapshot.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-3 text-sm">
              <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                <span>Drafts</span>
                <Badge variant="outline">3</Badge>
              </div>
              <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                <span>Shadowing</span>
                <Badge variant="secondary">2</Badge>
              </div>
              <div className="flex items-center justify-between rounded-lg border bg-background/60 px-3 py-2">
                <span>Approvals</span>
                <Badge variant="outline">1</Badge>
              </div>
            </CardContent>
          </Card>

          <Card className="bg-card/80">
            <CardHeader>
              <CardTitle className="text-lg">Next actions</CardTitle>
              <CardDescription>Queue for reviewers.</CardDescription>
            </CardHeader>
            <CardContent className="space-y-2 text-sm">
              <div className="rounded-md border bg-background/60 px-3 py-2">
                Review shadow results for gov-2025-01.
              </div>
              <div className="rounded-md border bg-background/60 px-3 py-2">
                Schedule approval window for gov-2025-02.
              </div>
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
