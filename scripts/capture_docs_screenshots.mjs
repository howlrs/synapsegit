#!/usr/bin/env node
/**
 * Rebuild the documentation screenshots from the SHA-256-verified v1.0.0-rc.1
 * Linux release archive. This intentionally uses a temporary repository and the
 * localhost application: no page, image, or browser chrome is fabricated.
 *
 * Run from the repository root:
 *   node scripts/capture_docs_screenshots.mjs --release-archive /path/to/synapsegit-v1.0.0-rc.1-x86_64-unknown-linux-gnu.tar.gz
 *
 * Or set SYNAPSEGIT_DOCS_RELEASE_ARCHIVE to that archive path.
 *
 * The script needs the already-installed browser dependencies under
 * scripts/browser/node_modules.  It does not download anything.
 */
import { createRequire } from "node:module";
import { spawn, execFileSync } from "node:child_process";
import { access, mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const require = createRequire(path.join(root, "scripts/browser/package.json"));

const archiveArgument = process.argv.indexOf("--release-archive");
if (archiveArgument !== -1 && !process.argv[archiveArgument + 1]) {
  throw new Error("--release-archive requires a path");
}
const releaseArchive = archiveArgument === -1
  ? process.env.SYNAPSEGIT_DOCS_RELEASE_ARCHIVE
  : process.argv[archiveArgument + 1];
if (!releaseArchive) {
  throw new Error("Set SYNAPSEGIT_DOCS_RELEASE_ARCHIVE or pass --release-archive /path/to/synapsegit-v1.0.0-rc.1-x86_64-unknown-linux-gnu.tar.gz");
}
const expectedArchiveSha256 = "b8b90ef1b9d34490f04f230e0d54cf3c50f834737d473ca83a9d9db64bef7b93";
const releaseDirectory = "synapsegit-v1.0.0-rc.1-x86_64-unknown-linux-gnu";
const assets = path.join(root, "docs/tutorial/assets");
const documentationAssets = path.join(root, "docs/assets/synapse-local");
const inputs = Object.freeze({
  original: path.join(assets, "mural-original.png"),
  current: path.join(assets, "mural-current.png"),
  proposal: path.join(assets, "mural-ai-proposal.png"),
});
const outputFiles = Object.freeze({
  overview: path.join(documentationAssets, "overview-hero.png"),
  creatorSession: path.join(documentationAssets, "creator-session.png"),
  dashboard: path.join(documentationAssets, "project-dashboard.png"),
  decision: path.join(documentationAssets, "decision-review.png"),
  importPreview: path.join(documentationAssets, "import-preview.png"),
  comparison: path.join(documentationAssets, "image-comparison.png"),
  tutorial: path.join(assets, "tutorial-overview.png"),
  metadata: path.join(documentationAssets, "capture.json"),
});

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function pngDimensions(bytes, name) {
  const signature = "89504e470d0a1a0a";
  if (bytes.subarray(0, 8).toString("hex") !== signature || bytes.subarray(12, 16).toString("ascii") !== "IHDR") {
    throw new Error(`${name} is not a PNG with an IHDR header`);
  }
  return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
}

async function assertInputs() {
  for (const [role, file] of Object.entries(inputs)) {
    try {
      await access(file);
    } catch {
      throw new Error(`Missing ${role} tutorial image: ${file}`);
    }
  }
  const actual = sha256(await readFile(releaseArchive));
  if (actual !== expectedArchiveSha256) {
    throw new Error(`Release archive SHA-256 mismatch: expected ${expectedArchiveSha256}, got ${actual}`);
  }
}

function run(binary, args) {
  return execFileSync(binary, args, { encoding: "utf8" });
}

async function startServer(binary, project) {
  const server = spawn(binary, ["--port", "0", "--project", `mural=${project}`, "--label", "mural=Synthetic mural tutorial"], {
    stdio: ["ignore", "ignore", "pipe"],
  });
  let log = "";
  const origin = await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`synapse-local did not start: ${log}`)), 15_000);
    server.once("error", (error) => { clearTimeout(timeout); reject(error); });
    server.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`synapse-local exited ${code}: ${log}`)); });
    server.stderr.on("data", (chunk) => {
      log += chunk.toString();
      const match = log.match(/http:\/\/127\.0\.0\.1:\d+/u);
      if (match) { clearTimeout(timeout); resolve(match[0]); }
    });
  });
  return { origin, async stop() {
    if (server.exitCode !== null) return;
    const exited = new Promise((resolve) => server.once("exit", resolve));
    server.kill("SIGTERM");
    await exited;
  } };
}

async function capture(page, file, viewport) {
  await page.setViewportSize(viewport);
  await page.screenshot({ path: file });
}

async function waitForDecodedImages(page, selector) {
  await page.locator(selector).first().waitFor();
  await page.waitForFunction((imageSelector) => [...document.querySelectorAll(imageSelector)].every((image) => (
    image.getAttribute("aria-busy") !== "true" && image.complete && image.naturalWidth > 0
  )), selector);
}

