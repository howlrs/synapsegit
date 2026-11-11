import { test, expect } from "./fixtures.mjs";

test("every page shows the project icon in the header and as the tab icon", async ({ page, app }) => {
  const violations = [];
  page.on("console", (message) => {
    if (message.type() === "error") violations.push(message.text());
  });
  await page.goto(`${app.origin}/`);

  // The mark is decorative: the link's accessible name is the product name alone.
  const brand = page.locator("a.brand");
  await expect(brand).toHaveAccessibleName("SynapseGit Local");
  const mark = brand.locator("img.brand__mark");
  await expect(mark).toHaveAttribute("alt", "");
  await expect.poll(() => mark.evaluate((image) => image.complete && image.naturalWidth > 0)).toBe(true);
  const box = await mark.boundingBox();
  expect(Math.round(box.width)).toBe(Math.round(box.height));

  const href = await page.locator('link[rel="icon"]').getAttribute("href");
  expect(href).toBe("/assets/icon.svg");
  const response = await page.request.get(`${app.origin}${href}`);
  expect(response.status()).toBe(200);
  expect(response.headers()["content-type"]).toBe("image/svg+xml");
  expect(await response.text()).toContain("<title>SynapseGit</title>");

  // Narrow screens keep the mark beside the name without overflowing the header.
  await page.setViewportSize({ width: 320, height: 640 });
  await expect(mark).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect(violations).toEqual([]);
});
