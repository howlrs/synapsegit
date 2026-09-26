import AxeBuilder from "@axe-core/playwright";
import { isolatedTest as test, expect, original, current, output } from "./fixtures.mjs";

async function importProposal(page, app, project, session, generationNote = null) {
  await page.goto(`${app.origin}/projects/${project}`);
  await page.locator('[name="session"]').fill(session);
  await page.locator('[name="creator_name"]').fill("再利用テスト");
  await page.locator('[name="subject_label"]').fill("再利用する作品");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) {
    await page.locator(`[name="${name}"]`).setInputFiles(file);
  }
  if (generationNote) {
    await page.getByText("提案の生成メモ（任意）", { exact: true }).click();
    await page.getByLabel("使用ツール", { exact: true }).fill(generationNote.tool);
    await page.getByLabel("モデル名", { exact: true }).fill(generationNote.model);
    await page.getByLabel("プロンプト", { exact: true }).fill(generationNote.prompt);
    await page.getByLabel("制作意図", { exact: true }).fill(generationNote.intent);
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
  test.setTimeout(120_000);
  await importProposal(page, app, "interrupted", "restart-source");
  const sourceOids = await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid));
  await app.restart();
  await page.goto(`${app.origin}/projects/interrupted/creator-sessions/restart-source`);
  await expect(page.getByRole("heading", { name: "セッションは未完了です", exact: true })).toBeVisible();
  await expect(page.getByText("新しいセッションでレビューできます", { exact: false }).first()).toBeVisible();
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
  test.setTimeout(120_000);
  const generationNote = {
    tool: "制作ツール",
    model: "試作モデル",
    prompt: "夕方の光を残す",
    intent: "輪郭を比較する",
  };
  const pinNote = "輪郭を再確認";
  await importProposal(page, app, "reviews", "deferred-source", generationNote);
  await expect(page.locator("[data-generation-note]")).toContainText(generationNote.prompt);
  const sourceOids = await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid));
  await page.getByLabel("ピンの対象画像", { exact: true }).selectOption("ai_output");
  await page.getByRole("button", { name: "中央にピンを追加", exact: true }).click();
  await page.getByLabel("ピン 1 のメモ", { exact: true }).fill(pinNote);
  await page.getByLabel("Rationale（任意）", { exact: true }).fill("クライアント確認後に決める");
  await decide(page, "Defer");
  await expect(page.locator("[data-generation-note]")).toContainText(generationNote.prompt);
  await expect(page.locator("[data-pin-list]")).toContainText(pinNote);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.getByRole("link", { name: "保留した提案を改めて判断する", exact: true }).click();
  await expect(page.getByText("元のDefer理由（参照のみ）: クライアント確認後に決める", { exact: true })).toBeVisible();
  for (const value of Object.values(generationNote)) await expect(page.getByText(value, { exact: true })).toBeVisible();
  await expect(page.getByRole("heading", { name: "元の画像上の判断メモ（参照のみ）", exact: true }).locator("..")).toContainText(pinNote);
  await expect(page.getByText("ai_output · (500000, 500000) · 輪郭を再確認", { exact: true })).toBeVisible();
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.locator('[name="session"]').fill("deferred-rereview");
  await page.getByRole("button", { name: "この提案を新しいセッションでレビューする", exact: true }).click();
  await page.waitForURL("**/creator-sessions/deferred-rereview");
  expect(await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid))).toEqual(sourceOids);
  await expect(page.locator("[data-creator-reuse-source]")).toContainText("deferred-source");
  await expect(page.locator("[data-creator-reuse-reference]")).toContainText("クライアント確認後に決める");
  for (const value of Object.values(generationNote)) await expect(page.locator("[data-creator-reuse-reference]")).toContainText(value);
  await expect(page.locator("[data-creator-reuse-reference]")).toContainText(pinNote);
  await expect(page.locator("[data-creator-reuse-reference]")).toContainText("ai_output · (500000, 500000)");
  await expect(page.getByText("生成メモなし", { exact: true })).toBeVisible();
  await expect(page.locator("[data-creator-pins]")).toHaveAttribute("data-annotations", "null");
  await expect(page.getByLabel("Rationale（任意）", { exact: true })).toHaveValue("");
  await decide(page, "Adopt");
  await expect(page.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();
  await page.goto(`${app.origin}/projects/reviews/creator-sessions/deferred-source`);
  const sourceMain = page.locator("main");
  await expect(sourceMain.getByRole("link", { name: "deferred-rereview", exact: true })).toBeVisible();
  await sourceMain.getByRole("link", { name: "deferred-rereview", exact: true }).click();
  await page.waitForURL("**/creator-sessions/deferred-rereview");
  await expect(page.getByRole("heading", { name: "deferred-rereview", exact: true })).toBeVisible();
});
