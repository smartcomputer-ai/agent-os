import { cn } from "@/lib/utils";

interface JsonViewerProps {
  data: unknown;
  className?: string;
}

export function JsonViewer({ data, className }: JsonViewerProps) {
  const json = JSON.stringify(data, null, 2);

  return (
    <pre
      className={cn(
        "rounded-lg border bg-muted/40 p-4 text-sm font-mono overflow-x-auto",
        className
      )}
    >
      <code className="text-foreground/90">{json}</code>
    </pre>
  );
}

interface JsonInlineProps {
  data: unknown;
  className?: string;
  maxLength?: number;
}

/** Compact inline JSON display for tables */
export function JsonInline({ data, className, maxLength = 60 }: JsonInlineProps) {
  const json = JSON.stringify(data);
  const truncated = json.length > maxLength ? json.slice(0, maxLength) + "..." : json;

  return (
    <code
      className={cn(
        "text-xs font-mono text-muted-foreground bg-muted/50 px-1.5 py-0.5 rounded",
        className
      )}
    >
      {truncated}
    </code>
  );
}
