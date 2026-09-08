import { test, expect, original, current, output } from "./fixtures.mjs";
import AxeBuilder from "@axe-core/playwright";

const opener = (page) => page.getByRole("button", { name: "画像を拡大して比較" });
const dialog = (page) => page.getByRole("dialog", { name: "画像を見比べる" });
const compareImages = (page) => page.locator("[data-synapse-compare-image]");
const cards = (page) => page.locator("img[data-synapse-image]");

async function visit(page, app, project = "complete") {
  await page.goto(`${app.origin}/projects/${project}/creator-sessions/sample`);
  await expect(cards(page).locator('..').first()).toBeVisible();
  await expect(page.locator('img[data-synapse-image][aria-busy="true"]')).toHaveCount(0);
}

async function openComparison(page) {
  await expect(opener(page)).toBeEnabled();
  await opener(page).click();
  await expect(dialog(page)).toBeVisible();
  await expect(page.getByRole("button", { name: "閉じる", exact: true })).toBeFocused();
}

test("complete session: keyboard open, role selection, aspect-correct zoom, close and no new API reads or writes", async ({ page, app }) => {
  const requests = [];
  const errors = [];
  page.on("request", (request) => { if (request.url().includes("/api/")) requests.push([request.method(), request.url()]); });
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    window.cspViolations = [];
    document.addEventListener("securitypolicyviolation", (event) => window.cspViolations.push(event.violatedDirective));
  });
  await visit(page, app);
  await expect(opener(page)).toBeEnabled();
  const initialRequests = requests.length;
  await opener(page).focus();
  await page.keyboard.press("Enter");
  await expect(dialog(page)).toBeVisible();
  await expect(page.getByLabel("比較画像 A", { exact: true })).toHaveValue("1");
  await expect(page.getByLabel("比較画像 B", { exact: true })).toHaveValue("2");
  const dimensions = await cards(page).nth(1).evaluate((image) => [image.naturalWidth, image.naturalHeight]);
  for (const [zoom, scale] of [["1", 1], ["2", 2]]) {
    await page.getByLabel("表示倍率").selectOption(zoom);
    await expect(compareImages(page).first()).toHaveAttribute("width", String(dimensions[0] * scale));
    await expect(compareImages(page).first()).toHaveAttribute("height", String(dimensions[1] * scale));
    const box = await compareImages(page).first().boundingBox();
    expect(box.width).toBe(dimensions[0] * scale);
    expect(box.height).toBe(dimensions[1] * scale);
  }
  const viewport = page.getByRole("region", { name: "比較画像 Aの表示領域" });
  await viewport.focus();
  await page.keyboard.press("ArrowRight");
  await expect.poll(() => viewport.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0);
  await page.getByLabel("比較画像 A", { exact: true }).selectOption("0");
  await expect(compareImages(page).first()).toHaveAttribute("src", await cards(page).first().getAttribute("src"));
  await expect(page.locator("[data-synapse-compare-caption]").first()).toContainText("Original");
  await page.getByLabel("表示倍率").selectOption("fit");
  expect(await compareImages(page).first().evaluate((image) => getComputedStyle(image).objectFit)).toBe("contain");
  await page.keyboard.press("Escape");
  await expect(dialog(page)).not.toBeVisible();
  await expect(opener(page)).toBeFocused();
  await expect(page.locator("[data-synapse-compare-image][src]")).toHaveCount(0);
  await openComparison(page);
  await expect(page.getByLabel("表示倍率")).toHaveValue("fit");
  await page.getByRole("button", { name: "閉じる", exact: true }).click();
  expect(requests.length).toBe(initialRequests);
  expect(requests.every(([method]) => method === "GET")).toBe(true);
  expect(errors).toEqual([]);
  expect(await page.evaluate(() => window.cspViolations)).toEqual([]);
});

