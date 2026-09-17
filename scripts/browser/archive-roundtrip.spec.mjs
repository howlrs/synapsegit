import AxeBuilder from "@axe-core/playwright";
import { archiveTest as test, expect } from "./fixtures.mjs";

const archiveName = "browser-roundtrip";
const exportForm = (page) => page.locator('form[data-confirm-maintenance="archive-export"]');
const restoreForm = (page) => page.locator('form[data-archive-restore="true"]');
const status = (form) => form.locator("[data-synapse-status]");

async function acceptNextDialog(page) {
  page.once("dialog", (dialog) => dialog.accept());
}

test("archive cards support keyboard access, pass axe, and restore an exported project", async ({ page, app }) => {
  const sourceRefs = app.refs("complete");

  await page.goto(`${app.origin}/projects/complete`);
  for (const heading of ["Repository integrity check", "Archive export"]) {
    await expect(page.getByRole("heading", { name: heading, exact: true })).toBeVisible();
  }

  await exportForm(page).locator('[name="archive_name"]').fill(archiveName);
  await exportForm(page).locator('[name="confirm_project_key"]').fill("complete");
  await acceptNextDialog(page);
  await exportForm(page).getByRole("button", { name: "Archiveを作成", exact: true }).click();
  await page.waitForURL("**/#archives-heading");
  await expect(page.getByRole("heading", { name: "Archives", exact: true })).toBeVisible();
  await expect(page.locator("section").filter({ has: page.locator("#archives-heading") })).toContainText(archiveName);

  await page.goto(`${app.origin}/projects/restore`);
  const restore = restoreForm(page);
  await expect(restore).toBeVisible();
  for (const heading of ["Repository integrity check", "Archive export", "Archive restore"]) {
    await expect(page.getByRole("heading", { name: heading, exact: true })).toBeVisible();
  }
  expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze()).violations).toEqual([]);

  const fsckKey = page.locator('form[data-confirm-maintenance="fsck"] [name="confirm_project_key"]');
  const fsckButton = page.getByRole("button", { name: "Read-only fsckを実行", exact: true });
  const exportName = exportForm(page).locator('[name="archive_name"]');
  const exportKey = exportForm(page).locator('[name="confirm_project_key"]');
  const exportButton = page.getByRole("button", { name: "Archiveを作成", exact: true });
  const restoreName = restore.locator('[name="archive_name"]');
  const restoreKey = restore.locator('[name="confirm_target_project_key"]');
  const emptyTarget = restore.locator('[name="confirm_empty_target"]');
  const restoreButton = restore.getByRole("button", { name: "Archiveを復元", exact: true });
  await fsckKey.focus();
  await page.keyboard.press("Tab");
  await expect(fsckButton).toBeFocused();
  await exportName.focus();
  await page.keyboard.press("Tab");
  await expect(exportKey).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(exportButton).toBeFocused();
  await restoreName.focus();
  await page.keyboard.press("Tab");
  await expect(restoreKey).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(emptyTarget).toBeFocused();
  await page.keyboard.press("Tab");
  await expect(restoreButton).toBeFocused();

  await restoreName.fill(archiveName);
  await restoreKey.fill("restore");
  await emptyTarget.check();
  await acceptNextDialog(page);
  await restoreButton.click();
  await expect(status(restore)).toContainText("アーカイブ「browser-roundtrip」を復元しました。");
  const restored = page.waitForURL("**/projects/restore");
  await restore.getByRole("link", { name: "復元した履歴を確認", exact: true }).click();
  await restored;
  expect(app.refs("restore")).toBe(sourceRefs);
});

test("restore refusals preserve the target refs", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/complete`);
  await exportForm(page).locator('[name="archive_name"]').fill(archiveName);
  await exportForm(page).locator('[name="confirm_project_key"]').fill("complete");
  await acceptNextDialog(page);
  await exportForm(page).getByRole("button", { name: "Archiveを作成", exact: true }).click();
  await page.waitForURL("**/#archives-heading");

  await page.goto(`${app.origin}/projects/restore`);
  const restore = restoreForm(page);
  const name = restore.locator('[name="archive_name"]');
  const key = restore.locator('[name="confirm_target_project_key"]');
  const empty = restore.locator('[name="confirm_empty_target"]');
  const button = restore.getByRole("button", { name: "Archiveを復元", exact: true });
  const initialRefs = app.refs("restore");
  let requests = 0;
  let dialogs = 0;
  page.on("request", (request) => {
    if (request.method() === "POST" && request.url().endsWith("/archive-restores")) requests += 1;
  });
  page.on("dialog", (dialog) => { dialogs += 1; dialog.accept(); });

  await name.fill(archiveName);
  await key.fill("wrong-target");
  await empty.check();
  await button.click();
  await expect(status(restore)).toContainText("The archive restore confirmation is invalid.");
  expect(app.refs("restore")).toBe(initialRefs);
  expect(requests).toBe(0);
  expect(dialogs).toBe(0);

  await key.fill("restore");
  await empty.uncheck();
  await button.click();
  await expect(empty).toBeFocused();
  expect(app.refs("restore")).toBe(initialRefs);
  expect(requests).toBe(0);
  expect(dialogs).toBe(0);

  // The form was rendered while this fixture project was empty. Populate only
  // this temporary target before submission to exercise the server-side race
  // guard that protects existing refs and reflog entries.
  app.addHistory("restore");
  const occupiedRefs = app.refs("restore");
  expect(occupiedRefs).not.toBe(initialRefs);
  await empty.check();
  await button.click();
  await expect(status(restore)).toContainText("The target project already contains Ref or reflog history.");
  expect(app.refs("restore")).toBe(occupiedRefs);
  expect(requests).toBe(1);
  expect(dialogs).toBe(1);
});
