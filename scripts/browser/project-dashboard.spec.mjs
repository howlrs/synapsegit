import AxeBuilder from "@axe-core/playwright";
import { test, expect } from "./fixtures.mjs";

const pages = [
  ["セッション", "", "セッション"],
  ["取り込む", "/import", "3つのファイルから始める"],
  ["管理", "/maintenance", "リポジトリ整合性の確認"],
  ["履歴", "/history", "Refs"],
];

test("the project opens on its sessions and links to import, maintenance, and history", async ({ page, app }, testInfo) => {
  await page.goto(`${app.origin}/projects/complete`);
  const nav = page.getByRole("navigation", { name: "プロジェクト内のページ" });
  await expect(nav.getByRole("link", { name: "セッション", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { name: "セッション", exact: true })).toBeVisible();
  await expect(page.locator("header.page-header").getByRole("link", { name: "公開用の制作ノートを作る" })).toBeVisible();
  // Import, maintenance, and history live on their own pages.
  await expect(page.locator("form[data-creator-upload]")).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "リポジトリ整合性の確認" })).toHaveCount(0);
  await expect(page.getByRole("heading", { name: "Refs", exact: true })).toHaveCount(0);
  await expect(page.getByText("Ref snapshot")).toHaveCount(0);

  const filter = page.getByLabel("状態・判断で絞り込む");
  await filter.selectOption("reject");
  await expect(page.locator("[data-creator-session-row]:visible")).toHaveCount(0);
  await expect(page.locator("[data-creator-session-count]")).toHaveText("0 件");
  await filter.selectOption("complete");
  await expect(page.locator("[data-creator-session-row]:visible")).toHaveCount(1);
  await filter.selectOption("defer");
  await expect(page.locator("[data-creator-session-row]:visible")).toHaveCount(1);
  await expect(page.locator("[data-creator-session-count]")).toHaveText("1 件");
  await expect(page.locator("[data-creator-session-row]")).toContainText("Browser tester");

  for (const [name, suffix, heading] of pages.slice(1)) {
    await nav.getByRole("link", { name, exact: true }).click();
    await expect(page).toHaveURL(`${app.origin}/projects/complete${suffix}`);
    await expect(nav.getByRole("link", { name, exact: true })).toHaveAttribute("aria-current", "page");
    await expect(page.getByRole("heading", { name: heading })).toBeVisible();
    await expect(page.locator("[data-creator-session-list]")).toHaveCount(0);
    const results = await new AxeBuilder({ page }).include("#main").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
    expect(results.violations).toEqual([]);
  }
  await page.goto(`${app.origin}/projects/complete`);
  await page.setViewportSize({ width: 360, height: 800 });
  await expect(page.getByLabel("状態・判断で絞り込む")).toBeVisible();
  for (const [name] of pages) await expect(nav.getByRole("link", { name, exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const results = await new AxeBuilder({ page }).include("#main").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
  expect(results.violations).toEqual([]);
  await page.screenshot({ path: testInfo.outputPath("project-dashboard.png"), fullPage: true });
});

test("project pages remain readable without JavaScript", async ({ browser, app }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  try {
    for (const [, suffix, heading] of pages) {
      await page.goto(`${app.origin}/projects/complete${suffix}`);
      await expect(page.getByRole("heading", { name: heading }).first()).toBeVisible();
    }
    await page.goto(`${app.origin}/projects/complete`);
    await expect(page.locator("header.page-header").getByRole("link", { name: "公開用の制作ノートを作る" })).toBeVisible();
    await expect(page.getByText("Comparison browser fixture")).toBeVisible();
  } finally {
    await context.close();
  }
});
