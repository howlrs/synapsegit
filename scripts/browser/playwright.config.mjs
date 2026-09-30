import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  fullyParallel: false,
  workers: 1,
  timeout: 60_000,
  expect: { timeout: 10_000 },
  // CI retries a failed test once so that a single runner stall does not fail
  // an otherwise green run. A test that passes only on retry is still reported
  // as flaky, in the list output and as a GitHub annotation. Local runs keep
  // zero retries so a failure reproduces directly.
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["list"], ["github"]] : "list",
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
