import { useState, type ReactNode } from "react";
import { ChevronRight, ChevronDown } from "lucide-react";
import { cn } from "@/lib/utils";

interface TreeNodeProps {
  /** Label for the node */
  label: ReactNode;
  /** Optional icon to show before the label */
  icon?: ReactNode;
  /** Optional badge/count to show after the label */
  badge?: ReactNode;
  /** Optional metadata line below the label */
  metadata?: ReactNode;
  /** Child nodes (if expandable) */
  children?: ReactNode;
  /** Whether the node starts expanded */
  defaultExpanded?: boolean;
  /** Indentation level (0 = root) */
  level?: number;
  /** Optional click handler for the label */
  onLabelClick?: () => void;
  /** Additional class names */
  className?: string;
}

export function TreeNode({
  label,
  icon,
  badge,
  metadata,
  children,
  defaultExpanded = false,
  level = 0,
  onLabelClick,
  className,
}: TreeNodeProps) {
  const [expanded, setExpanded] = useState(defaultExpanded);
  const hasChildren = Boolean(children);
  const indent = level * 16;

  return (
    <div className={cn("select-none", className)}>
      <div
        className={cn(
          "flex items-start gap-1 py-1 px-2 rounded-md",
          "hover:bg-muted/50 transition-colors",
          hasChildren && "cursor-pointer"
        )}
        style={{ paddingLeft: `${indent + 8}px` }}
        onClick={() => hasChildren && setExpanded(!expanded)}
      >
        {/* Expand/collapse chevron */}
        <span className="w-4 h-4 flex items-center justify-center mt-0.5 shrink-0">
          {hasChildren ? (
            expanded ? (
              <ChevronDown className="w-3.5 h-3.5 text-muted-foreground" />
            ) : (
              <ChevronRight className="w-3.5 h-3.5 text-muted-foreground" />
            )
          ) : null}
        </span>

        {/* Icon */}
        {icon && (
          <span className="w-4 h-4 flex items-center justify-center mt-0.5 shrink-0">
            {icon}
          </span>
        )}

        {/* Main content */}
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2">
            {/* Label */}
            {onLabelClick ? (
              <button
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  onLabelClick();
                }}
                className="text-sm font-medium text-foreground hover:text-primary hover:underline text-left"
              >
                {label}
              </button>
            ) : (
              <span className="text-sm font-medium text-foreground">{label}</span>
            )}

            {/* Badge */}
            {badge && <span className="shrink-0">{badge}</span>}
          </div>

          {/* Metadata line */}
          {metadata && (
            <div className="text-xs text-muted-foreground mt-0.5">{metadata}</div>
          )}
        </div>
      </div>

      {/* Children */}
      {hasChildren && expanded && <div className="mt-0.5">{children}</div>}
    </div>
  );
}

/** Simple leaf node without expand/collapse */
interface TreeLeafProps {
  label: ReactNode;
  icon?: ReactNode;
  level?: number;
  onClick?: () => void;
  className?: string;
}

export function TreeLeaf({
  label,
  icon,
  level = 0,
  onClick,
  className,
}: TreeLeafProps) {
  const indent = level * 16;

  return (
    <div
      className={cn(
        "flex items-center gap-1 py-1 px-2 rounded-md",
        "hover:bg-muted/50 transition-colors",
        onClick && "cursor-pointer",
        className
      )}
      style={{ paddingLeft: `${indent + 8}px` }}
      onClick={onClick}
    >
      {/* Spacer for alignment with tree nodes */}
      <span className="w-4 h-4 shrink-0" />

      {/* Icon */}
      {icon && (
        <span className="w-4 h-4 flex items-center justify-center shrink-0">
          {icon}
        </span>
      )}

      {/* Label */}
      <span
        className={cn(
          "text-sm",
          onClick
            ? "text-foreground hover:text-primary hover:underline"
            : "text-muted-foreground"
        )}
      >
        {label}
      </span>
    </div>
  );
}

/** Property row for key-value display within a tree */
interface TreePropertyProps {
  name: string;
  value: ReactNode;
  level?: number;
  onValueClick?: () => void;
}

export function TreeProperty({
  name,
  value,
  level = 0,
  onValueClick,
}: TreePropertyProps) {
  const indent = level * 16;

  return (
    <div
      className="flex items-center gap-2 py-0.5 px-2"
      style={{ paddingLeft: `${indent + 28}px` }}
    >
      <span className="text-xs text-muted-foreground">{name}:</span>
      {onValueClick ? (
        <button
          type="button"
          onClick={onValueClick}
          className="text-xs font-mono text-primary hover:underline"
        >
          {value}
        </button>
      ) : (
        <span className="text-xs font-mono text-foreground">{value}</span>
      )}
    </div>
  );
}
