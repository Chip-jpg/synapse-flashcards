import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Frontend tests: React screens in a simulated browser (jsdom), with Tauri
// replaced by `@tauri-apps/api/mocks`. Run with `npm test`.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
});
