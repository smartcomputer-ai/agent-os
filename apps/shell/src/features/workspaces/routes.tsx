import type { RouteObject } from "react-router-dom";
import { WorkspacesIndexPage } from "./pages/index";
import { WorkspacePage } from "./pages/workspace";

export const workspacesRoutes: RouteObject[] = [
  { path: "/workspaces", element: <WorkspacesIndexPage /> },
  { path: "/workspaces/:wsId", element: <WorkspacePage /> },
];
