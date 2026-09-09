import AxeBuilder from "@axe-core/playwright";
import { test, expect, original, current, output } from "./fixtures.mjs";

test.use({ hasTouch: true });

async function begin(page, app, session) {
  await page.goto(`${app.origin}/projects/reviews`);
  await page.locator('[name="session"]').fill(session);
  await page.locator('[name="creator_name"]').fill("Pin reviewer");
  await page.locator('[name="subject_label"]').fill("位置付きメモ");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) await page.locator(`[name="${name}"]`).setInputFiles(file);
  await page.getByRole("button", { name: "Proposalを作成" }).click();
  await page.waitForURL(`**/creator-sessions/${session}`);
  await expect(page.getByRole("button", { name: "中央にピンを追加" })).toBeEnabled();
}

for (const disposition of ["Adopt", "Reject", "Defer"]) {
  test(`private pins are saved with ${disposition} and retain exact coordinates`, async ({ page, app }) => {
    await begin(page, app, `pins-${disposition.toLowerCase()}`);
    await page.getByLabel("ピンの対象画像", { exact: true }).selectOption("ai_output");
    await page.getByRole("button", { name: "中央にピンを追加" }).click();
    const note = "右上の輪郭\nPRIVATE_PIN_CANARY <script>window.pinInjected = true</script>";
    await page.getByLabel("ピン 1 のメモ", { exact: true }).fill(note);
    const marker = page.locator('[data-pin-markers] [data-pin-index="0"]');
    await marker.focus();
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Shift+ArrowUp");
    await expect(page.getByLabel("ピン 1 X座標", { exact: true })).toHaveValue("510000");
    await expect(page.getByLabel("ピン 1 Y座標", { exact: true })).toHaveValue("499000");
    await page.getByLabel("Rationale（任意）", { exact: true }).fill("Proposal全体を判断");
    const request = page.waitForRequest(req => req.url().endsWith("/decisions") && req.method() === "POST");
    page.once("dialog", dialog => dialog.accept());
    const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
    await page.getByRole("button", { name: disposition, exact: true }).click();
    const payload = (await request).postDataJSON();
    expect(payload.annotations.pins[0]).toMatchObject({ role: "ai_output", x: 510000, y: 499000, note });
    expect(payload.annotations.pins[0].blob_oid).toMatch(/^blob:sg-oid-v1:sha256:/u);
    await navigation;
    await page.reload();
    const stored = JSON.parse(await page.locator("[data-creator-pins]").getAttribute("data-annotations"));
    expect(stored).toEqual(payload.annotations);
    await expect(page.locator("[data-pin-list]")).toContainText(note);
    await expect(page.locator("[data-pin-list] script")).toHaveCount(0);
    expect(await page.evaluate(() => window.pinInjected)).toBeUndefined();
    await expect(page.getByRole("button", { name: "中央にピンを追加" })).toHaveCount(0);
  });
}

