import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18nReady } from "./i18n";
import { logStartupEvent } from "./lib/tauri";
import "./index.css";
import App from "./App.tsx";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      networkMode: "always",
      staleTime: 0,
      refetchOnWindowFocus: false,
      refetchOnReconnect: false,
    },
    mutations: {
      retry: false,
      networkMode: "always",
    },
  },
});

// E2E tests only: answer every backend call from a fake. Vite drops this from other builds.
if (import.meta.env.MODE === "e2e") await import("../e2e/fake-backend");

await i18nReady;
logStartupEvent("i18n_ready", performance.now()).catch(() => {});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </StrictMode>
);
logStartupEvent("root_rendered", performance.now()).catch(() => {});
