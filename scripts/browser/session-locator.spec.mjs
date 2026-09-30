import AxeBuilder from "@axe-core/playwright";
import { test, expect } from "./fixtures.mjs";

test("the Japanese session-name locator opens an exact known session without changing the active filter", async ({ page, app }) => {
  await page.setViewportSize({ width: 360, height: 800 });
  await page.goto(`${app.origin}/projects/complete`);
  const filter = page.getByLabel("状態・判断で絞り込む");
  await filter.selectOption("defer");
  const locator = page.locator("[data-creator-session-locator]");
  await expect(locator).toBeVisible();
  await expect(locator.getByText("一覧は最大200件です。古いセッションも、完全に一致する名前で開けます。", { exact: true })).toBeVisible();

  const input = locator.getByLabel("セッション名", { exact: true });
  await input.fill("Sample");
  await input.press("Enter");
  await expect(page).toHaveURL(`${app.origin}/projects/complete`);
  await expect(filter).toHaveValue("defer");
  expect(await input.evaluate(node => node.validity.valid)).toBe(false);
  const results = await new AxeBuilder({ page }).include("[data-creator-session-locator]").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
  expect(results.violations).toEqual([]);

  // Removing the real row makes this browser test cover the locator without
  // visible summaries. It does not establish the server's 200-summary cap.
  await page.locator("[data-creator-session-row]", { hasText: "sample" }).evaluate((row) => row.remove());
  await expect(page.locator("[data-creator-session-row]")).toHaveCount(0);
  await input.fill("sample");
  await input.press("Enter");
  await page.waitForURL("**/projects/complete/creator-sessions/sample");
  await expect(page.getByRole("heading", { name: "sample", exact: true })).toBeVisible();
});

test("the English locator retains exact case and labels", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete?lang=en`);
  const locator = page.locator("[data-creator-session-locator]");
  await expect(locator.getByRole("heading", { name: "Open a session by name", exact: true })).toBeVisible();
  await expect(locator.getByText("The list shows at most 200 sessions. Open an older session by its exact name.", { exact: true })).toBeVisible();
  await expect(locator.getByLabel("Session name", { exact: true })).toHaveAttribute("pattern", "[a-z][a-z0-9\\-]{0,63}");
  await locator.getByLabel("Session name", { exact: true }).fill("sample");
  await locator.getByRole("button", { name: "Open session", exact: true }).click();
  await page.waitForURL("**/projects/complete/creator-sessions/sample");
});

test("the dashboard gives readable session-locator advice without JavaScript", async ({ browser, app }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  const page = await context.newPage();
  await page.goto(`${app.origin}/projects/complete`);
  await expect(page.locator("[data-creator-session-locator]")).toBeHidden();
  await expect(page.getByText("古いセッションを名前で開くにはJavaScriptを有効にしてください。一覧は最大200件です。", { exact: true })).toBeVisible();
  await context.close();
});
