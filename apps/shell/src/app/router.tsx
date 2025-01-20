import { createBrowserRouter } from "react-router-dom";
import { ShellLayout } from "./shell-layout";
import { HomePage } from "./home";
import { AppErrorBoundary } from "./error-boundary";
import { manifestRoutes } from "../features/manifest/routes";
import { workspacesRoutes } from "../features/workspaces/routes";
import { governanceRoutes } from "../features/governance/routes";

export const router = createBrowserRouter([
  {
    path: "/",
    element: <ShellLayout />,
    errorElement: <AppErrorBoundary />,
    children: [
      { index: true, element: <HomePage /> },
      ...manifestRoutes,
      ...workspacesRoutes,
      ...governanceRoutes,
    ],
  },
]);
