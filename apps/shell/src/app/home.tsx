import { Link } from "react-router-dom";
import type { LucideIcon } from "lucide-react";
import { Compass, FolderTree, ShieldCheck } from "lucide-react";
import { Card } from "@/components/ui/card";
import { cn } from "@/lib/utils";

type FeatureTile = {
  to: string;
  label: string;
  description: string;
  icon: LucideIcon;
};

const FEATURES: FeatureTile[] = [
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

export function HomePage() {
  return (
    <div className="flex h-[calc(100dvh-7.5rem)] flex-col items-center justify-center">
      <div className="w-full max-w-3xl space-y-8 animate-in fade-in-0 slide-in-from-bottom-4 duration-500">
        <header className="text-center">
          <p className="text-sm font-medium uppercase tracking-widest text-muted-foreground">
            Welcome to
          </p>
          <h1 className="mt-2 text-4xl font-semibold tracking-tight text-foreground font-[var(--font-display)] sm:text-5xl">
            AgentOS Shell
          </h1>
          <p className="mt-3 text-muted-foreground">
            Deterministic control plane for AI agents
          </p>
        </header>

        <div className="grid gap-4 sm:grid-cols-3">
          {FEATURES.map((feature) => (
            <FeatureCard key={feature.to} {...feature} />
          ))}
        </div>
      </div>
    </div>
  );
}

function FeatureCard({ to, label, description, icon: Icon }: FeatureTile) {
  return (
    <Link to={to} className="block h-full">
      <Card
        className={cn(
          "group relative h-full cursor-pointer overflow-hidden p-6 transition-all duration-200",
          "hover:shadow-lg hover:scale-[1.02]",
          "bg-card/80 backdrop-blur-sm"
        )}
      >
        <div className="flex flex-col items-center gap-4 text-center">
          <div
            className={cn(
              "flex size-14 items-center justify-center rounded-2xl transition-colors",
              "bg-primary/10 text-primary",
              "group-hover:bg-primary group-hover:text-primary-foreground"
            )}
          >
            <Icon className="size-7" />
          </div>
          <div>
            <h2 className="text-lg font-semibold">{label}</h2>
            <p className="mt-1 text-sm text-muted-foreground">{description}</p>
          </div>
        </div>
      </Card>
    </Link>
  );
}