test("pins keep normalized placement across zoom and narrow view, with mouse/touch/keyboard editing", async ({ page, app }) => {
  await begin(page, app, "pins-viewport");
  await page.locator("[data-pin-preview]").tap({ position: { x: 50, y: 50 } });
  await page.getByLabel("ピン 1 のメモ", { exact: true }).fill("ここを確認");
  await page.getByLabel("ピン 1 X座標", { exact: true }).fill("250000");
  await page.getByLabel("ピン 1 Y座標", { exact: true }).fill("750000");
  for (const width of [1440, 375]) {
    await page.setViewportSize({ width, height: 1000 });
    for (const zoom of ["fit", "1", "2"]) {
      await page.getByLabel("ピン画像の表示倍率", { exact: true }).selectOption(zoom);
      const position = await page.locator("[data-pin-canvas]").evaluate(canvas => {
        const image = canvas.querySelector("img").getBoundingClientRect();
        const marker = canvas.querySelector(".image-pin").getBoundingClientRect();
        return { x: (marker.x + marker.width / 2 - image.x) / image.width, y: (marker.y + marker.height / 2 - image.y) / image.height };
      });
      expect(position.x).toBeCloseTo(.25, 3);
      expect(position.y).toBeCloseTo(.75, 3);
    }
  }
  await page.getByLabel("ピン画像の表示倍率", { exact: true }).selectOption("fit");
  const marker = page.locator(".image-pin");
  await marker.scrollIntoViewIfNeeded();
  const rect = await marker.boundingBox();
  await page.mouse.move(rect.x + rect.width / 2, rect.y + rect.height / 2);
  await page.mouse.down();
  await page.mouse.move(rect.x + rect.width / 2 + 20, rect.y + rect.height / 2 - 10);
  await page.mouse.up();
  expect(Number(await page.getByLabel("ピン 1 X座標", { exact: true }).inputValue())).toBeGreaterThan(250000);
  const axe = await new AxeBuilder({ page }).analyze();
  expect(axe.violations).toEqual([]);
  await page.getByRole("button", { name: "中央にピンを追加" }).click();
  await page.getByLabel("ピン 2 のメモ", { exact: true }).fill("二番目を残す");
  await page.getByRole("button", { name: "ピン 1 を削除", exact: true }).click();
  await expect(page.getByLabel("ピン 1 のメモ", { exact: true })).toHaveValue("二番目を残す");
  await marker.focus();
  await page.keyboard.press("Delete");
  await expect(page.locator(".image-pin")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "中央にピンを追加" })).toBeFocused();
});

test("pin and JSON limits prevent submission, cancellation and failure preserve draft", async ({ page, app }) => {
  await begin(page, app, "pins-limit");
  await page.getByRole("button", { name: "中央にピンを追加" }).click();
  await page.getByLabel("ピン 1 のメモ", { exact: true }).fill("あ".repeat(67));
  await expect(page.getByRole("button", { name: "Adopt", exact: true })).toBeDisabled();
  await page.getByLabel("ピン 1 のメモ", { exact: true }).fill("a".repeat(200));
  for (let index = 2; index <= 10; index += 1) {
    await page.getByRole("button", { name: "中央にピンを追加" }).click();
    await page.getByLabel(`ピン ${index} のメモ`, { exact: true }).fill("a".repeat(200));
  }
  await expect(page.getByRole("button", { name: "中央にピンを追加" })).toBeDisabled();
  await page.getByLabel("Rationale（任意）", { exact: true }).fill('"'.repeat(3400));
  await expect(page.locator("[data-decision-json-count]")).toContainText("残り -");
  await expect(page.getByRole("button", { name: "Adopt", exact: true })).toBeDisabled();
  await page.getByLabel("Rationale（任意）", { exact: true }).fill("判断理由");
  page.once("dialog", dialog => dialog.dismiss());
  await page.getByRole("button", { name: "Defer", exact: true }).click();
  await expect(page.locator("[data-synapse-status]")).toHaveText("Decisionは送信されませんでした。");
  await page.route("**/creator-sessions/pins-limit/decisions", route => route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ title: "Temporary", detail: "Temporary failure", code: "internal_error" }) }));
  page.once("dialog", dialog => dialog.accept());
  await page.getByRole("button", { name: "Defer", exact: true }).click();
  await expect(page.locator("[data-synapse-status]")).toHaveText("Temporary failure");
  await expect(page.getByLabel("ピン 10 のメモ", { exact: true })).toHaveValue("a".repeat(200));
  await expect(page.getByLabel("ピン 10 のメモ", { exact: true })).toBeEnabled();
  await expect(page.getByRole("button", { name: "中央にピンを追加" })).toBeDisabled();
});

test("attachment and decode failures explain why pins are unavailable", async ({ page, app }) => {
  for (const project of ["mixed", "broken"]) {
    await page.goto(`${app.origin}/projects/${project}/creator-sessions/sample`);
    await page.getByLabel("ピンの対象画像", { exact: true }).selectOption("current");
    await expect(page.locator("[data-pin-status]")).toContainText("attachment扱い");
    await expect(page.locator("[data-pin-preview]")).toBeHidden();
    if (project === "broken") {
      await page.getByLabel("ピンの対象画像", { exact: true }).selectOption("original");
      await expect(page.locator("[data-pin-status]")).toContainText("decode失敗");
    }
  }
});
