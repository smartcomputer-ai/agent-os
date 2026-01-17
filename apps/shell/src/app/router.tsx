import { createBrowserRouter, Navigate } from "react-router-dom";
import { ShellLayout } from "./shell-layout";
import { AppErrorBoundary } from "./error-boundary";
import { explorerRoutes } from "../features/explorer/routes";
import { workspacesRoutes } from "../features/workspaces/routes";
import { governanceRoutes } from "../features/governance/routes";

export const router = createBrowserRouter([
  { path: "/", element: <Navigate to="/explorer" replace /> },
  {
    path: "/",
    element: <ShellLayout />,
    errorElement: <AppErrorBoundary />,
    children: [...explorerRoutes, ...workspacesRoutes, ...governanceRoutes],
  },
]);
