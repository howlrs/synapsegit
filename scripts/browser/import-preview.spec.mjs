import { test, expect, original, current, output } from "./fixtures.mjs";
import AxeBuilder from "@axe-core/playwright";
import { readFile } from "node:fs/promises";

const form = (page) => page.locator("form[data-synapse-creator-upload]");
const field = (page, name) => page.locator(`[data-creator-file]:has(input[name="${name}"])`);
const preview = (page, name) => field(page, name).locator("[data-creator-preview]");
const summary = (page) => page.locator("[data-creator-file-summary]");

async function visit(page, app) {
  await page.goto(`${app.origin}/projects/pending`);
  await expect(form(page)).toBeVisible();
  await expect(page.locator("[data-creator-preview-note]")).toBeVisible();
}

async function details(page, name) {
  await page.getByLabel("Session", { exact: true }).fill(name);
  await page.getByLabel("Creator name", { exact: true }).fill("Preview tester");
  await page.getByLabel("Subject label", { exact: true }).fill("Local preview fixture");
}

async function selectFiles(page) {
  await page.getByLabel("Original image", { exact: true }).setInputFiles(original);
  await page.getByLabel("Current image", { exact: true }).setInputFiles(current);
  await page.getByLabel("AI output (caller-supplied)", { exact: true }).setInputFiles(output);
  await expect(page.locator("[data-creator-preview]:visible")).toHaveCount(3);
}

async function observeResources(page) {
  await page.addInitScript(() => {
    window.previewUrls = [];
    window.revokedPreviewUrls = [];
    window.previewCspViolations = [];
    const create = URL.createObjectURL.bind(URL);
    const revoke = URL.revokeObjectURL.bind(URL);
    URL.createObjectURL = (blob) => {
      const url = create(blob);
      window.previewUrls.push(url);
      return url;
    };
    URL.revokeObjectURL = (url) => { window.revokedPreviewUrls.push(url); revoke(url); };
    document.addEventListener("securitypolicyviolation", (event) => window.previewCspViolations.push(event.violatedDirective));
  });
}

test("selecting and clearing files shows local previews and sizes without an API request", async ({ page, app }) => {
  const apiRequests = [];
  page.on("request", (request) => { if (request.url().startsWith(`${app.origin}/api/`)) apiRequests.push(request.url()); });
  await observeResources(page);
  await visit(page, app);
  await expect(summary(page)).toContainText("0 / 3");
  await selectFiles(page);
  await expect(summary(page)).toContainText("3 / 3");
  await expect(field(page, "original_image").locator("[data-creator-file-info]")).toContainText("mural-original.png");
  await expect(field(page, "original_image").locator("[data-creator-file-info]")).toContainText("bytes");
  await expect(field(page, "original_image").locator("[data-creator-preview-status]")).toContainText("px · ローカルプレビュー");
  const oldUrl = await preview(page, "original_image").getAttribute("src");
  await page.getByRole("button", { name: "Original imageの選択を解除", exact: true }).click();
  await expect(preview(page, "original_image")).not.toBeVisible();
  await expect(preview(page, "original_image")).not.toHaveAttribute("src");
  await expect(page.getByLabel("Original image", { exact: true })).toBeFocused();
  await expect(summary(page)).toContainText("2 / 3");
  expect(await page.evaluate((url) => window.revokedPreviewUrls.includes(url), oldUrl)).toBe(true);
  expect(apiRequests).toEqual([]);
  expect(await page.evaluate(() => window.previewCspViolations)).toEqual([]);
});

