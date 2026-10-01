import AxeBuilder from "@axe-core/playwright";
import { test, expect } from "./fixtures.mjs";

for (const [lang, details, limit, result] of [
  ["ja", "技術的な詳細", "ファイルの内容を比べた結果で、見た目の比較ではありません。", "確認の結果"],
  ["en", "Technical details", "This compares file contents; it is not a visual comparison.", "Check result"],
]) {
  test(`${lang}: identifiers stay folded under technical details and limits stay visible`, async ({ page, app }) => {
    await page.goto(`${app.origin}/projects/complete/creator-sessions/sample?lang=${lang}`);
    const folded = page.locator("details.technical-details");
    expect(await folded.count()).toBeGreaterThan(3);
    for (const block of await folded.all()) expect(await block.getAttribute("open")).toBeNull();
    await expect(page.getByText(limit, { exact: false })).toBeVisible();
    await expect(page.getByText(result, { exact: true })).toBeVisible();

    const image = page.locator(".media-card").first();
    const oid = image.locator("details.technical-details code");
    await expect(oid).toBeHidden();
    await image.getByText(details, { exact: true }).click();
    await expect(oid).toBeVisible();
    await expect(oid).toHaveText(/^blob:sg-oid-v1:sha256:[0-9a-f]{64}$/u);
    const adapter = page.locator("#evidence-heading").locator("xpath=ancestor::section").locator("details.technical-details");
    await adapter.getByText(details, { exact: true }).click();
    await expect(adapter.getByText("synapsegit.observation.byte-identity", { exact: false })).toBeVisible();
    expect((await new AxeBuilder({ page }).include("#main").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);

    await page.goto(`${app.origin}/projects/complete?lang=${lang}`);
    const heads = page.locator("[data-creator-session-row] details.technical-details");
    await expect(heads.locator("code").first()).toBeHidden();
    await heads.first().locator("summary").click();
    await expect(heads.locator("code").first()).toContainText("sg-oid-v1");
    expect((await new AxeBuilder({ page }).include("#main").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
  });
}
