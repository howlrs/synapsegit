import AxeBuilder from "@axe-core/playwright";
import { readFile } from "node:fs/promises";
import { isolatedTest as test, expect, original, current, output } from "./fixtures.mjs";

const api = (page, route) => page.evaluate(async route => {
  const token = document.querySelector('meta[name="synapse-local-token"]').content;
  return (await fetch(route, { headers: { "X-Synapse-Local-Token": token } })).json();
}, route);

test("fresh public text previews and downloads without private source text or Core writes", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/reviews/import`);
  await page.locator('[name="session"]').fill("public-source");
  await page.locator('[name="creator_name"]').fill("PRIVATE_CREATOR_CANARY");
  await page.locator('[name="subject_label"]').fill("PRIVATE_SUBJECT_CANARY");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) await page.locator(`[name="${name}"]`).setInputFiles(file);
  await page.getByText("提案の生成メモ（任意）", { exact: true }).click();
  await page.locator('[name="generation_prompt"]').fill("PRIVATE_PROMPT_CANARY");
  await page.getByRole("button", { name: "提案を作成", exact: true }).click();
  await page.waitForURL("**/creator-sessions/public-source");
  await page.getByLabel("理由（任意）", { exact: true }).fill("PRIVATE_RATIONALE_CANARY");
  page.once("dialog", dialog => dialog.accept());
  const navigation = page.waitForEvent("framenavigated", { predicate: frame => frame === page.mainFrame() });
  await page.getByRole("button", { name: "不採用", exact: true }).click();
  await navigation;
  await page.goto(`${app.origin}/projects/reviews`);
  await page.getByRole("link", { name: "公開用の制作ノートを作る", exact: true }).click();
  const before = await api(page, "/api/v1/projects/reviews/refs");
  const external = [];
  page.on("request", request => { if (!request.url().startsWith(app.origin)) external.push(request.url()); });
  for (const control of await page.locator("[data-public-max-bytes]").all()) await expect(control).toHaveValue("");
  await expect(page.locator("body")).not.toContainText("PRIVATE_PROMPT_CANARY");
  await expect(page.locator("body")).not.toContainText("PRIVATE_RATIONALE_CANARY");
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("public-source");
  await expect(page.locator('[name="creator_display_name"]')).toHaveValue("PRIVATE_CREATOR_CANARY");
  await expect(page.locator('[name="title"]')).toHaveValue("PRIVATE_SUBJECT_CANARY");
  const prose = '日本語の "引用" と \\ パス\n次の行 <script>表示だけ</script>';
  await page.locator('[name="title"]').fill("公開する作品名");
  await page.locator('[name="creator_display_name"]').fill("");
  await page.locator('[name="summary"]').fill(prose);
  await page.locator('[name="public_decision_note"]').fill("公開用に別途書いた判断メモ");
  await page.setViewportSize({ width: 375, height: 900 });
  expect((await new AxeBuilder({ page }).analyze()).violations).toEqual([]);
  await page.getByRole("button", { name: "入力した文章を確認", exact: true }).focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-presentation-preview]")).toBeHidden();
  await page.getByRole("checkbox", { name: "候補を確認し、公開用の文章として使うことを確認しました。", exact: true }).check();
  await page.getByRole("button", { name: "入力した文章を確認", exact: true }).click();
  await expect(page.locator("[data-presentation-preview]")).toBeVisible();
  await expect(page.locator("[data-presentation-text]")).toContainText(prose);
  await expect(page.locator("body")).not.toContainText("PRIVATE_PROMPT_CANARY");
  await expect(page.locator("body")).not.toContainText("PRIVATE_RATIONALE_CANARY");
  const downloaded = page.waitForEvent("download");
  await page.getByRole("button", { name: "説明文ファイルを書き出す", exact: true }).click();
  const download = await downloaded;
  expect(download.suggestedFilename()).toBe("presentation.toml");
  const text = await readFile(await download.path(), "utf8");
  expect(text).toContain("公開する作品名"); expect(text).toContain("[sessions.public-source]");
  expect(text).not.toContain("PRIVATE_PROMPT_CANARY"); expect(text).not.toContain("PRIVATE_RATIONALE_CANARY"); expect(text).not.toContain("blob:sha256:");
  expect(external).toEqual([]);
  expect(await api(page, "/api/v1/projects/reviews/refs")).toEqual(before);
  await page.locator('[name="summary"]').fill("編集後");
  await expect(page.locator("[data-presentation-preview]")).toBeHidden();
});

test("public text validation rejects byte and control limits and keeps omissions", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete/presentation`);
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("sample");
  await expect(page.locator('[name="creator_display_name"]')).toHaveValue("Browser tester");
  await expect(page.locator('[name="title"]')).toHaveValue("Comparison browser fixture");
  await page.locator('[name="creator_display_name"]').fill("");
  await page.locator('[name="title"]').fill("");
  await page.locator('[name="title"]').fill("あ".repeat(101));
  await page.getByRole("checkbox", { name: "候補を確認し、公開用の文章として使うことを確認しました。", exact: true }).check();
  await page.getByRole("button", { name: "入力した文章を確認", exact: true }).click();
  await expect(page.locator("[data-presentation-status]")).toContainText("303 / 300");
  await expect(page.locator("[data-presentation-preview]")).toBeHidden();
  await page.locator('[name="title"]').fill("禁止\u202e文字");
  await page.getByRole("checkbox", { name: "候補を確認し、公開用の文章として使うことを確認しました。", exact: true }).check();
  await page.getByRole("button", { name: "入力した文章を確認", exact: true }).click();
  await expect(page.locator("[data-presentation-status]")).toContainText("forbidden control");
  await expect(page.locator("[data-presentation-preview]")).toBeHidden();
  await page.locator('[name="title"]').fill("");
  await page.getByRole("checkbox", { name: "候補を確認し、公開用の文章として使うことを確認しました。", exact: true }).check();
  await page.getByRole("button", { name: "入力した文章を確認", exact: true }).click();
  await expect(page.locator("[data-presentation-preview]")).toBeVisible();
  const downloaded = page.waitForEvent("download");
  await page.getByRole("button", { name: "説明文ファイルを書き出す", exact: true }).click();
  const text = await readFile(await (await downloaded).path(), "utf8");
  expect(text.trim()).toBe("[sessions.sample]");
});

