import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "server-and-units",
          environment: "node",
          include: ["tests/**/*.test.ts"],
          setupFiles: ["tests/setup.ts"]
        }
      },
      {
        test: {
          name: "client-components",
          environment: "jsdom",
          include: ["tests/client/**/*.test.tsx"],
          setupFiles: ["tests/setup.ts"]
        }
      }
    ],
    coverage: {
      provider: "v8",
      include: ["src/**/*.{ts,tsx}"],
      exclude: ["src/shared/**"],
      reporter: ["text", "lcov"],
      thresholds: {
        statements: 32,
        branches: 70,
        functions: 43,
        lines: 32
      }
    }
  }
});