async function main() {
  await assertInputs();
  const workspace = await mkdtemp(path.join(tmpdir(), "synapsegit-docs-capture-"));
  let server;
  let browser;
  try {
    run("tar", ["-xzf", releaseArchive, "-C", workspace]);
    const releaseRoot = path.join(workspace, releaseDirectory);
    const synapse = path.join(releaseRoot, "synapse");
    const local = path.join(releaseRoot, "synapse-local");
    const version = run(local, ["--version"]).trim();
    if (!version.includes("1.0.0-rc.1")) throw new Error(`Unexpected synapse-local version: ${version}`);
    if (process.argv.includes("--verify-runtime")) {
      process.stdout.write(`${version}\n`);
      return;
    }

    const repository = path.join(workspace, "mural");
    await mkdir(repository);
    run(synapse, ["creator-run", repository, "mural-treatment-01", inputs.original, inputs.current, inputs.proposal,
      "--subject", "Community Hall Coastal Mural", "--creator", "Synthetic tutorial studio",
      "--decision", "adopt", "--rationale", "Synthetic tutorial decision: retain the documented proposal for review."]);
    server = await startServer(local, repository);
    const { chromium } = require("playwright");
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ locale: "ja-JP", viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 1 });
    const page = await context.newPage();

    await page.goto(`${server.origin}/projects/mural`, { waitUntil: "networkidle" });
    await capture(page, outputFiles.overview, { width: 1440, height: 1000 });
    await capture(page, outputFiles.dashboard, { width: 1440, height: 1000 });
    await capture(page, outputFiles.tutorial, { width: 1440, height: 1000 });

    await page.getByRole("link", { name: "mural-treatment-01", exact: true }).click();
    await page.waitForLoadState("networkidle");
    await waitForDecodedImages(page, "img[data-synapse-image]");
    await page.locator("[data-synapse-comparison]").scrollIntoViewIfNeeded();
    await capture(page, outputFiles.creatorSession, { width: 1440, height: 1100 });
    await page.getByRole("button", { name: "画像を拡大して比較" }).click();
    await page.getByRole("dialog", { name: "画像を見比べる" }).waitFor();
    await page.getByRole("dialog", { name: "画像を見比べる" }).getByRole("radio", { name: "重ねて表示" }).check();
    await capture(page, outputFiles.comparison, { width: 1440, height: 1000 });
    await page.getByRole("button", { name: "閉じる", exact: true }).click();

    await page.goto(`${server.origin}/projects/mural/import`, { waitUntil: "networkidle" });
    await page.getByLabel("新しいセッション名", { exact: true }).fill("proposal-review");
    await page.getByLabel("作成者名", { exact: true }).fill("Synthetic tutorial studio");
    await page.getByLabel("対象名", { exact: true }).fill("Community hall mural conservation study");
    await page.getByLabel("Original画像", { exact: true }).setInputFiles(inputs.original);
    await page.getByLabel("Current画像", { exact: true }).setInputFiles(inputs.current);
    await page.getByLabel("AI output（外部で作成）", { exact: true }).setInputFiles(inputs.proposal);
    await waitForDecodedImages(page, "img[data-creator-preview]");
    await page.locator("[data-creator-file]").first().scrollIntoViewIfNeeded();
    await capture(page, outputFiles.importPreview, { width: 1440, height: 1200 });
    await page.getByRole("button", { name: "提案を作成" }).click();
    await page.waitForURL("**/creator-sessions/proposal-review");
    await waitForDecodedImages(page, "img[data-synapse-image]");
    await capture(page, outputFiles.decision, { width: 1440, height: 1100 });
    await context.close();

    const imageBytes = Object.fromEntries(await Promise.all(Object.entries({ ...inputs, ...outputFiles })
      .filter(([, file]) => file.endsWith(".png"))
      .map(async ([key, file]) => [key, await readFile(file)])));
    const imageHashes = Object.fromEntries(Object.entries(imageBytes).map(([key, bytes]) => [key, sha256(bytes)]));
    const imageDimensions = Object.fromEntries(Object.entries(imageBytes).map(([key, bytes]) => [key, pngDimensions(bytes, key)]));
    const originalDimensions = imageDimensions.original;
    const comparisonDimensions = imageDimensions.comparison;
    if (comparisonDimensions.width === originalDimensions.width && comparisonDimensions.height === originalDimensions.height) {
      throw new Error("image-comparison.png must retain different decoded dimensions for the browser mismatch fixture");
    }
    await writeFile(outputFiles.metadata, `${JSON.stringify({
      format: 1,
      runtime: { release: "v1.0.0-rc.1", archive_sha256: expectedArchiveSha256, synapse_local_version: version },
      capture: {
        date: new Date().toISOString().slice(0, 10),
        playwright: require("playwright/package.json").version,
        chromium: browser.version(),
        locale: "ja-JP",
        demonstration: "Synthetic tutorial; creator-run --decision adopt records a scripted example, not a participant decision or pilot result.",
        archive_checksum_source: "https://github.com/howlrs/synapsegit/releases/download/v1.0.0-rc.1/SHA256SUMS",
      },
      inputs: { synthetic: true, files: Object.fromEntries(Object.entries(inputs).map(([key, file]) => [key, path.relative(root, file)])), hashes: Object.fromEntries(Object.entries(inputs).map(([key]) => [key, imageHashes[key]])) },
      captures: Object.fromEntries(Object.entries(outputFiles).filter(([, file]) => file.endsWith(".png")).map(([key, file]) => [key, { file: path.relative(root, file), sha256: imageHashes[key], dimensions: imageDimensions[key] }])),
    }, null, 2)}\n`);
    process.stdout.write(`Captured ${Object.values(outputFiles).filter((file) => file.endsWith(".png")).length} screenshots with ${version}\n`);
  } finally {
    await browser?.close();
    await server?.stop();
    await rm(workspace, { recursive: true, force: true });
  }
}

await main();
