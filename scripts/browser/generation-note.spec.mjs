import AxeBuilder from "@axe-core/playwright";
import { isolatedTest as test, expect, decisionButton, original, current, output, waitForCreatorUploadReady } from "./fixtures.mjs";

const escapeRegExp = (text) => text.replace(/[.*+?^${}()|[\]\\]/gu, "\\$&");

async function fillImport(page, session) {
  await page.locator('[name="session"]').fill(session);
  await page.locator('[name="creator_name"]').fill("Creator");
  await page.locator('[name="subject_label"]').fill("作品");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) await page.locator(`[name="${name}"]`).setInputFiles(file);
}

for (const disposition of ["Adopt", "Reject", "Defer"]) {
  test(`generation note survives ${disposition} and reload`, async ({ page, app }) => {
    await page.goto(`${app.origin}/projects/reviews/import`);
    await page.getByText("プロンプトと制作意図（任意）", { exact: true }).click();
    if (disposition === "Adopt") {
      expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
    }
    const session = `note-${disposition.toLowerCase()}`;
    await fillImport(page, session);
    await page.getByLabel("使用ツール（任意）", { exact: true }).fill("外部ツール");
    await page.getByLabel("生成モデル（任意）", { exact: true }).fill("申告モデル");
    const prompt = "PRIVATE_GENERATION_CANARY 日本語\n<script>window.noteInjected = true</script>";
    await page.getByLabel("プロンプト", { exact: true }).fill(prompt);
    await page.getByLabel("制作意図", { exact: true }).fill("配色の検討\n判断理由とは別");
    await waitForCreatorUploadReady(page);
    await page.getByRole("button", { name: "提案を作成" }).click();
    await page.waitForURL(`**/creator-sessions/${session}`);
    await expect(page.locator("[data-generation-note]")).toContainText(prompt);
    await expect(page.getByText("モデルが動いたことや、作者が誰かの証明ではありません。", { exact: false })).toBeVisible();
    page.once("dialog", dialog => dialog.accept());
    const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
    await page.getByRole("button", { name: decisionButton[disposition], exact: true }).click();
    await navigation;
    // The navigation event precedes document readiness; finish the app reload
    // before requesting a second reload to verify persisted history.
    await page.waitForLoadState("domcontentloaded");
    await expect(page.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();
    await page.reload();
    await expect(page.locator("[data-generation-note]")).toContainText(prompt);
    await expect(page.locator("[data-generation-note] script")).toHaveCount(0);
    expect(await page.evaluate(() => window.noteInjected)).toBeUndefined();
  });
}

for (const { lang, tool, model, toolHint, modelHint, unknownHint, details, prompt } of [
  {
    lang: "ja",
    tool: "使用ツール（任意）",
    model: "生成モデル（任意）",
    toolHint: "AI outputを作るのに使った道具・製品・サービスの名前です。",
    modelHint: "操作しているAIエージェントのモデルや製品名とは異なる場合があります。",
    unknownHint: "分からない項目は推測で埋めず、空欄のままで構いません。",
    details: "プロンプトと制作意図（任意）",
    prompt: "プロンプト",
  },
  {
    lang: "en",
    tool: "Tool used (optional)",
    model: "Generation model (optional)",
    toolHint: "The tool, product, or service you used to make the AI output.",
    modelHint: "It may differ from the model of the AI agent you work with, or from the product name.",
    unknownHint: "Leave anything you do not know blank instead of guessing.",
    details: "Prompt and intent (optional)",
    prompt: "Prompt",
  },
]) {
  test(`${lang}: tool and model are shown together outside the collapsed prompt and intent`, async ({ page, app }) => {
    await page.setViewportSize({ width: 375, height: 900 });
    await page.goto(`${app.origin}/projects/reviews/import?lang=${lang}`);
    const toolField = page.getByLabel(tool, { exact: true });
    const modelField = page.getByLabel(model, { exact: true });
    await expect(toolField).toBeVisible();
    await expect(modelField).toBeVisible();
    await expect(toolField).toHaveAccessibleDescription(new RegExp(escapeRegExp(toolHint), "u"));
    await expect(modelField).toHaveAccessibleDescription(new RegExp(`${escapeRegExp(modelHint)}.*${escapeRegExp(unknownHint)}`, "su"));
    await expect(page.getByLabel(prompt, { exact: true })).toBeHidden();
    for (const field of [toolField, modelField]) await expect(field).not.toHaveAttribute("required");
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);

    // Keyboard order: tool, model, then the collapsed prompt and intent.
    await toolField.focus();
    await page.keyboard.press("Tab");
    await expect(modelField).toBeFocused();
    await page.keyboard.press("Tab");
    const summary = page.getByText(details, { exact: true });
    await expect(summary).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page.getByLabel(prompt, { exact: true })).toBeVisible();

    // The existing byte counters and over-limit feedback are unchanged.
    await modelField.fill("あ".repeat(101));
    await expect(page.locator('[data-creator-text-count="generation_model"]')).toContainText("303 / 300 bytes");
    await expect(modelField).toHaveAttribute("aria-invalid", "true");
    await modelField.fill("model-a");
    await expect(page.locator('[data-creator-text-count="generation_model"]')).toHaveText("7 / 300 bytes");
    await expect(modelField).toHaveAttribute("aria-invalid", "false");
  });
}

for (const { name, tool, model, shown } of [
  { name: "blank", tool: "", model: "", shown: null },
  { name: "tool-only", tool: "画像生成アプリ", model: "", shown: "使用ツール:\n画像生成アプリ" },
  { name: "model-only", tool: "", model: "model-a", shown: "生成モデル:\nmodel-a" },
]) {
  test(`a ${name} tool and model note imports and shows only what was entered`, async ({ page, app }) => {
    await page.goto(`${app.origin}/projects/reviews/import`);
    const session = `source-${name}`;
    await fillImport(page, session);
    await page.getByLabel("使用ツール（任意）", { exact: true }).fill(tool);
    await page.getByLabel("生成モデル（任意）", { exact: true }).fill(model);
    await waitForCreatorUploadReady(page);
    await page.getByRole("button", { name: "提案を作成", exact: true }).click();
    await page.waitForURL(`**/creator-sessions/${session}`);
    await expect(page.getByRole("heading", { name: "提案の生成メモ", exact: true })).toBeVisible();
    if (shown === null) {
      await expect(page.locator("[data-generation-note]")).toHaveCount(0);
      await expect(page.getByText("生成メモなし", { exact: true })).toBeVisible();
    } else {
      expect(await page.locator("[data-generation-note]").evaluate(node => node.textContent)).toBe(shown);
    }
  });
}
