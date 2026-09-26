import AxeBuilder from "@axe-core/playwright";
import { isolatedTest as test, expect, original, current, output } from "./fixtures.mjs";

async function importProposal(page, app, project, session) {
  await page.goto(`${app.origin}/projects/${project}`);
  await page.locator('[name="session"]').fill(session);
  await page.locator('[name="creator_name"]').fill("再利用テスト");
  await page.locator('[name="subject_label"]').fill("再利用する作品");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) {
    await page.locator(`[name="${name}"]`).setInputFiles(file);
  }
  await page.getByRole("button", { name: "Proposalを作成", exact: true }).click();
  await page.waitForURL(`**/creator-sessions/${session}`);
}

async function decide(page, name) {
  page.once("dialog", dialog => dialog.accept());
  const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
  await page.getByRole("button", { name, exact: true }).click();
  await navigation;
  await page.waitForLoadState("domcontentloaded");
}

test("import, restart, then reuse the interrupted proposal and record a new decision", async ({ page, app }) => {
  await importProposal(page, app, "interrupted", "restart-source");
  const sourceOids = await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid));
  await app.restart();
  await page.goto(`${app.origin}/projects/interrupted/creator-sessions/restart-source`);
  await expect(page.getByRole("heading", { name: "セッションは未完了です", exact: true })).toBeVisible();
  await expect(page.getByText("新しいセッションでレビューできます", { exact: false })).toBeVisible();
  for (const image of await page.locator("img[data-synapse-image]").all()) {
    await expect.poll(() => image.evaluate(node => node.complete && node.naturalWidth > 0)).toBe(true);
  }
  await page.getByRole("link", { name: "この提案を新しいセッションでレビューする", exact: true }).click();
  await expect(page.getByRole("heading", { name: "記録済みの提案を新しいセッションでレビュー", exact: true })).toBeVisible();
  await page.locator('[name="session"]').fill("restart-reused");
  await page.getByRole("button", { name: "この提案を新しいセッションでレビューする", exact: true }).focus();
  await page.keyboard.press("Enter");
  await page.waitForURL("**/creator-sessions/restart-reused");
  expect(await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid))).toEqual(sourceOids);
  await expect(page.locator("[data-creator-reuse-source]")).toContainText("restart-source");
  await decide(page, "Adopt");
  await expect(page.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();
});

test("Defer re-review reuses all images, shows the old reason only as reference, and remains accessible on narrow screens", async ({ page, app }) => {
  await importProposal(page, app, "reviews", "deferred-source");
  const sourceOids = await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid));
  await page.getByLabel("Rationale（任意）", { exact: true }).fill("クライアント確認後に決める");
  await decide(page, "Defer");
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole("link", { name: "保留した提案を改めて判断する", exact: true }).click();
  await expect(page.getByText("元のDefer理由（参照のみ）: クライアント確認後に決める", { exact: true })).toBeVisible();
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.locator('[name="session"]').fill("deferred-rereview");
  await page.getByRole("button", { name: "この提案を新しいセッションでレビューする", exact: true }).click();
  await page.waitForURL("**/creator-sessions/deferred-rereview");
  expect(await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid))).toEqual(sourceOids);
  await expect(page.locator("[data-creator-reuse-source]")).toContainText("deferred-source");
  await expect(page.locator("body")).not.toContainText("クライアント確認後に決める");
  await decide(page, "Adopt");
  await expect(page.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();
});
