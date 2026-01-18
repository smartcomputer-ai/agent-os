import { useLocation, useNavigate } from "react-router-dom";
import {
  ArrowLeft,
  Compass,
  FolderTree,
  Search,
  ShieldCheck,
  Sparkles,
  X,
} from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { useWorld } from "@/app/world-provider";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

type FeatureInfo = {
  path: string;
  label: string;
  icon: LucideIcon;
};

const FEATURES: FeatureInfo[] = [
  { path: "/explorer", label: "Explorer", icon: Compass },
  { path: "/workspaces", label: "Workspaces", icon: FolderTree },
  { path: "/governance", label: "Governance", icon: ShieldCheck },
];

function getActiveFeature(pathname: string): FeatureInfo | null {
  return FEATURES.find((f) => pathname.startsWith(f.path)) ?? null;
}

function isFeatureRoot(pathname: string): boolean {
  return FEATURES.some((f) => f.path === pathname);
}

export function Navbar() {
  const location = useLocation();
  const navigate = useNavigate();
  const { world, status } = useWorld();
  const isHome = location.pathname === "/";
  const activeFeature = getActiveFeature(location.pathname);
  const isAtFeatureRoot = isFeatureRoot(location.pathname);

  const manifestLabel = world.manifestHash
    ? world.manifestHash.slice(0, 8)
    : null;

  const handleBackOrClose = () => {
    if (isAtFeatureRoot) {
      // At feature root, close to home
      navigate("/");
    } else {
      // Deeper in feature, go back
      navigate(-1);
    }
  };

  return (
    <nav className="fixed top-4 left-0 right-0 z-50 mx-4 flex h-14 items-center justify-between rounded-full border border-border/60 bg-background/80 px-2 shadow-sm backdrop-blur-xl sm:mx-6 sm:px-4">
      {/* Left section */}
      <div className="flex min-w-0 shrink-0 items-center gap-2">
        {!isHome && activeFeature ? (
          <>
            <Button
              variant="ghost"
              size="icon"
              className="rounded-full"
              onClick={handleBackOrClose}
              aria-label={isAtFeatureRoot ? "Close app" : "Go back"}
            >
              {isAtFeatureRoot ? (
                <X className="size-4" />
              ) : (
                <ArrowLeft className="size-4" />
              )}
            </Button>
            <div className="flex items-center gap-2">
              <div className="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary">
                <activeFeature.icon className="size-4" />
              </div>
              <span className="font-semibold">{activeFeature.label}</span>
            </div>
          </>
        ) : (
          <div className="flex items-center gap-2 pl-2">
            <div className="flex size-8 items-center justify-center rounded-lg bg-primary text-primary-foreground">
              <Sparkles className="size-4" />
            </div>
            <span className="font-semibold tracking-tight">AgentOS</span>
          </div>
        )}
      </div>

      {/* Center section - search bar placeholder */}
      <div className="mx-4 hidden max-w-md flex-1 md:block">
        <div className="relative">
          <Search className="absolute left-3 top-1/2 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="text"
            placeholder="Search or run command..."
            className="h-9 rounded-full border-border/60 bg-background/60 pl-9 pr-4 text-sm"
            readOnly
          />
          <kbd className="absolute right-3 top-1/2 -translate-y-1/2 rounded border bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground">
            ⌘K
          </kbd>
        </div>
      </div>

      {/* Right section */}
      <div className="flex shrink-0 items-center gap-2 sm:gap-3">
        <span className="hidden text-sm text-muted-foreground lg:inline">
          {world.name ?? "World"}
        </span>
        {manifestLabel && (
          <Badge variant="outline" className="hidden text-xs lg:inline-flex">
            {manifestLabel}
          </Badge>
        )}
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <button
                type="button"
                className="flex items-center justify-center p-1"
                aria-label={status.connected ? "Connected" : "Disconnected"}
              >
                <span
                  className={cn(
                    "size-2.5 rounded-full",
                    status.connected ? "bg-green-500" : "bg-red-500"
                  )}
                />
              </button>
            </TooltipTrigger>
            <TooltipContent side="bottom">
              <p className="text-xs">
                {status.connected ? "Connected" : "Disconnected"} &middot;{" "}
                {status.label}
              </p>
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
      </div>
    </nav>
  );
}
