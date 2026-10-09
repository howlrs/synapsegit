import { isolatedTest as test, expect } from "./fixtures.mjs";

function tiff(gps) {
  const bytes = Buffer.alloc(40); bytes.set([73, 73, 42, 0, 8, 0, 0, 0, 1, 0]);
  bytes.writeUInt16LE(gps ? 0x8825 : 0x0100, 10); bytes.writeUInt16LE(4, 12);
  bytes.writeUInt32LE(1, 14); bytes.writeUInt32LE(32, 18); return bytes;
}
function jpeg(gps) { const exif = tiff(gps); return Buffer.concat([Buffer.from([255,216,255,225,0,exif.length + 8]), Buffer.from("Exif\0\0"), exif, Buffer.from([255,217])]); }
function crc(type, data) { let value=0xffffffff;for(const byte of Buffer.concat([Buffer.from(type),data])){value^=byte;for(let i=0;i<8;i++)value=value&1?(value>>>1)^0xedb88320:value>>>1;}return (~value)>>>0; }
function chunk(type, data) { const out=Buffer.alloc(12+data.length);out.writeUInt32BE(data.length);out.write(type,4);data.copy(out,8);out.writeUInt32BE(crc(type,data),8+data.length);return out; }
function pngGps() { return Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]),chunk("eXIf",tiff(true)),chunk("IEND",Buffer.alloc(0))]); }
function pngCompressedXmp() { return Buffer.concat([Buffer.from([137,80,78,71,13,10,26,10]),chunk("iTXt",Buffer.concat([Buffer.from("XML:com.adobe.xmp\0"),Buffer.from([1,0,0,0])])),chunk("IEND",Buffer.alloc(0))]); }
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

test("a pending metadata preflight prevents a fast proposal POST", async ({ page, app }) => {
  await page.goto(`${app.origin}/projects/reviews/import`);
  await page.locator('[name="session"]').fill("delayed-metadata"); await page.locator('[name="creator_name"]').fill("Creator"); await page.locator('[name="subject_label"]').fill("Subject");
  await page.evaluate(() => {
    window.releaseMetadataReads = [];
    const slice = File.prototype.slice;
    File.prototype.slice = function (...args) {
      const blob = slice.apply(this, args);
      if (args[1] !== 256 * 1024) return blob;
      return { arrayBuffer: () => new Promise(resolve => {
        window.releaseMetadataReads.push(async () => resolve(await blob.arrayBuffer()));
      }) };
    };
  });
  let posts=0; page.on("request", request=>{if(request.method()==="POST"&&request.url().includes("creator-sessions"))posts++;});
  for (const name of ["original_image","current_image","ai_output"]) await page.locator(`[name="${name}"]`).setInputFiles(file(`${name}.jpg`,jpeg(true)));
  await page.getByRole("button",{name:"提案を作成"}).click();
  expect(posts).toBe(0);
  await expect(page.locator('[name="ai_output"]')).toHaveAttribute("aria-invalid", "true");
  await page.evaluate(async () => { await Promise.all(window.releaseMetadataReads.map(release => release())); });
  await expect(page.locator('[name="ai_output"]')).toHaveAttribute("aria-invalid", "false");
  await page.getByRole("button",{name:"提案を作成"}).click();
  await page.waitForURL("**/creator-sessions/delayed-metadata"); expect(posts).toBe(1);
});
