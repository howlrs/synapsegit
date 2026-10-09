import { isolatedTest as test, expect } from "./fixtures.mjs";

function tiff(gps) {
  const bytes = Buffer.alloc(40); bytes.set([73, 73, 42, 0, 8, 0, 0, 0, 1, 0]);
  bytes.writeUInt16LE(gps ? 0x8825 : 0x0100, 10); bytes.writeUInt16LE(4, 12);
  bytes.writeUInt32LE(1, 14); bytes.writeUInt32LE(32, 18); return bytes;
}
function jpeg(gps) { const exif = tiff(gps); return Buffer.concat([Buffer.from([255,216,255,225,0,exif.length + 8]), Buffer.from("Exif\0\0"), exif, Buffer.from([255,217])]); }
function pngGps() { const exif = tiff(true); return Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]), Buffer.from([0,0,0,exif.length,101,88,73,102]), exif, Buffer.alloc(4), Buffer.from([0,0,0,0,73,69,78,68]), Buffer.alloc(4)]); }
const file = (name, buffer) => ({ name, mimeType: "application/octet-stream", buffer });

test("location warnings appear before import and remain scoped to each selected file", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/reviews/import`);
  const fields = page.locator("[data-creator-file]");
  const original = fields.nth(0), current = fields.nth(1), output = fields.nth(2);
  await original.locator('input[type="file"]').setInputFiles(file("gps.jpg", jpeg(true)));
  await expect(original.locator("[data-creator-metadata-warning]")).toBeVisible();
  await expect(original.locator("[data-creator-metadata-warning]")).toContainText("位置情報");
  await expect(current.locator("[data-creator-metadata-warning]")).toBeHidden();
  await current.locator('input[type="file"]').setInputFiles(file("gps.png", pngGps()));
  await expect(current.locator("[data-creator-metadata-warning]")).toBeVisible();
  await output.locator('input[type="file"]').setInputFiles(file("opaque.gif", Buffer.from("GIF89a")));
  await expect(output.locator("[data-creator-metadata-warning]")).toBeVisible();
  await expect(output.locator("[data-creator-metadata-warning]")).toContainText("確認しきれ");
  await original.locator('input[type="file"]').setInputFiles(file("benign.jpg", Buffer.from([255,216,255,217])));
  await expect(original.locator("[data-creator-metadata-warning]")).toBeHidden();
  await expect(current.locator("[data-creator-metadata-warning]")).toBeVisible();
  await current.locator("[data-creator-file-clear]").click();
  await expect(current.locator("[data-creator-metadata-warning]")).toBeHidden();
  await expect(output.locator("[data-creator-metadata-warning]")).toBeVisible();
  await expect(page).toHaveURL(/\/projects\/reviews\/import$/u);
});