test("verified presentation suggestions ignore stale responses and preserve edited public text", async ({ page, app }) => {
  app.cli("creator-run", app.projectPath("complete"), "later", original, current, output,
    "--subject", "Later subject", "--creator", "Later creator", "--decision", "defer", "--rationale", "Later fixture");
  const suggestion = (creator, subject) => JSON.stringify({ creator_display_name: creator, title: subject });
  await page.route("**/presentation-suggestions", async route => {
    if (route.request().url().endsWith("/sample/presentation-suggestions")) {
      await new Promise(resolve => setTimeout(resolve, 100));
      await route.fulfill({ contentType: "application/json", body: suggestion("Browser tester", "Comparison browser fixture") });
      return;
    }
    await route.fulfill({ contentType: "application/json", body: suggestion("Later creator", "Later subject") });
  });
  await page.goto(`${app.origin}/projects/complete/presentation`);
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("sample");
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("later");
  await expect(page.locator('[name="creator_display_name"]')).toHaveValue("Later creator");
  await expect(page.locator('[name="title"]')).toHaveValue("Later subject");
  await page.locator('[name="creator_display_name"]').fill("Edited public creator");
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("sample");
  await expect(page.locator('[name="creator_display_name"]')).toHaveValue("Edited public creator");
  await expect(page.locator('[name="title"]')).toHaveValue("Comparison browser fixture");
  await expect(page.getByRole("checkbox", { name: "候補を確認し、公開用の文章として使うことを確認しました。", exact: true })).not.toBeChecked();
  await page.getByRole("button", { name: "入力した文章を確認", exact: true }).click();
  await expect(page.locator("[data-presentation-preview]")).toBeHidden();
});

test("editing a confirmed suggestion requires a new acknowledgement and rejects unknown suggestion fields", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete/presentation`);
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("sample");
  await expect(page.locator('[data-presentation-suggestions-help]')).toBeVisible();
  await page.getByRole("checkbox", { name: "候補を確認し、公開用の文章として使うことを確認しました。", exact: true }).check();
  await page.locator('[name="title"]').fill("Edited public title");
  await expect(page.getByRole("checkbox", { name: "候補を確認し、公開用の文章として使うことを確認しました。", exact: true })).not.toBeChecked();
  await page.getByRole("button", { name: "入力した文章を確認", exact: true }).click();
  await expect(page.locator("[data-presentation-preview]")).toBeHidden();
  await page.route("**/presentation-suggestions", route => route.fulfill({ contentType: "application/json", body: JSON.stringify({ title: "ok", creator_display_name: "ok", private_note: "no" }) }));
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("");
  await page.getByLabel("完了したセッション", { exact: true }).selectOption("sample");
  await expect(page.locator("[data-presentation-status]")).toContainText("公開用候補の応答が不正");
});
