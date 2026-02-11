import type { RouteObject } from "react-router-dom";
import { UnifiedChatPage } from "./pages/unified-chat";

export const demiurgeRoutes: RouteObject[] = [
  { path: "/chat", element: <UnifiedChatPage /> },
];
