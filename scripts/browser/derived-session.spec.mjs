import AxeBuilder from "@axe-core/playwright";
import { test, expect, original, current, output } from "./fixtures.mjs";

for (const disposition of ["Adopt", "Reject", "Defer"]) {
  test(`derive from ${disposition} with one fresh candidate and unchanged reference bytes`, async ({ page, app }) => {
    const source = `source-${disposition.toLowerCase()}`;
    await page.goto(`${app.origin}/projects/reviews`);
    await page.locator('[name="session"]').fill(source);
    await page.locator('[name="creator_name"]').fill("元の表示名");
    await page.locator('[name="subject_label"]').fill("元の作品名");
    for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) await page.locator(`[name="${name}"]`).setInputFiles(file);
    await page.getByText("提案の生成メモ（任意）", { exact: true }).click();
    await page.getByLabel("プロンプト", { exact: true }).fill("OLD_SOURCE_PRIVATE_NOTE");
    await page.getByRole("button", { name: "Proposalを作成", exact: true }).click();
    await page.waitForURL(`**/creator-sessions/${source}`);
    const oids = await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid));
    await page.getByLabel("Rationale（任意）", { exact: true }).fill("OLD_SOURCE_RATIONALE");
    page.once("dialog", dialog => dialog.accept());
    const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
    await page.getByRole("button", { name: disposition, exact: true }).click();
    await navigation;
    await page.getByRole("link", { name: "この記録から次の案を試す", exact: true }).click();
    await expect(page.getByRole("heading", { name: "派生元と再利用する参照画像", exact: true })).toBeVisible();
    await expect(page.getByText("Adopt済みでも元のAI outputをCurrentへ昇格しません。", { exact: false })).toBeVisible();
    for (const image of await page.locator("img[data-synapse-image]").all()) {
      await expect(image).toHaveAttribute("src", /^blob:/u);
      await expect.poll(() => image.evaluate(node => node.complete && node.naturalWidth > 0)).toBe(true);
    }
    await expect(page.locator('input[type="file"]')).toHaveCount(1);
    await expect(page.locator('[name="ai_output"]')).toHaveValue("");
    await expect(page.locator('[name="creator_name"]')).toHaveValue("元の表示名");
    await expect(page.locator('[name="subject_label"]')).toHaveValue("元の作品名");
    await expect(page.locator('[name="generation_prompt"]')).toHaveValue("");
    await expect(page.locator("body")).not.toContainText("OLD_SOURCE_PRIVATE_NOTE");
    await expect(page.locator("body")).not.toContainText("OLD_SOURCE_RATIONALE");
    const next = `derived-${disposition.toLowerCase()}`;
    await page.locator('[name="session"]').fill(next);
    await page.locator('[name="creator_name"]').fill("新しい表示名");
    await page.locator('[name="subject_label"]').fill("新しい作品名");
    await page.locator('[name="ai_output"]').setInputFiles(original);
    if (disposition === "Adopt") {
      await page.setViewportSize({ width: 375, height: 900 });
      expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
    }
    const button = page.getByRole("button", { name: "参照画像を引き継いでProposalを作成", exact: true });
    await button.focus();
    await page.keyboard.press("Enter");
    await page.waitForURL(`**/creator-sessions/${next}`);
    const nextOids = await page.locator("img[data-synapse-image]").evaluateAll(images => images.map(image => image.dataset.oid));
    expect(nextOids.slice(0, 2)).toEqual(oids.slice(0, 2));
    expect(nextOids[1]).not.toBe(oids[2]);
    await expect(page.locator("[data-creator-source]")).toContainText(source);
    await expect(page.getByText("生成メモなし", { exact: true })).toBeVisible();
    page.once("dialog", dialog => dialog.accept());
    const completed = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
    await page.getByRole("button", { name: disposition, exact: true }).click();
    await completed;
    await page.reload();
    await expect(page.locator("[data-creator-source]")).toContainText(source);
  });
}
