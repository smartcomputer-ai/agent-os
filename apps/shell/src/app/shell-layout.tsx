import { NavLink, Outlet } from "react-router-dom";
import type { LucideIcon } from "lucide-react";
import {
  Compass,
  FolderTree,
  MoreVertical,
  Search,
  ShieldCheck,
  Sparkles,
} from "lucide-react";
import { useWorld } from "./world-provider";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Separator } from "@/components/ui/separator";
import { cn } from "@/lib/utils";

const NAV_ITEMS = [
  {
    to: "/explorer",
    label: "Explorer",
    description: "Manifest, defs, plans",
    icon: Compass,
  },
  {
    to: "/workspaces",
    label: "Workspaces",
    description: "Trees, files, sync",
    icon: FolderTree,
  },
  {
    to: "/governance",
    label: "Governance",
    description: "Proposals, approvals",
    icon: ShieldCheck,
  },
];

export function ShellLayout() {
  const { world, status } = useWorld();
  const manifestLabel = world.manifestHash
    ? `manifest ${world.manifestHash.slice(0, 8)}`
    : null;
  const headLabel =
    world.journalHead != null ? `head ${world.journalHead}` : null;

  return (
    <div className="min-h-screen bg-[radial-gradient(75%_60%_at_10%_10%,#f9ebd7_0%,transparent_60%),radial-gradient(60%_60%_at_90%_0%,#dbe7f5_0%,transparent_55%),linear-gradient(180deg,#fbf7f1_0%,#f2ebe0_100%)]">
      <div className="flex min-h-screen flex-col lg:flex-row">
        <aside className="relative flex w-full flex-col border-b border-border/60 bg-background/80 backdrop-blur-xl lg:w-64 lg:border-b-0 lg:border-r">
          <div className="p-4">
            <div className="flex items-center gap-3">
              <div className="flex size-10 items-center justify-center rounded-xl bg-primary text-primary-foreground shadow-sm">
                <Sparkles className="size-5" />
              </div>
              <div>
                <div className="text-sm font-semibold tracking-tight">
                  AgentOS
                </div>
                <div className="text-xs text-muted-foreground">
                  Deterministic control plane
                </div>
              </div>
            </div>
            <div className="mt-4 flex items-center gap-2">
              <Badge variant={status.connected ? "secondary" : "destructive"}>
                {status.connected ? "Connected" : "Disconnected"}
              </Badge>
              <span className="text-xs text-muted-foreground">
                {status.label}
              </span>
            </div>
          </div>
          <Separator />
          <ScrollArea className="flex-1 px-3 py-4">
            <div className="space-y-5">
              <div>
                <p className="px-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
                  Core
                </p>
                <nav className="mt-2 space-y-1">
                  {NAV_ITEMS.map((item) => (
                    <NavItem key={item.to} {...item} />
                  ))}
                </nav>
              </div>
              <div className="rounded-xl border border-dashed border-border/80 bg-card/60 p-3 text-xs text-muted-foreground">
                <div className="text-[11px] uppercase tracking-wide">
                  System notes
                </div>
                <p className="mt-2">
                  Plans are orchestrated here, reducers stay deterministic, and
                  effects live at the edge.
                </p>
              </div>
            </div>
          </ScrollArea>
          <Separator />
          <div className="p-4">
            <div className="rounded-xl border bg-card/80 p-3 text-xs text-muted-foreground">
              <div className="text-[11px] uppercase tracking-wide">World</div>
              <div className="mt-2 text-sm font-semibold text-foreground">
                {world.name ?? "World"}
              </div>
              <div className="text-xs">
                version {world.version ?? "not set"}
              </div>
              <div className="mt-3 flex flex-wrap gap-2">
                {manifestLabel ? (
                  <Badge variant="outline" className="text-[10px]">
                    {manifestLabel}
                  </Badge>
                ) : null}
                {headLabel ? (
                  <Badge variant="outline" className="text-[10px]">
                    {headLabel}
                  </Badge>
                ) : null}
              </div>
            </div>
          </div>
        </aside>

        <div className="flex min-h-screen flex-1 flex-col">
          <header className="sticky top-0 z-10 flex h-14 items-center justify-between border-b border-border/60 bg-background/75 px-4 backdrop-blur-xl sm:px-6">
            <div className="flex items-center gap-4">
              <div>
                <div className="text-sm font-semibold">
                  {world.name ?? "World"}
                </div>
                <div className="text-xs text-muted-foreground">
                  Runtime overview and controls
                </div>
              </div>
              <div className="hidden items-center gap-2 md:flex">
                {manifestLabel ? (
                  <Badge variant="outline" className="text-xs">
                    {manifestLabel}
                  </Badge>
                ) : null}
                {headLabel ? (
                  <Badge variant="outline" className="text-xs">
                    {headLabel}
                  </Badge>
                ) : null}
              </div>
            </div>
            <div className="flex items-center gap-2">
              <Button
                variant="outline"
                size="sm"
                className="hidden gap-2 text-muted-foreground sm:flex"
              >
                <Search className="size-4" />
                Search
                <span className="rounded border px-1.5 py-0.5 text-[10px] text-muted-foreground">
                  Cmd+K
                </span>
              </Button>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <Button
                    variant="ghost"
                    size="icon"
                    aria-label="Open quick actions"
                  >
                    <MoreVertical className="size-4" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuLabel>Quick actions</DropdownMenuLabel>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem>Open workspace</DropdownMenuItem>
                  <DropdownMenuItem>New proposal</DropdownMenuItem>
                  <DropdownMenuItem>Sync manifests</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </header>
          <main className="flex-1 px-4 py-6 sm:px-6">
            <Outlet />
          </main>
        </div>
      </div>
    </div>
  );
}

type NavItemProps = {
  to: string;
  label: string;
  description: string;
  icon: LucideIcon;
};

function NavItem({ to, label, description, icon: Icon }: NavItemProps) {
  return (
    <NavLink to={to} className="block">
      {({ isActive }) => (
        <span
          className={cn(
            "group flex items-start gap-3 rounded-xl px-3 py-2 text-sm transition hover:bg-accent/60",
            isActive && "bg-accent text-foreground shadow-sm"
          )}
        >
          <span
            className={cn(
              "mt-0.5 flex size-8 items-center justify-center rounded-lg border bg-background text-muted-foreground transition group-hover:text-foreground",
              isActive && "border-primary/40 bg-primary text-primary-foreground"
            )}
          >
            <Icon className="size-4" />
          </span>
          <span>
            <span className="block font-medium">{label}</span>
            <span className="block text-xs text-muted-foreground">
              {description}
            </span>
          </span>
        </span>
      )}
    </NavLink>
  );
}
