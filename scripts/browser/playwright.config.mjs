import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  reporter: "list",
  use: {
    browserName: "chromium",
    headless: true,
    // The established Japanese browser suite must remain deterministic even
    // when the host Chromium default is English. Locale-specific workflows
    // select their language explicitly, just as a person does in the UI.
    locale: "ja-JP",
    viewport: { width: 1440, height: 1000 },
    serviceWorkers: "block",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
});
