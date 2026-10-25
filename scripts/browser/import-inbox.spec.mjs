import AxeBuilder from "@axe-core/playwright";
import { inboxTest as test, expect, original, current, output } from "./fixtures.mjs";
import { copyFile, mkdir, stat, writeFile } from "node:fs/promises";
import path from "node:path";

async function addInboxCandidate(app, slug, { subject = "Script subject", creator = "Script creator" } = {}) {
  const candidate = path.join(app.inboxRoot, slug);
  await mkdir(candidate);
  await copyFile(original, path.join(candidate, "original"));
  await copyFile(current, path.join(candidate, "current"));
  await copyFile(output, path.join(candidate, "ai-output"));
  const sizes = await Promise.all([original, current, output].map(async file => (await stat(file)).size));
  await writeFile(path.join(candidate, "manifest.json"), JSON.stringify({ version: "synapsegit-import-inbox-v1", original: { name: "original", size: sizes[0] }, current: { name: "current", size: sizes[1] }, ai_output: { name: "ai-output", size: sizes[2] }, metadata: { subject_label: subject, creator_name: creator } }));
}

async function openInbox(page, app) {
  await page.goto(`${app.origin}/projects/pending/import`);
  await expect(page.locator("[data-import-inbox]")).toBeVisible();
}

function inboxRow(page, slug) {
  return page.locator("[data-import-inbox-list] li").filter({ hasText: slug });
}

async function stageInboxCandidate(page, slug) {
  await inboxRow(page, slug).getByRole("button", { name: "確認する" }).click();
  await expect(page.locator("form[data-import-inbox-preview]")).toBeVisible();
}

async function trackInboxWrites(page) {
  await page.addInitScript(() => {
    const nativeFetch = window.fetch.bind(window);
    window.synapseInboxWrites = [];
    window.fetch = (input, init) => {
      const url = input instanceof Request ? input.url : String(input);
      const method = init?.method || (input instanceof Request ? input.method : "GET");
      if (new URL(url, location.href).pathname.includes("/import-inbox/") && ["POST", "DELETE"].includes(method)) {
        window.synapseInboxWrites.push({ method, path: new URL(url, location.href).pathname, body: init?.body || null });
      }
      return nativeFetch(input, init);
    };
  });
}

async function inboxWrites(page) {
  return page.evaluate(() => window.synapseInboxWrites);
}

test("manifest-last inbox stages preview bytes before explicit Human Decision", async ({ page, app }) => {
  test.setTimeout(120_000);
  const candidate = path.join(app.inboxRoot, "script-candidate");
  await mkdir(candidate);
  await copyFile(original, path.join(candidate, "original"));
  await copyFile(current, path.join(candidate, "current"));
  await copyFile(output, path.join(candidate, "ai-output"));
  const sizes = await Promise.all([original, current, output].map(async file => (await stat(file)).size));
  await writeFile(path.join(candidate, "manifest.json"), JSON.stringify({ version: "synapsegit-import-inbox-v1", original: { name: "original", size: sizes[0] }, current: { name: "current", size: sizes[1] }, ai_output: { name: "ai-output", size: sizes[2] }, metadata: { subject_label: "Script subject", creator_name: "Script creator", generation_note: { tool: "script", model: "test", prompt: "private", intent: "review" } } }));
  await page.goto(`${app.origin}/projects/pending/import`);
  const section = page.locator("[data-import-inbox]");
  await expect(section).toBeVisible();
  await expect(page.locator('[name="session"]')).toHaveCount(1);
  await expect(page.getByLabel("新しいセッション名", { exact: true })).toHaveCount(1);
  await page.getByRole("button", { name: "確認する" }).focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("form[data-import-inbox-preview]")).toBeVisible();
  await expect(page.locator("[data-import-inbox-images] img")).toHaveCount(3);
  const restage = page.getByRole("button", { name: "確認する" });
  for (let attempt = 0; attempt < 9; attempt += 1) {
    const deleted = page.waitForResponse(response => response.request().method() === "DELETE" && response.url().includes("/import-inbox/stages/"));
    await page.getByRole("button", { name: "一覧へ戻る" }).click();
    expect((await deleted).status()).toBe(204);
    await expect(page.locator("form[data-import-inbox-preview]")).toBeHidden();
    await expect(restage).toBeEnabled();
    if (attempt < 8) {
      await restage.click();
      await expect(page.locator("form[data-import-inbox-preview]")).toBeVisible();
    }
  }
  await restage.click();
  await expect(page.locator("form[data-import-inbox-preview]")).toBeVisible();
  await writeFile(path.join(candidate, "original"), "changed after staging");
  const preview = page.locator("form[data-import-inbox-preview]");
  await preview.getByLabel("Inboxのセッション名", { exact: true }).fill("inbox-review");
  await preview.getByLabel("Inboxの作成者名", { exact: true }).fill("Edited creator");
  await preview.locator('[name="inbox_generation_prompt"]').fill("p".repeat(8192));
  await preview.getByRole("button", { name: "提案を作成" }).click();
  await page.waitForURL("**/creator-sessions/inbox-review");
  await page.getByLabel("理由（任意）", { exact: true }).fill("Inbox bytes reviewed.");
  page.once("dialog", dialog => dialog.accept());
  const navigation = page.waitForEvent("framenavigated", { predicate: (frame) => frame === page.mainFrame() });
  await page.getByRole("button", { name: "保留", exact: true }).click();
  await navigation;
  await page.waitForLoadState("domcontentloaded");
  await expect(page.getByRole("heading", { name: "記録した判断", exact: true })).toBeVisible();
  await page.setViewportSize({ width: 390, height: 844 });
  expect(await page.locator("main").evaluate(element => element.scrollWidth <= element.clientWidth)).toBe(true);
  const axe = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
  expect(axe.violations).toEqual([]);
});

