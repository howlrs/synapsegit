import AxeBuilder from "@axe-core/playwright";
import { test, expect } from "./fixtures.mjs";

test("project dashboard keeps creation first and filters readable session summaries", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete`);
  await expect(page.getByRole("heading", { name: "セッション" })).toBeVisible();
  const upload = page.getByRole("heading", { name: /Creator session を開始/ });
  const sessions = page.getByRole("heading", { name: "セッション" });
  const maintenance = page.getByRole("heading", { name: "リポジトリ整合性の確認" });
  expect(await upload.boundingBox()).toBeTruthy();
  expect((await sessions.boundingBox()).y).toBeGreaterThan((await upload.boundingBox()).y);
  expect((await maintenance.boundingBox()).y).toBeGreaterThan((await sessions.boundingBox()).y);
  await page.getByLabel("状態・判断で絞り込む").selectOption("defer");
  await expect(page.locator("[data-creator-session-row]:visible")).toHaveCount(1);
  await expect(page.locator("[data-creator-session-count]")).toHaveText("1 件");
  await page.setViewportSize({ width: 360, height: 800 });
  await expect(page.getByLabel("状態・判断で絞り込む")).toBeVisible();
  const results = await new AxeBuilder({ page }).include("#main").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
  expect(results.violations).toEqual([]);
  await page.screenshot({ path: "docs/assets/synapse-local/project-dashboard.png", fullPage: true });
});

test("project dashboard remains readable without JavaScript", async ({ browser, app }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto(`${app.origin}/projects/complete`);
  await expect(page.getByRole("heading", { name: "セッション" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "メンテナンス" })).toBeVisible();
  await context.close();
});
