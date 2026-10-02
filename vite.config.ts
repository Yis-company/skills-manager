import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  // The Playwright specs in e2e/ run with `pnpm e2e`, not with vitest.
  test: {
    include: ["src/**/*.test.ts"],
  },
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/target/**"],
    },
  },
  preview: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
});
