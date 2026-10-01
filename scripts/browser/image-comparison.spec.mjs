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
    await dialog(page).getByLabel("表示倍率", { exact: true }).selectOption(zoom);
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
  await dialog(page).getByRole("radio", { name: "重ねて表示" }).check();
  const overlayViewport = page.getByRole("region", { name: "重ねた比較画像の表示領域" });
  const geometry = await page.locator("[data-synapse-compare-overlay-canvas] image").evaluateAll((images) => images.map((image) => { const b = image.getBoundingClientRect(); return [b.x, b.y, b.width, b.height]; }));
  expect(geometry[0]).toEqual(geometry[1]);
  expect(geometry[0].slice(2)).toEqual(dimensions.map((dimension) => dimension * 2));
  await overlayViewport.focus();
  await page.keyboard.press("ArrowRight");
  await expect.poll(() => overlayViewport.evaluate((element) => element.scrollLeft)).toBeGreaterThan(0);
  await dialog(page).getByRole("radio", { name: "並べて表示" }).check();
  await page.getByLabel("比較画像 A", { exact: true }).selectOption("0");
  await expect(compareImages(page).first()).toHaveAttribute("src", await cards(page).first().getAttribute("src"));
  await expect(page.locator("[data-synapse-compare-caption]").first()).toContainText("Original");
  await dialog(page).getByLabel("表示倍率", { exact: true }).selectOption("fit");
  expect(await compareImages(page).first().evaluate((image) => getComputedStyle(image).objectFit)).toBe("contain");
  await page.keyboard.press("Escape");
  await expect(dialog(page)).not.toBeVisible();
  await expect(opener(page)).toBeFocused();
  await expect(page.locator("[data-synapse-compare-image][src]")).toHaveCount(0);
  await openComparison(page);
  await expect(dialog(page).getByLabel("表示倍率", { exact: true })).toHaveValue("fit");
  await page.getByRole("button", { name: "閉じる", exact: true }).click();
  expect(requests.length).toBe(initialRequests);
  expect(requests.every(([method]) => method === "GET")).toBe(true);
  expect(errors).toEqual([]);
  expect(await page.evaluate(() => window.cspViolations)).toEqual([]);
});

test("matching decoded images overlay at a shared origin with integer opacity", async ({ page, browser, app }) => {
  const requests = [];
  page.on("request", (request) => { if (request.url().includes("/api/")) requests.push(request.method()); });
  await visit(page, app, "transparent");
  const initialRequests = requests.length;
  await openComparison(page);
  const overlayMode = dialog(page).getByRole("radio", { name: "重ねて表示" });
  await overlayMode.focus();
  await page.keyboard.press("Space");
  await expect(page.locator("[data-synapse-compare-overlay]")).toBeVisible();
  await expect(page.getByLabel("比較画像 A", { exact: true })).toBeVisible();
  await page.getByLabel("比較画像 A", { exact: true }).selectOption("0");
  await expect(page.locator("[data-synapse-compare-overlay-caption]")).toContainText("Original を下");
  await dialog(page).getByLabel("表示倍率", { exact: true }).selectOption("1");
  const pixelPage = await browser.newPage();
  const slider = dialog(page).getByLabel(/画像 B の不透明度/);
  for (const [value, opacity] of [["0", "0"], ["37", "0.37"], ["50", "0.5"], ["100", "1"]]) {
    await slider.fill(value);
    await expect(slider).toHaveAttribute("aria-valuenow", value);
    await expect(page.locator("[data-synapse-compare-opacity-value]")).toHaveText(`${value}%`);
    await expect(page.locator("[data-synapse-compare-overlay-image-b]")).toHaveAttribute("opacity", opacity);
    // Sample the actual browser rendering, independently of the presentation attribute.
    const screenshot = await page.locator("[data-synapse-compare-overlay-canvas]").screenshot();
    const pixels = await pixelPage.evaluate(async (encoded) => {
      const image = new Image();
      image.src = `data:image/png;base64,${encoded}`;
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = image.width;
      canvas.height = image.height;
      const context = canvas.getContext("2d");
      context.drawImage(image, 0, 0);
      return [16, 48].map((x) => [...context.getImageData(x, 16, 1, 1).data]);
    }, screenshot.toString("base64"));
    expect(pixels[0]).toEqual([255, 0, 0, 255]); // Transparent B preserves red A.
    for (const [channel, expected] of [[0, 255 * (1 - Number(opacity))], [1, 0], [2, 255 * Number(opacity)]]) {
      expect(Math.abs(pixels[1][channel] - expected)).toBeLessThanOrEqual(1);
    }
  }
  await pixelPage.close();
  await slider.focus();
  await page.keyboard.press("Home");
  await expect(slider).toHaveValue("0");
  await page.keyboard.press("ArrowRight");
  await expect(slider).toHaveValue("1");
  await expect(page.locator("[data-synapse-compare-overlay-caption]")).toContainText("位置合わせ・差分解析は行いません");
  await dialog(page).getByLabel("表示倍率", { exact: true }).selectOption("2");
  await expect(page.locator("[data-synapse-compare-overlay-canvas]")).toHaveAttribute("width", "128");
  const layers = await page.locator("[data-synapse-compare-overlay-canvas] image").evaluateAll((images) => images.map((image) => { const b = image.getBoundingClientRect(); return [b.x, b.y, b.width, b.height]; }));
  expect(layers[0]).toEqual(layers[1]);
  expect(layers[0].slice(2)).toEqual([128, 64]);
  await page.keyboard.press("Escape");
  await expect(opener(page)).toBeFocused();
  await openComparison(page);
  await expect(slider).toHaveValue("50");
  await expect(slider).toHaveAttribute("aria-valuenow", "50");
  expect(requests.length).toBe(initialRequests);
  expect(requests.every((method) => method === "GET")).toBe(true);
});

test("different decoded dimensions keep the side-by-side comparison available", async ({ page, app }) => {
  await visit(page, app, "mismatch");
  await openComparison(page);
  await expect(dialog(page).getByRole("radio", { name: "重ねて表示" })).toBeDisabled();
  await expect(page.locator("[data-synapse-compare-overlay-unavailable]")).toBeVisible();
  await expect(page.locator("[data-synapse-compare-pane]").first()).toBeVisible();
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
  await dialog(page).getByRole("radio", { name: "重ねて表示" }).check();
  await expect(dialog(page).getByLabel(/画像 B の不透明度/)).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
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
  await dialog(page).getByLabel("表示倍率", { exact: true }).selectOption("2");
  await page.keyboard.press("Escape");
  await page.getByLabel("理由（任意）").fill("Need another visual inspection.");
  page.once("dialog", (confirmation) => confirmation.accept());
  await page.getByRole("button", { name: "保留", exact: true }).click();
  await expect(page.getByRole("heading", { name: "タイムライン", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "保留", exact: true })).toHaveCount(0);
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
  await dialog(page).getByRole("radio", { name: "重ねて表示" }).check();
  const oldUrls = await cards(page).evaluateAll((images) => images.map((image) => image.src));
  await page.evaluate(() => window.dispatchEvent(new PageTransitionEvent("pagehide", { persisted: true })));
  await expect(dialog(page)).not.toBeVisible();
  await expect(page.locator("[data-synapse-compare-image][src]")).toHaveCount(0);
  await expect(page.locator("img[data-synapse-image][src]")).toHaveCount(0);
  await expect(page.locator("[data-synapse-compare-overlay-canvas] image[href]")).toHaveCount(0);
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
    await expect(page.getByRole("heading", { name: "タイムライン", exact: true })).toBeVisible();
  } finally {
    await context.close();
  }
});
