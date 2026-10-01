import AxeBuilder from "@axe-core/playwright";
import { archiveTest, isolatedTest as test, expect, original, current, output } from "./fixtures.mjs";

async function beginEnglishReview(page, app, session) {
  await page.goto(`${app.origin}/projects/reviews/import?lang=en`);
  await page.locator('[name="session"]').fill(session);
  await page.locator('[name="creator_name"]').fill("English browser reviewer");
  await page.locator('[name="subject_label"]').fill("English review fixture");
  for (const [name, file] of [["original_image", original], ["current_image", current], ["ai_output", output]]) {
    await page.locator(`[name="${name}"]`).setInputFiles(file);
  }
  await page.getByRole("button", { name: "Create proposal", exact: true }).click();
  await page.waitForURL(`**/creator-sessions/${session}`);
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
}

async function acceptDecision(page, choice) {
  page.once("dialog", (dialog) => dialog.accept());
  const navigation = page.waitForEvent("framenavigated", { predicate: (frame) => frame === page.mainFrame() });
  await choice.click();
  await navigation;
  await page.waitForLoadState("domcontentloaded");
}

test("English selection persists through import, decision, recorded rationale, and a narrow layout", async ({ page, app }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${app.origin}/projects/reviews/import?lang=en`);
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.getByRole("link", { name: "English", exact: true })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { name: "Start from three files", exact: true })).toBeVisible();

  await page.locator('[name="session"]').fill("english-review");
  await page.locator('[name="creator_name"]').fill("English browser reviewer");
  await page.locator('[name="subject_label"]').fill("English review fixture");
  await page.locator('[name="original_image"]').setInputFiles(original);
  await page.locator('[name="current_image"]').setInputFiles(current);
  await page.locator('[name="ai_output"]').setInputFiles(output);
  await page.getByRole("button", { name: "Create proposal", exact: true }).click();
  await page.waitForURL("**/creator-sessions/english-review");
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.getByRole("heading", { name: "Record a decision", exact: true })).toBeVisible();

  const rationale = page.getByLabel("Rationale (optional)", { exact: true });
  await rationale.fill("The English rationale is retained after recording.");
  const submit = page.getByRole("button", { name: "Defer", exact: true });
  await expect(submit).toHaveAccessibleDescription("Record a decision to defer adopting the AI output.");
  page.once("dialog", (dialog) => dialog.accept());
  const navigation = page.waitForEvent("framenavigated", { predicate: (frame) => frame === page.mainFrame() });
  await submit.click();
  await navigation;
  await expect(page.getByRole("heading", { name: "Recorded decision", exact: true })).toBeVisible();
  await expect(page.locator("[data-decision-rationale]")).toHaveText("The English rationale is retained after recording.");
  await expect(page.locator("[data-decision-outcome]")).toContainText("A decision to defer adopting the AI output was recorded.");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);

  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page.locator("[data-decision-rationale]")).toHaveText("The English rationale is retained after recording.");
});

test("the explicit language choice takes priority and preserves unrelated page parameters", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete?filter=complete&lang=en`);
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await expect(page).toHaveURL(`${app.origin}/projects/complete?filter=complete`);
  const selectedLanguage = page.getByRole("link", { name: "English", exact: true });
  const otherLanguage = page.getByRole("link", { name: "日本語", exact: true });
  await expect(selectedLanguage).toHaveAttribute("aria-current", "page");
  expect(await selectedLanguage.evaluate((link) => getComputedStyle(link).backgroundColor))
    .not.toBe(await otherLanguage.evaluate((link) => getComputedStyle(link).backgroundColor));
  await expect(page.getByRole("link", { name: "日本語", exact: true })).toHaveAttribute("href", "/projects/complete?filter=complete&lang=ja");
  await page.goto(`${app.origin}/projects/complete?filter=complete`);
  await expect(page.locator("html")).toHaveAttribute("lang", "en");
  await page.getByRole("link", { name: "日本語", exact: true }).click();
  await expect(page.locator("html")).toHaveAttribute("lang", "ja");
  await expect(page).toHaveURL(`${app.origin}/projects/complete?filter=complete`);
});

for (const [button, disposition, description, outcome] of [
  ["Adopt", "adopt", "Adopt the AI output without changes.", "The AI output was adopted without changes."],
  ["Reject", "reject", "Record a decision not to adopt the AI output.", "A decision not to adopt the AI output was recorded."],
  ["Defer", "defer", "Record a decision to defer adopting the AI output.", "A decision to defer adopting the AI output was recorded."],
]) {
  test(`English ${button} confirmation records the exact rationale`, async ({ page, app }) => {
    await beginEnglishReview(page, app, `english-${disposition}`);
    const rationale = page.getByLabel("Rationale (optional)", { exact: true });
    const reason = `English ${button} rationale`;
    await rationale.fill(reason);
    const choice = page.getByRole("button", { name: button, exact: true });
    await expect(choice).toHaveAccessibleDescription(description);
    await acceptDecision(page, choice);
    await expect(page.getByRole("heading", { name: "Recorded decision", exact: true })).toBeVisible();
    await expect(page.locator("[data-decision-rationale]")).toHaveText(reason);
    await expect(page.locator("[data-decision-outcome]")).toHaveText(outcome);
    await page.reload();
    await expect(page.locator("html")).toHaveAttribute("lang", "en");
    await expect(page.locator("[data-decision-rationale]")).toHaveText(reason);
  });
}

