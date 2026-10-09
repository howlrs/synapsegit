import { isolatedTest as test, inboxTest, expect, original, current, output, waitForCreatorUploadReady } from "./fixtures.mjs";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { deflateSync } from "node:zlib";

function tiff(gps) {
  const bytes = Buffer.alloc(40); bytes.set([73, 73, 42, 0, 8, 0, 0, 0, 1, 0]);
  bytes.writeUInt16LE(gps ? 0x8825 : 0x0100, 10); bytes.writeUInt16LE(4, 12);
  bytes.writeUInt32LE(1, 14); bytes.writeUInt32LE(32, 18); return bytes;
}
function jpeg(gps) { const exif = tiff(gps); return Buffer.concat([Buffer.from([255,216,255,225,0,exif.length + 8]), Buffer.from("Exif\0\0"), exif, Buffer.from([255,218,0,8,1,1,0,0,63,0,0,255,217])]); }
function crc(type, data) { let value=0xffffffff;for(const byte of Buffer.concat([Buffer.from(type),data])){value^=byte;for(let i=0;i<8;i++)value=value&1?(value>>>1)^0xedb88320:value>>>1;}return (~value)>>>0; }
function chunk(type, data) { const out=Buffer.alloc(12+data.length);out.writeUInt32BE(data.length);out.write(type,4);data.copy(out,8);out.writeUInt32BE(crc(type,data),8+data.length);return out; }
function png(...metadata) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(1, 0); ihdr.writeUInt32BE(1, 4); ihdr.set([8, 2, 0, 0, 0], 8);
  return Buffer.concat([
    Buffer.from([137,80,78,71,13,10,26,10]), chunk("IHDR", ihdr), ...metadata,
    chunk("IDAT", deflateSync(Buffer.from([0, 0, 0, 0]))), chunk("IEND", Buffer.alloc(0)),
  ]);
}
function pngGps() { return png(chunk("eXIf", tiff(true))); }
function pngCompressedXmp() { return png(chunk("iTXt", Buffer.concat([Buffer.from("XML:com.adobe.xmp\0"), Buffer.from([1,0,0,0])]))); }
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
  await expect(current.locator("[data-creator-metadata-warning]")).toContainText("位置情報が含まれる可能性");
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

test("compressed XMP warns as unchecked and derived upload keeps a warning target", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/reviews/import`);
  await page.locator('[name="original_image"]').setInputFiles(file("xmp.png", pngCompressedXmp()));
  await expect(page.locator("[data-creator-file]").first().locator("[data-creator-metadata-warning]")).toContainText("確認しきれ");
  await page.goto(`${app.origin}/projects/complete/creator-sessions/sample/derive`);
  await page.locator('[name="ai_output"]').setInputFiles(file("xmp.png", pngCompressedXmp()));
  await expect(page.locator("[data-creator-metadata-warning]")).toContainText("確認しきれ");
});

test("tutorial PNG uploads finish metadata preflight without location warnings", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/reviews/import`);
  for (const [name, image] of [["original_image", original], ["current_image", current], ["ai_output", output]]) {
    await page.locator(`[name="${name}"]`).setInputFiles(image);
  }
  await waitForCreatorUploadReady(page);
  for (const field of await page.locator("[data-creator-file]").all()) {
    await expect(field.locator("[data-creator-metadata-warning]")).toBeHidden();
  }
});

test("a pending metadata preflight prevents a fast proposal POST", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/reviews/import`);
  await page.locator('[name="session"]').fill("delayed-metadata"); await page.locator('[name="creator_name"]').fill("Creator"); await page.locator('[name="subject_label"]').fill("Subject");
  await page.evaluate(() => {
    window.releaseMetadataReads = [];
    const slice = File.prototype.slice;
    File.prototype.slice = function (...args) {
      const blob = slice.apply(this, args);
      if (args.length !== 2 || args[0] !== 0 || args[1] !== Math.min(this.size, 64 * 1024)) return blob;
      return { arrayBuffer: () => new Promise(resolve => {
        window.releaseMetadataReads.push(async () => resolve(await blob.arrayBuffer()));
      }) };
    };
  });
  let posts=0; page.on("request", request=>{if(request.method()==="POST"&&request.url().includes("creator-sessions"))posts++;});
  for (const name of ["original_image","current_image","ai_output"]) await page.locator(`[name="${name}"]`).setInputFiles(file(`${name}.jpg`,jpeg(true)));
  await expect.poll(() => page.evaluate(() => window.releaseMetadataReads.length)).toBe(3);
  await page.getByRole("button",{name:"提案を作成"}).click();
  expect(posts).toBe(0);
  await expect(page.locator('[name="ai_output"]')).toHaveAttribute("aria-invalid", "true");
  await page.evaluate(async () => { await Promise.all(window.releaseMetadataReads.map(release => release())); });
  await expect(page.locator('[name="ai_output"]')).toHaveAttribute("aria-invalid", "false");
  await page.getByRole("button",{name:"提案を作成"}).click();
  await page.waitForURL("**/creator-sessions/delayed-metadata"); expect(posts).toBe(1);
});

inboxTest("an Inbox candidate shows staged location warnings beside its images before a proposal exists", async ({ page, app }) => {
  const candidate = path.join(app.inboxRoot, "located-photo");
  await mkdir(candidate);
  const clean = Buffer.from([255, 216, 255, 217]);
  const files = [["original", jpeg(true)], ["current", clean], ["ai-output", clean]];
  for (const [name, bytes] of files) await writeFile(path.join(candidate, name), bytes);
  await writeFile(path.join(candidate, "manifest.json"), JSON.stringify({ version: "synapsegit-import-inbox-v1", original: { name: "original", size: files[0][1].length }, current: { name: "current", size: clean.length }, ai_output: { name: "ai-output", size: clean.length }, metadata: { subject_label: "Located subject", creator_name: "Script creator" } }));
  let proposals = 0;
  page.on("request", request => { if (request.method() === "POST" && request.url().endsWith("/creator-sessions")) proposals++; });
  await page.goto(`${app.origin}/projects/pending/import`);
  await page.getByRole("button", { name: "確認する" }).click();
  const figures = page.locator("[data-import-inbox-images] figure");
  await expect(figures).toHaveCount(3);
  await expect(figures.nth(0).locator("[data-creator-metadata-warning]")).toContainText("位置情報が含まれる可能性");
  await expect(page.locator("[data-import-inbox-images] [data-creator-metadata-warning]")).toHaveCount(1);
  expect(proposals).toBe(0);
});
