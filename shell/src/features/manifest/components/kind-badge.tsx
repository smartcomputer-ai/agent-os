import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";
import { KIND_STYLES, type DefKind } from "../types";

interface KindBadgeProps {
  kind: DefKind | string;
  className?: string;
}

export function KindBadge({ kind, className }: KindBadgeProps) {
  const styles = KIND_STYLES[kind as DefKind] ?? "bg-muted text-muted-foreground";

  return (
    <Badge
      variant="outline"
      className={cn("font-mono text-[10px] uppercase tracking-wider", styles, className)}
    >
      {kind}
    </Badge>
  );
}