test("an imported candidate stays listed as imported and stops counting as waiting", async ({ page, app }) => {
  test.setTimeout(120_000);
  for (const slug of ["first-candidate", "second-candidate"]) {
    const candidate = path.join(app.inboxRoot, slug);
    await mkdir(candidate);
    await copyFile(original, path.join(candidate, "original"));
    await copyFile(current, path.join(candidate, "current"));
    await copyFile(output, path.join(candidate, "ai-output"));
    const sizes = await Promise.all([original, current, output].map(async file => (await stat(file)).size));
    await writeFile(path.join(candidate, "manifest.json"), JSON.stringify({ version: "synapsegit-import-inbox-v1", original: { name: "original", size: sizes[0] }, current: { name: "current", size: sizes[1] }, ai_output: { name: "ai-output", size: sizes[2] }, metadata: { subject_label: "Script subject", creator_name: "Script creator" } }));
  }
  await page.goto(`${app.origin}/projects/pending`);
  await expect(page.locator("[data-inbox-ready-count]")).toHaveText("2 件");

  await page.goto(`${app.origin}/projects/pending/import`);
  const rows = page.locator("[data-import-inbox-list] li");
  await rows.filter({ hasText: "first-candidate" }).getByRole("button", { name: "確認する" }).click();
  const preview = page.locator("form[data-import-inbox-preview]");
  await expect(preview.getByLabel("Inboxのセッション名", { exact: true })).toHaveValue("inbox-first-candidate");
  await preview.getByRole("button", { name: "提案を作成" }).click();
  await page.waitForURL("**/creator-sessions/inbox-first-candidate");

  await page.goto(`${app.origin}/projects/pending`);
  await expect(page.locator("[data-inbox-ready-count]")).toHaveText("1 件");
  await page.goto(`${app.origin}/projects/pending/import`);
  // The waiting candidate comes first; the imported one keeps a link to its session.
  await expect(rows.first()).toContainText("second-candidate");
  const imported = rows.filter({ hasText: "first-candidate" });
  await expect(imported).toContainText("取り込み済みです（セッション inbox-first-candidate）");
  await expect(imported.getByRole("link", { name: "セッション inbox-first-candidate を開く" })).toHaveAttribute("href", "/projects/pending/creator-sessions/inbox-first-candidate");
  await expect(imported.getByRole("button", { name: "別のセッションとして取り込む" })).toBeVisible();
  expect((await new AxeBuilder({ page }).include("#main").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);

  await rows.filter({ hasText: "second-candidate" }).getByRole("button", { name: "確認する" }).click();
  await preview.getByRole("button", { name: "提案を作成" }).click();
  await page.waitForURL("**/creator-sessions/inbox-second-candidate");
  await page.goto(`${app.origin}/projects/pending`);
  await expect(page.locator("[data-inbox-ready-count]")).toHaveCount(0);
  await expect(page.locator("[data-inbox-ready-notice]")).toHaveCount(0);
  await page.goto(`${app.origin}/projects/pending/import`);
  await expect(page.locator("[data-import-inbox-status]")).toHaveText("取り込み待ちの候補はありません。取り込み済みの候補は下に表示しています。");
});

test("staging one inbox candidate locks every choice until its response settles", async ({ page, app }) => {
  await addInboxCandidate(app, "first", { subject: "First subject" });
  await addInboxCandidate(app, "second", { subject: "Second subject" });
  await trackInboxWrites(page);
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  let requested;
  const firstRequested = new Promise(resolve => { requested = resolve; });
  await page.route("**/import-inbox/*/stages", async route => {
    requested();
    await gate;
    await route.continue();
  });
  await openInbox(page, app);
  const first = inboxRow(page, "first").getByRole("button", { name: "確認する" });
  const second = inboxRow(page, "second").getByRole("button", { name: "確認する" });
  try {
    await first.click();
    await firstRequested;
    await expect(first).toBeDisabled();
    await expect(second).toBeDisabled();
    // The busy guard also rejects an event that bypasses native disabled-click handling.
    await second.evaluate(button => button.dispatchEvent(new Event("click")));
    expect(await inboxWrites(page)).toHaveLength(1);
  } finally {
    release();
  }
  await expect(page.getByLabel("Inboxのセッション名", { exact: true })).toHaveValue("inbox-first");
  await expect(page.locator("[data-import-inbox-images] img")).toHaveCount(3);
  await page.unroute("**/import-inbox/*/stages");
});

test("a failed inbox staging request restores every choice for a retry", async ({ page, app }) => {
  await addInboxCandidate(app, "retry");
  await addInboxCandidate(app, "other");
  let requests = 0;
  await page.route("**/import-inbox/retry/stages", async route => {
    requests += 1;
    if (requests === 1) {
      await route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ detail: "Temporary staging failure" }) });
      return;
    }
    await route.continue();
  });
  await openInbox(page, app);
  const review = inboxRow(page, "retry").getByRole("button", { name: "確認する" });
  await review.click();
  await expect(page.locator("[data-import-inbox-status]")).toContainText("Temporary staging failure");
  await expect(review).toBeEnabled();
  await expect(inboxRow(page, "other").getByRole("button", { name: "確認する" })).toBeEnabled();
  await review.click();
  await expect(page.locator("form[data-import-inbox-preview]")).toBeVisible();
  expect(requests).toBe(2);
  await page.unroute("**/import-inbox/retry/stages");
});