test("Japanese byte limits and oversized files block submission before a request, and correcting them restores validity", async ({ page, app }) => {
  const requests = [];
  page.on("request", (request) => { if (request.method() === "POST") requests.push(request.url()); });
  await visit(page, app);
  await details(page, "preflight-limits");
  await selectFiles(page);
  const creator = page.getByLabel("Creator name", { exact: true });
  await creator.fill("あ".repeat(101));
  await expect(page.locator('[data-creator-text-count="creator_name"]')).toContainText("303 / 300 bytes");
  await expect(creator).toHaveAttribute("aria-invalid", "true");
  await page.getByRole("button", { name: "Proposalを作成" }).click();
  await expect(creator).toBeFocused();
  expect(requests).toEqual([]);
  await creator.fill("あ".repeat(100));
  expect(await creator.evaluate((input) => input.checkValidity())).toBe(true);
  const subject = page.getByLabel("Subject label", { exact: true });
  await subject.fill("あ".repeat(167));
  await expect(page.locator('[data-creator-text-count="subject_label"]')).toContainText("501 / 500 bytes");
  expect(await subject.evaluate((input) => input.checkValidity())).toBe(false);
  await subject.fill("あ".repeat(166) + "ab");
  expect(await subject.evaluate((input) => input.checkValidity())).toBe(true);
  const fileInput = page.getByLabel("Original image", { exact: true });
  await fileInput.evaluate((input) => {
    const transfer = new DataTransfer();
    transfer.items.add(new File([new Uint8Array(64 * 1024 * 1024 + 1)], "too-large.png", { type: "image/png" }));
    input.files = transfer.files;
    input.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await expect(field(page, "original_image").locator("[data-creator-preview-status]")).toContainText("64 MiB以内");
  await expect(preview(page, "original_image")).not.toHaveAttribute("src");
  await page.getByRole("button", { name: "Proposalを作成" }).click();
  await expect(fileInput).toBeFocused();
  expect(requests).toEqual([]);
  await fileInput.setInputFiles(original);
  await expect(preview(page, "original_image")).toBeVisible();
  expect(await form(page).evaluate((element) => element.checkValidity())).toBe(true);
});

test("untrusted filename and MIME do not authorize inline SVG; broken and opaque bytes remain importable", async ({ page, app }) => {
  await observeResources(page);
  await visit(page, app);
  await details(page, "opaque-preview");
  const name = '<img data-injected src=x>.png';
  const svg = Buffer.from('<svg xmlns="http://www.w3.org/2000/svg" onload="window.previewInjected=true"><rect width="1" height="1"/></svg>');
  await page.getByLabel("Original image", { exact: true }).setInputFiles({ name, mimeType: "image/png", buffer: svg });
  await expect(field(page, "original_image").locator("[data-creator-file-info]")).toContainText(name);
  await expect(field(page, "original_image").locator("[data-creator-preview-status]")).toContainText("この形式はプレビューできません");
  expect(await page.evaluate(() => window.previewUrls.length)).toBe(0);
  await expect(page.locator("[data-injected]")).toHaveCount(0);
  await page.getByLabel("Current image", { exact: true }).setInputFiles({
    name: "broken.png", mimeType: "image/png", buffer: Buffer.from([137, 80, 78, 71, 13, 10, 26, 10, 0]),
  });
  await expect(field(page, "current_image").locator("[data-creator-preview-status]")).toContainText("プレビューを表示できません");
  await expect(preview(page, "current_image")).not.toHaveAttribute("src");
  expect(await page.evaluate(() => window.revokedPreviewUrls.length)).toBe(1);
  await page.getByLabel("AI output (caller-supplied)", { exact: true }).setInputFiles({ name: "empty.bin", mimeType: "application/octet-stream", buffer: Buffer.alloc(0) });
  await expect(field(page, "ai_output").locator("[data-creator-preview-status]")).toContainText("そのまま取り込めます");
  expect(await form(page).evaluate((element) => element.checkValidity())).toBe(true);
  expect(await page.evaluate(() => window.previewInjected)).toBeUndefined();
  await page.getByRole("button", { name: "Proposalを作成" }).click();
  await page.waitForURL("**/creator-sessions/opaque-preview");
  await expect(page.getByRole("button", { name: "Defer", exact: true })).toBeVisible();
});

test("upload locks the selected content while pending, then preserves it when the request fails", async ({ page, app }) => {
  await visit(page, app);
  await details(page, "request-failure");
  await selectFiles(page);
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const requested = new Promise((resolve) => {
    page.route("**/api/v1/projects/pending/creator-sessions", async (route) => {
      resolve();
      await gate;
      await route.fulfill({ status: 503, contentType: "application/problem+json", body: JSON.stringify({ detail: "Test storage unavailable" }) });
    });
  });
  try {
    await page.getByRole("button", { name: "Proposalを作成" }).click();
    await requested;
    for (const control of await form(page).locator("input, button").all()) await expect(control).toBeDisabled();
    await expect(page.locator("[data-creator-preview]:visible")).toHaveCount(3);
  } finally {
    release();
  }
  await expect(form(page).locator("[data-synapse-status]")).toContainText("Test storage unavailable");
  await expect(page.getByLabel("Original image", { exact: true })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Original imageの選択を解除", exact: true })).toBeEnabled();
  await expect(page.getByLabel("Creator name", { exact: true })).toHaveValue("Preview tester");
  await expect(summary(page)).toContainText("3 / 3");
});

test("replacement, form reset and persisted page lifecycle revoke old previews without stale decoding", async ({ page, app }) => {
  await observeResources(page);
  await visit(page, app);
  await selectFiles(page);
  const firstUrls = await page.evaluate(() => [...window.previewUrls]);
  await page.getByLabel("Original image", { exact: true }).setInputFiles(current);
  await expect(field(page, "original_image").locator("[data-creator-file-info]")).toContainText("mural-current.png");
  await expect(preview(page, "original_image")).toBeVisible();
  expect(await page.evaluate((url) => window.revokedPreviewUrls.includes(url), firstUrls[0])).toBe(true);
  await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })));
  await expect(page.locator("[data-creator-preview][src]")).toHaveCount(0);
  expect(await page.evaluate(() => window.previewUrls.every((url) => window.revokedPreviewUrls.includes(url)))).toBe(true);
  await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })));
  await expect(page.locator("[data-creator-preview]:visible")).toHaveCount(3);
  await form(page).evaluate((element) => element.reset());
  await expect(summary(page)).toContainText("0 / 3");
  await expect(page.locator("[data-creator-preview][src]")).toHaveCount(0);
  await expect(page.locator('[data-creator-text-count="creator_name"]')).toHaveText("0 / 300 bytes");
  expect(await page.evaluate(() => window.previewUrls.every((url) => window.revokedPreviewUrls.includes(url)))).toBe(true);
});

