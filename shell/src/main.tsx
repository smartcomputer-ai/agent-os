import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "react-router-dom";
import { router } from "./app/router";
import { WorldProvider } from "./app/world-provider";
import { queryClient } from "./sdk/queryClient";
import "./index.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <WorldProvider>
        <RouterProvider router={router} />
      </WorldProvider>
    </QueryClientProvider>
  </StrictMode>,
);
