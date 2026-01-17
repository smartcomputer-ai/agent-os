import type { RouteObject } from "react-router-dom";
import { ExplorerOverview } from "./pages/overview";
import { ManifestPage } from "./pages/manifest";
import { DefsPage } from "./pages/defs";
import { DefDetailPage } from "./pages/def-detail";
import { PlanDiagramPage } from "./pages/plan-diagram";

export const explorerRoutes: RouteObject[] = [
  { path: "/explorer", element: <ExplorerOverview /> },
  { path: "/explorer/manifest", element: <ManifestPage /> },
  { path: "/explorer/defs", element: <DefsPage /> },
  { path: "/explorer/defs/:kind/:name", element: <DefDetailPage /> },
  { path: "/explorer/plans/:name", element: <PlanDiagramPage /> },
];
