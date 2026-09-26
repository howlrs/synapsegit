import AxeBuilder from "@axe-core/playwright";
import { test, expect } from "./fixtures.mjs";

test("project dashboard keeps creation first and filters readable session summaries", async ({ page, app }, testInfo) => {
  await page.goto(`${app.origin}/projects/complete`);
  await expect(page.getByRole("heading", { name: "セッション" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "メンテナンス", exact: true })).toBeVisible();
  await expect(page.locator("header.page-header").getByRole("link", { name: "公開用の制作ノートを作る" })).toBeVisible();
  const upload = page.getByRole("heading", { name: /Creator session を開始/ });
  const sessions = page.getByRole("heading", { name: "セッション" });
  const maintenance = page.getByRole("heading", { name: "リポジトリ整合性の確認" });
  expect(await upload.boundingBox()).toBeTruthy();
  expect((await sessions.boundingBox()).y).toBeGreaterThan((await upload.boundingBox()).y);
  expect((await maintenance.boundingBox()).y).toBeGreaterThan((await sessions.boundingBox()).y);
  expect(await page.evaluate(([uploadNode, sessionsNode, maintenanceNode]) =>
    Boolean(uploadNode.compareDocumentPosition(sessionsNode) & Node.DOCUMENT_POSITION_FOLLOWING)
      && Boolean(sessionsNode.compareDocumentPosition(maintenanceNode) & Node.DOCUMENT_POSITION_FOLLOWING),
    [await upload.elementHandle(), await sessions.elementHandle(), await maintenance.elementHandle()]
  )).toBe(true);
  await page.getByLabel("状態・判断で絞り込む").selectOption("defer");
  await expect(page.locator("[data-creator-session-row]:visible")).toHaveCount(1);
  await expect(page.locator("[data-creator-session-count]")).toHaveText("1 件");
  await expect(page.locator("[data-creator-session-row]")).toContainText("Browser tester");
  await page.setViewportSize({ width: 360, height: 800 });
  await expect(page.getByLabel("状態・判断で絞り込む")).toBeVisible();
  const results = await new AxeBuilder({ page }).include("#main").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
  expect(results.violations).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath("project-dashboard.png"), fullPage: true });
});

test("project dashboard remains readable without JavaScript", async ({ browser, app }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto(`${app.origin}/projects/complete`);
  await expect(page.getByRole("heading", { name: "セッション" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "メンテナンス", exact: true })).toBeVisible();
  await expect(page.locator("header.page-header").getByRole("link", { name: "公開用の制作ノートを作る" })).toBeVisible();
  await expect(page.getByText("Comparison browser fixture")).toBeVisible();
  await context.close();
});