test("English cancellation keeps the rationale and keyboard retry records it", async ({ page, app }) => {
  await beginEnglishReview(page, app, "english-cancel");
  const rationale = page.getByLabel("Rationale (optional)", { exact: true });
  const choice = page.getByRole("button", { name: "Defer", exact: true });
  await rationale.fill("Keep this after cancelling.");
  let decisionPosts = 0;
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/decisions")) decisionPosts += 1;
  });
  page.once("dialog", (dialog) => dialog.dismiss());
  await choice.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("[data-synapse-status]")).toHaveText("The Decision was not sent.");
  await expect(rationale).toHaveValue("Keep this after cancelling.");
  await expect(choice).toBeFocused();
  expect(decisionPosts).toBe(0);
  await acceptDecision(page, choice);
  await expect(page.locator("[data-decision-rationale]")).toHaveText("Keep this after cancelling.");
  expect(decisionPosts).toBe(1);
});

test("English public text form checks author supplied text on a narrow screen", async ({ page, app }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${app.origin}/projects/complete/presentation?lang=en`);
  await expect(page.getByRole("heading", { name: "Create public production notes", exact: true })).toBeVisible();
  await page.getByLabel("Completed session", { exact: true }).selectOption("sample");
  await page.getByLabel("Work title (optional)", { exact: true }).fill("English public title");
  await page.getByLabel("Summary (optional)", { exact: true }).fill("Text entered for publication review only.");
  const check = page.getByRole("button", { name: "Check the entered text", exact: true });
  await check.focus();
  await page.keyboard.press("Enter");
  await expect(page.getByRole("heading", { name: "Review the public text", exact: true })).toBeVisible();
  await expect(page.locator("[data-presentation-text]")).toContainText("English public title");
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
});

async function deferEnglishSource(page, app, source) {
  await beginEnglishReview(page, app, source);
  await page.getByLabel("Rationale (optional)", { exact: true }).fill("Source decision remains a reference.");
  await acceptDecision(page, page.getByRole("button", { name: "Defer", exact: true }));
}

test("English derive carries reference images into a separate session", async ({ page, app }) => {
  test.setTimeout(120_000);
  await deferEnglishSource(page, app, "english-derive-source");
  await page.getByRole("link", { name: "Try a next candidate from this record", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Source and reused reference images", exact: true })).toBeVisible();
  await page.locator('[name="session"]').fill("english-derived");
  await page.locator('[name="ai_output"]').setInputFiles(output);
  await page.getByRole("button", { name: "Create a proposal with the reference images", exact: true }).click();
  await page.waitForURL("**/creator-sessions/english-derived");
  await expect(page.locator("[data-creator-source]")).toContainText("english-derive-source");
});

test("English re-review creates a separate session from a deferred record", async ({ page, app }) => {
  test.setTimeout(120_000);
  await deferEnglishSource(page, app, "english-reuse-source");
  await page.getByRole("link", { name: "Re-review the deferred proposal in a new session", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Review a recorded proposal in a new session", exact: true })).toBeVisible();
  await page.locator('[name="session"]').fill("english-reused");
  await page.getByRole("button", { name: "Review this proposal in a new session", exact: true }).click();
  await page.waitForURL("**/creator-sessions/english-reused");
  await expect(page.locator("[data-creator-reuse-source]")).toContainText("english-reuse-source");
});

archiveTest("English maintenance requires exact keys and an explicit empty-target acknowledgement", async ({ page, app }) => {
  const exportForm = page.locator('form[data-confirm-maintenance="archive-export"]');
  await page.goto(`${app.origin}/projects/complete/maintenance?lang=en`);
  const fsck = page.locator('form[data-confirm-maintenance="fsck"]');
  const fsckKey = fsck.locator('[name="confirm_project_key"]');
  const fsckButton = fsck.getByRole("button", { name: "Check integrity (read-only)", exact: true });
  await expect(fsck).toContainText("To confirm, type the project key complete");
  await fsckKey.fill("completex");
  page.once("dialog", (dialog) => dialog.accept());
  await fsckButton.click();
  await expect(fsck.locator("[data-synapse-status]")).toContainText("local safety checks rejected");
  await fsckKey.fill("complete");
  page.once("dialog", (dialog) => dialog.accept());
  const reload = page.waitForEvent("framenavigated", { predicate: (frame) => frame === page.mainFrame() });
  await fsckButton.click();
  await reload;
  await expect(page.getByText("Latest result since this app started:", { exact: false })).toBeVisible();

  await exportForm.locator('[name="archive_name"]').fill("english-archive");
  await exportForm.locator('[name="confirm_project_key"]').fill("complete");
  page.once("dialog", (dialog) => dialog.accept());
  await exportForm.getByRole("button", { name: "Create archive", exact: true }).click();
  await page.waitForURL("**/#archives-heading");
  await page.goto(`${app.origin}/projects/restore/maintenance?lang=en`);
  const restore = page.locator('form[data-archive-restore="true"]');
  const name = restore.locator('[name="archive_name"]');
  const key = restore.locator('[name="confirm_target_project_key"]');
  const empty = restore.locator('[name="confirm_empty_target"]');
  const submit = restore.getByRole("button", { name: "Restore archive", exact: true });
  await name.fill("english-archive");
  await key.fill("restorex");
  await empty.check();
  await submit.click();
  await expect(restore.locator("[data-synapse-status]")).toContainText("confirmation");
  await key.fill("restore");
  await empty.uncheck();
  await submit.click();
  await expect(empty).toBeFocused();
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);
});
