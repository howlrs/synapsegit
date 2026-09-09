import { test, expect, original, current, output } from "./fixtures.mjs";

for (const disposition of ["Adopt", "Reject", "Defer"]) {
  test(`generation note survives ${disposition} and reload`, async ({ page, app }) => {
    await page.goto(`${app.origin}/projects/reviews`);
    const session = `note-${disposition.toLowerCase()}`;
    await page.locator('[name="session"]').fill(session);
    await page.locator('[name="creator_name"]').fill("Creator");
    await page.locator('[name="subject_label"]').fill("作品");
    for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) await page.locator(`[name="${name}"]`).setInputFiles(file);
    await page.getByText("提案の生成メモ（任意）", { exact: true }).click();
    await page.getByLabel("使用ツール", { exact: true }).fill("外部ツール");
    await page.getByLabel("モデル名", { exact: true }).fill("申告モデル");
    const prompt = "PRIVATE_GENERATION_CANARY 日本語\n<script>window.noteInjected = true</script>";
    await page.getByLabel("プロンプト", { exact: true }).fill(prompt);
    await page.getByLabel("制作意図", { exact: true }).fill("配色の検討\n判断理由とは別");
    await page.getByRole("button", { name: "Proposalを作成" }).click();
    await page.waitForURL(`**/creator-sessions/${session}`);
    await expect(page.locator("[data-generation-note]")).toContainText(prompt);
    await expect(page.getByText("モデルの実行・実作者の証明ではありません。", { exact: false })).toBeVisible();
    page.once("dialog", dialog => dialog.accept());
    const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
    await page.getByRole("button", { name: disposition, exact: true }).click();
    await navigation;
    await page.reload();
    await expect(page.locator("[data-generation-note]")).toContainText(prompt);
    await expect(page.locator("[data-generation-note] script")).toHaveCount(0);
    expect(await page.evaluate(() => window.noteInjected)).toBeUndefined();
  });
}