test("mobile, dark and reduced motion: visible controls, modal focus and automated accessibility", async ({ page, app }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.emulateMedia({ colorScheme: "dark", reducedMotion: "reduce" });
  await visit(page, app);
  await openComparison(page);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  const panes = await page.locator("[data-synapse-compare-pane]").evaluateAll((elements) => elements.map((element) => {
    const box = element.getBoundingClientRect();
    return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
  }));
  expect(panes[1].top).toBeGreaterThan(panes[0].bottom);
  expect(panes.every((pane) => pane.left >= 0 && pane.right <= 390)).toBe(true);
  for (let i = 0; i < 12; i += 1) {
    await page.keyboard.press("Tab");
    expect(await page.evaluate(() => document.activeElement === document.body || document.querySelector("dialog").contains(document.activeElement))).toBe(true);
  }
  const results = await new AxeBuilder({ page }).include("#image-comparison").withTags(["wcag2a", "wcag2aa", "wcag21aa"]).analyze();
  expect(results.violations).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(opener(page)).toBeFocused();
});

test("pending review: import, compare and explicitly defer from the review form", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/pending`);
  await page.locator('[name="session"]').fill("browser-review");
  await page.locator('[name="creator_name"]').fill("Browser reviewer");
  await page.locator('[name="subject_label"]').fill("Mural review");
  await page.locator('[name="original_image"]').setInputFiles(original);
  await page.locator('[name="current_image"]').setInputFiles(current);
  await page.locator('[name="ai_output"]').setInputFiles(output);
  await page.getByRole("button", { name: "Proposalを作成" }).click();
  await page.waitForURL("**/creator-sessions/browser-review");
  await openComparison(page);
  await page.getByLabel("表示倍率").selectOption("2");
  await page.keyboard.press("Escape");
  await page.getByLabel("Rationale（任意）").fill("Need another visual inspection.");
  page.once("dialog", (confirmation) => confirmation.accept());
  await page.getByRole("button", { name: "Defer", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Timeline", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Defer", exact: true })).toHaveCount(0);
  await openComparison(page);
});

test("attachment-only roles never enter comparison and broken rasters show actionable errors", async ({ page, app }) => {
  await visit(page, app, "mixed");
  await openComparison(page);
  await expect(page.getByLabel("比較画像 A", { exact: true })).toHaveValue("0");
  await expect(page.getByLabel("比較画像 B", { exact: true })).toHaveValue("2");
  await expect(page.locator('[data-synapse-compare-source] option[value="1"]')).toHaveCount(2);
  for (const option of await page.locator('[data-synapse-compare-source] option[value="1"]').all()) await expect(option).toBeDisabled();
  const attachmentUrl = await page.locator("[data-synapse-image-download]").nth(1).getAttribute("href");
  for (const image of await compareImages(page).all()) expect(await image.getAttribute("src")).not.toBe(attachmentUrl);
  await page.keyboard.press("Escape");
  await visit(page, app, "broken");
  await expect(opener(page)).toBeDisabled();
  await expect(page.locator("[data-synapse-image-status]").first()).toContainText("画像を表示できません");
  await expect(page.locator("[data-synapse-image-status]").nth(2)).toContainText("画像を表示できません");
  await expect(page.locator("[data-synapse-image-download]").nth(1)).toBeVisible();
  await expect(page.locator("[data-synapse-compare-image][src]")).toHaveCount(0);
});

test("page lifecycle releases comparison sources and re-fetches after a persisted restore", async ({ page, app }) => {
  await visit(page, app);
  await openComparison(page);
  const oldUrls = await cards(page).evaluateAll((images) => images.map((image) => image.src));
  await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })));
  await expect(dialog(page)).not.toBeVisible();
  await expect(page.locator("[data-synapse-compare-image][src]")).toHaveCount(0);
  await expect(page.locator("img[data-synapse-image][src]")).toHaveCount(0);
  await expect(opener(page)).toBeDisabled();
  await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pageshow", { persisted: true })));
  await openComparison(page);
  await expect(page.locator('img[data-synapse-image][aria-busy="true"]')).toHaveCount(0);
  const newUrls = await cards(page).evaluateAll((images) => images.map((image) => image.src));
  expect(newUrls.every((url) => url.startsWith("blob:") && !oldUrls.includes(url))).toBe(true);
});

test("without JavaScript, comparison stays hidden and history remains readable", async ({ browser, app }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  try {
    const page = await context.newPage();
    await page.goto(`${app.origin}/projects/complete/creator-sessions/sample`);
    await expect(opener(page)).not.toBeVisible();
    await expect(dialog(page)).not.toBeVisible();
    await expect(page.getByRole("heading", { name: "Timeline", exact: true })).toBeVisible();
  } finally {
    await context.close();
  }
});
