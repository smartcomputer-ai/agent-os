import type { RouteObject } from "react-router-dom";
import { GovernanceIndexPage } from "./pages/index";
import { GovernanceDraftPage } from "./pages/draft";

export const governanceRoutes: RouteObject[] = [
  { path: "/governance", element: <GovernanceIndexPage /> },
  { path: "/governance/draft", element: <GovernanceDraftPage /> },
];
