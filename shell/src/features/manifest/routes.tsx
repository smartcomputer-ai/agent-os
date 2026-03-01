import type { RouteObject } from "react-router-dom";
import { ManifestTreePage } from "./pages/manifest-tree";
import { DefsPage } from "./pages/defs";
import { DefDetailPage } from "./pages/def-detail";

export const manifestRoutes: RouteObject[] = [
  { path: "/manifest", element: <ManifestTreePage /> },
  { path: "/manifest/defs", element: <DefsPage /> },
  { path: "/manifest/defs/:kind/:name", element: <DefDetailPage /> },
];
