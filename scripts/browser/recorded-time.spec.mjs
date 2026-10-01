import { test, expect } from "./fixtures.mjs";

test.use({ timezoneId: "Asia/Tokyo" });

test("recorded times move to the browser time zone and keep the exact UTC value", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete/creator-sessions/sample`);
  const times = page.locator("time[data-local-time]");
  await expect(times.first()).toBeVisible();
  const count = await times.count();
  expect(count).toBeGreaterThan(4);
  for (let index = 0; index < count; index += 1) {
    const time = times.nth(index);
    await expect(time).not.toContainText("UTC");
    await expect(time).toHaveAttribute("title", /Z$/u);
    await expect(time).toHaveAttribute("datetime", /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{3})?Z$/u);
  }
  await expect(page.locator(".timeline strong").last()).toHaveText("人の判断を記録");
});

test.describe("without JavaScript", () => {
  test.use({ javaScriptEnabled: false });
  test("recorded times stay readable as UTC", async ({ page, app }) => {
    await page.goto(`${app.origin}/projects/complete`);
    await expect(page.locator("time[data-local-time]").first()).toContainText(" UTC");
  });
});
