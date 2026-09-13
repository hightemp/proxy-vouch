import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests-ui",
  timeout: 30_000,
  use: {
    baseURL: "http://127.0.0.1:1431",
    viewport: { width: 1320, height: 900 },
    launchOptions: {
      executablePath: process.env.CHROME_PATH || undefined,
    },
    screenshot: "only-on-failure",
  },
  webServer: {
    command: "pnpm dev --port 1431",
    url: "http://127.0.0.1:1431",
    reuseExistingServer: false,
  },
});