test("a late signature read cannot replace a more recently selected file", async ({ page, app }) => {
  await page.addInitScript(() => {
    const read = Blob.prototype.arrayBuffer;
    let captured = false;
    Blob.prototype.arrayBuffer = async function () {
      const first = this.size === 12 && !captured;
      if (first) {
        captured = true;
        await new Promise((resolve) => { window.finishFirstRead = resolve; });
      }
      const result = await read.call(this);
      if (first) window.firstReadComplete = true;
      return result;
    };
  });
  await visit(page, app);
  await page.getByLabel("Original image", { exact: true }).setInputFiles(original);
  await expect.poll(() => page.evaluate(() => typeof window.finishFirstRead)).toBe("function");
  await page.getByLabel("Original image", { exact: true }).setInputFiles(current);
  await expect(preview(page, "original_image")).toBeVisible();
  const currentUrl = await preview(page, "original_image").getAttribute("src");
  await page.evaluate(() => window.finishFirstRead());
  await expect.poll(() => page.evaluate(() => window.firstReadComplete)).toBe(true);
  await expect(preview(page, "original_image")).toHaveAttribute("src", currentUrl);
  await expect(field(page, "original_image").locator("[data-creator-file-info]")).toContainText("mural-current.png");
});

test("mobile preflight has explicit labels and no automated accessibility violations", async ({ page, app }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await visit(page, app);
  const bytes = await readFile(original);
  await page.getByLabel("Original image", { exact: true }).setInputFiles({ name: `${"long".repeat(30)}.png`, mimeType: "application/octet-stream", buffer: bytes });
  await expect(preview(page, "original_image")).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  const results = await new AxeBuilder({ page }).include('section[aria-labelledby="creator-upload-heading"]').withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
  expect(results.violations).toEqual([]);
});
