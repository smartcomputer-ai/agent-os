import { useState, useCallback } from "react";
import { Check, Copy } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

interface CopyableHashProps {
  hash: string;
  /** Number of characters to show before truncating. Defaults to 16. Use 0 for full hash. */
  truncate?: number;
  className?: string;
}

export function CopyableHash({ hash, truncate = 16, className }: CopyableHashProps) {
  const [copied, setCopied] = useState(false);

  const handleCopy = useCallback(async () => {
    await navigator.clipboard.writeText(hash);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  }, [hash]);

  const displayHash = truncate > 0 && hash.length > truncate
    ? `${hash.slice(0, truncate)}...`
    : hash;

  return (
    <TooltipProvider>
      <Tooltip>
        <TooltipTrigger asChild>
          <button
            type="button"
            onClick={handleCopy}
            className={cn(
              "inline-flex items-center gap-1.5 font-mono text-xs bg-muted px-1.5 py-0.5 rounded cursor-pointer hover:bg-muted/80 transition-colors",
              className
            )}
          >
            <span>{displayHash}</span>
            {copied ? (
              <Check className="w-3 h-3 text-green-500" />
            ) : (
              <Copy className="w-3 h-3 text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity" />
            )}
          </button>
        </TooltipTrigger>
        <TooltipContent>
          <p>{copied ? "Copied!" : "Click to copy full hash"}</p>
        </TooltipContent>
      </Tooltip>
    </TooltipProvider>
  );
}
