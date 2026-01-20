import type { RouteObject } from "react-router-dom";
import { ManifestTreePage } from "./pages/manifest-tree";
import { DefsPage } from "./pages/defs";
import { DefDetailPage } from "./pages/def-detail";
import { PlanDiagramPage } from "./pages/plan-diagram";

export const manifestRoutes: RouteObject[] = [
  { path: "/manifest", element: <ManifestTreePage /> },
  { path: "/manifest/defs", element: <DefsPage /> },
  { path: "/manifest/defs/:kind/:name", element: <DefDetailPage /> },
  { path: "/manifest/plans/:name", element: <PlanDiagramPage /> },
];
