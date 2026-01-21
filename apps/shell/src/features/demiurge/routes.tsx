import type { RouteObject } from "react-router-dom";
import { ChatsIndexPage } from "./pages/index";
import { ChatPage } from "./pages/chat";

export const demiurgeRoutes: RouteObject[] = [
  { path: "/chat", element: <ChatsIndexPage /> },
  { path: "/chat/:chatId", element: <ChatPage /> },
];