test("discarding a staged inbox candidate locks its other actions and preserves the preview on failure", async ({ page, app }) => {
  await addInboxCandidate(app, "discard");
  await trackInboxWrites(page);
  await openInbox(page, app);
  await stageInboxCandidate(page, "discard");
  await page.evaluate(() => { window.synapseInboxWrites = []; });
  const preview = page.locator("form[data-import-inbox-preview]");
  const cancel = preview.getByRole("button", { name: "一覧へ戻る" });
  const submit = preview.getByRole("button", { name: "提案を作成" });
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  let requested;
  const firstRequested = new Promise(resolve => { requested = resolve; });
  await page.route("**/import-inbox/stages/*", async route => {
    if (route.request().method() !== "DELETE") return route.continue();
    requested();
    await gate;
    await route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ detail: "Temporary discard failure" }) });
  });
  try {
    await cancel.click();
    await firstRequested;
    await expect(cancel).toBeDisabled();
    await expect(submit).toBeDisabled();
    await cancel.evaluate(button => button.dispatchEvent(new Event("click")));
    await preview.evaluate(form => form.dispatchEvent(new Event("submit", { cancelable: true })));
    expect(await inboxWrites(page)).toEqual([expect.objectContaining({ method: "DELETE" })]);
  } finally {
    release();
  }
  await expect(page.locator("[data-import-inbox-status]")).toContainText("Temporary discard failure");
  await expect(preview).toBeVisible();
  await expect(cancel).toBeEnabled();
  await expect(submit).toBeEnabled();
  await page.unroute("**/import-inbox/stages/*");
});

test("a pending inbox proposal locks cancel and duplicate submission, then restores both after failure", async ({ page, app }) => {
  await addInboxCandidate(app, "submit");
  await trackInboxWrites(page);
  await openInbox(page, app);
  await stageInboxCandidate(page, "submit");
  await page.evaluate(() => { window.synapseInboxWrites = []; });
  const preview = page.locator("form[data-import-inbox-preview]");
  const cancel = preview.getByRole("button", { name: "一覧へ戻る" });
  const submit = preview.getByRole("button", { name: "提案を作成" });
  const subject = preview.getByLabel("Inboxの対象名", { exact: true });
  await subject.fill("あ".repeat(200));
  let release;
  const gate = new Promise(resolve => { release = resolve; });
  let requested;
  const firstRequested = new Promise(resolve => { requested = resolve; });
  await page.route("**/import-inbox/stages/*/creator-sessions", async route => {
    requested();
    await gate;
    await route.continue();
  });
  try {
    await submit.click();
    await firstRequested;
    await expect(submit).toBeDisabled();
    await expect(cancel).toBeDisabled();
    await cancel.evaluate(button => button.dispatchEvent(new Event("click")));
    await preview.evaluate(form => form.dispatchEvent(new Event("submit", { cancelable: true })));
    expect(await inboxWrites(page)).toEqual([expect.objectContaining({ method: "POST", path: expect.stringMatching(/creator-sessions$/u) })]);
  } finally {
    release();
  }
  await expect(page.locator("[data-import-inbox-status]")).toContainText("The staged import metadata is invalid.");
  await expect(submit).toBeEnabled();
  await expect(cancel).toBeEnabled();
  await expect(subject).toHaveValue("あ".repeat(200));
  await page.unroute("**/import-inbox/stages/*/creator-sessions");
  await preview.getByLabel("Inboxの作成者名", { exact: true }).fill("Retried creator");
  await subject.fill("Corrected subject");
  const retryRequest = page.waitForRequest(request => request.method() === "POST" && request.url().endsWith("/creator-sessions"));
  await submit.click();
  expect(JSON.parse((await retryRequest).postData())).toMatchObject({ creator_name: "Retried creator", subject_label: "Corrected subject" });
  await page.waitForURL("**/creator-sessions/inbox-submit");
});
