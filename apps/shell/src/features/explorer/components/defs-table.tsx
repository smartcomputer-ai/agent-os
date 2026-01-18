import { Link } from "react-router-dom";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { KindBadge } from "./kind-badge";
import { toDisplayKind } from "../types";

export interface DefListing {
  kind: string;
  name: string;
  cap_type?: string | null;
  params_schema?: string | null;
  plan_steps?: number | null;
  policy_rules?: number | null;
  receipt_schema?: string | null;
}

interface DefsTableProps {
  defs: DefListing[];
  showKind?: boolean;
}

export function DefsTable({ defs, showKind = true }: DefsTableProps) {
  if (defs.length === 0) {
    return (
      <div className="flex items-center justify-center py-12 text-muted-foreground">
        No definitions found
      </div>
    );
  }

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          {showKind && <TableHead className="w-24">Kind</TableHead>}
          <TableHead className="text-right">Details</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {defs.map((def) => (
          <TableRow key={`${def.kind}:${def.name}`} className="group">
            <TableCell>
              <Link
                to={getDefLink(def)}
                className="font-mono text-sm text-foreground hover:text-primary hover:underline underline-offset-4"
              >
                {def.name}
              </Link>
            </TableCell>
            {showKind && (
              <TableCell>
                <KindBadge kind={toDisplayKind(def.kind)} />
              </TableCell>
            )}
            <TableCell className="text-right text-muted-foreground text-sm">
              {getDefMeta(def)}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function getDefLink(def: DefListing): string {
  // Convert API kind (defplan) to display kind (plan) for URL
  const displayKind = toDisplayKind(def.kind);

  // Plans get their own specialized view
  if (displayKind === "plan") {
    return `/explorer/plans/${encodeURIComponent(def.name)}`;
  }
  return `/explorer/defs/${displayKind}/${encodeURIComponent(def.name)}`;
}

function getDefMeta(def: DefListing): string {
  if (def.plan_steps != null) {
    return `${def.plan_steps} step${def.plan_steps === 1 ? "" : "s"}`;
  }
  if (def.policy_rules != null) {
    return `${def.policy_rules} rule${def.policy_rules === 1 ? "" : "s"}`;
  }
  if (def.cap_type) {
    return def.cap_type;
  }
  if (def.params_schema) {
    return def.params_schema;
  }
  return "";
}

interface DefsTableSkeletonProps {
  rows?: number;
  showKind?: boolean;
}

export function DefsTableSkeleton({ rows = 5, showKind = true }: DefsTableSkeletonProps) {
  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          {showKind && <TableHead className="w-24">Kind</TableHead>}
          <TableHead className="text-right">Details</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {Array.from({ length: rows }).map((_, i) => (
          <TableRow key={i}>
            <TableCell>
              <div className="h-4 w-48 bg-muted animate-pulse rounded" />
            </TableCell>
            {showKind && (
              <TableCell>
                <div className="h-5 w-16 bg-muted animate-pulse rounded-full" />
              </TableCell>
            )}
            <TableCell className="text-right">
              <div className="h-4 w-20 bg-muted animate-pulse rounded ml-auto" />
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
