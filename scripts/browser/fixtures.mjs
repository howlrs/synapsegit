import { test as base, expect } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { mkdtemp, mkdir, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const browserProfile = process.env.SYNAPSEGIT_BROWSER_PROFILE === undefined
  ? "debug"
  : process.env.SYNAPSEGIT_BROWSER_PROFILE;
if (browserProfile !== "debug" && browserProfile !== "release") {
  throw new Error(
    `Invalid SYNAPSEGIT_BROWSER_PROFILE ${JSON.stringify(browserProfile)}; expected "debug" or "release"`,
  );
}
const binaries = path.resolve(root, process.env.CARGO_TARGET_DIR || "target", browserProfile);
const assets = path.join(root, "docs/tutorial/assets");
export const original = path.join(assets, "mural-original.png");
export const current = path.join(assets, "mural-current.png");
export const output = path.join(assets, "mural-ai-proposal.png");
// Japanese decision button labels, keyed by the English disposition name used in test titles.
export const decisionButton = Object.freeze({ Adopt: "採用", Reject: "不採用", Defer: "保留" });
const mismatchedOutput = path.join(root, "docs/assets/synapse-local/image-comparison.png");

async function appFixture({ archives = false, inbox = false }, use) {
    const directory = await mkdtemp(path.join(tmpdir(), "synapse-browser-"));
    let server;
    let origin;
    try {
      const opaque = path.join(directory, "opaque.txt");
      const broken = path.join(directory, "broken.png");
      const transparent = path.join(directory, "transparent.png");
      const red = path.join(directory, "red.png");
      const archiveRoot = path.join(directory, "archives");
      const inboxRoot = path.join(directory, "inbox");
      if (archives) await mkdir(archiveRoot);
      if (inbox) await mkdir(inboxRoot);
      await writeFile(opaque, "opaque attachment, not an image");
      await writeFile(broken, Buffer.from([137, 80, 78, 71, 13, 10, 26, 10, 0]));
      // 64x32 RGBA: solid red A and blue B whose left half is fully transparent.
      await writeFile(red, Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAEAAAAAgCAYAAACinX6EAAAAWElEQVR4nO3QMREAMBDDsPAn/YWhoR60+7zb7mfTAVoDdIDWAB2gNUAHaA3QAVoDdIDWAB2gNUAHaA3QAVoDdIDWAB2gNUAHaA3QAVoDdIDWAB2gNUAHaA8g8vDiJft7OwAAAABJRU5ErkJggg==", "base64"));
      await writeFile(transparent, Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAEAAAAAgCAYAAACinX6EAAAAQUlEQVR4nO3QMQ0AAAwDoPo33alY+nBggCTNWLcEzAkQIECAAAECBAgQIECAAAECBAgQIECAAAECBAgQIECAgG8Hmy/0pnlzDEEAAAAASUVORK5CYII=", "base64"));
      const cli = (...args) => execFileSync(path.join(binaries, "synapse"), args, { encoding: "utf8" });
      for (const [key, files] of [
        ["complete", [original, current, output]],
        ["mixed", [original, opaque, output]],
        ["broken", [broken, opaque, broken]],
        ["transparent", [red, red, transparent]],
        ["mismatch", [original, current, mismatchedOutput]],
      ]) {
        cli("creator-run", path.join(directory, key), "sample", ...files,
          "--subject", "Comparison browser fixture", "--creator", "Browser tester",
          "--decision", "defer", "--rationale", "Review fixture");
      }
      for (const key of ["pending", "reviews", "interrupted", ...(archives ? ["restore"] : [])]) cli("init", path.join(directory, key));
      const projectPath = (key) => path.join(directory, key);
      const refs = (key) => cli("refs", projectPath(key));
      const addHistory = (key, session = "existing-history") => cli(
        "creator-run", projectPath(key), session, original, current, output,
        "--subject", "Archive refusal fixture", "--creator", "Browser tester",
        "--decision", "defer", "--rationale", "Synthetic fixture history",
      );
      const serverArgs = ["--port", "0",
        ...(archives ? ["--archive-root", archiveRoot] : []),
        ...(inbox ? ["--import-root", `pending=${inboxRoot}`] : []),
        ...["complete", "mixed", "broken", "transparent", "mismatch", "pending", "reviews", "interrupted", ...(archives ? ["restore"] : [])].flatMap((key) => ["--project", `${key}=${projectPath(key)}`]),
      ];
      const start = async () => new Promise((resolve, reject) => {
        server = spawn(path.join(binaries, "synapse-local"), serverArgs, { stdio: ["ignore", "ignore", "pipe"] });
        let log = "";
        const timeout = setTimeout(() => reject(new Error("localhost test server did not start")), 15_000);
        server.once("error", (error) => { clearTimeout(timeout); reject(error); });
        server.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`localhost exited ${code}: ${log}`)); });
        server.stderr.on("data", (chunk) => {
          log += chunk.toString();
          const match = log.match(/http:\/\/127\.0\.0\.1:\d+/u);
          if (match) { clearTimeout(timeout); origin = match[0]; resolve(origin); }
        });
      });
      const restart = async () => {
        if (server && server.exitCode === null) {
          const stopped = new Promise((resolve) => server.once("exit", resolve));
          server.kill("SIGTERM");
          await stopped;
        }
        await start();
      };
      await start();
      await use({ get origin() { return origin; }, refs, addHistory, restart, inboxRoot, cli, projectPath });
    } finally {
      if (server && server.exitCode === null) {
        const stopped = new Promise((resolve) => server.once("exit", resolve));
        server.kill("SIGTERM");
        await stopped;
      }
      await rm(directory, { recursive: true, force: true });
    }
}

// Wait until every selected Creator upload has finished its observable
// metadata preflight. This deliberately observes validity rather than using
// a timing delay, so normal submissions cannot race the required preflight.
export async function waitForCreatorUploadReady(page) {
  await expect.poll(() => page.locator('[data-creator-file] input[type="file"]').evaluateAll((inputs) =>
    inputs.some((input) => input.files?.length)
      && inputs.filter((input) => input.files?.length).every((input) => input.getAttribute("aria-invalid") === "false"),
  )).toBe(true);
}

export const test = base.extend({ app: [({}, use) => appFixture({ archives: false }, use), { scope: "worker" }] });
// New multi-session workflows use their own repository so unrelated tests do not
// change their bounded verification cost or leave records behind after failure.
export const isolatedTest = base.extend({ app: [({}, use) => appFixture({ archives: false }, use), { scope: "test" }] });
// Archive workflows mutate server-owned archive state, so they always receive
// their own temporary archive root and project repositories.
export const archiveTest = base.extend({ app: [({}, use) => appFixture({ archives: true }, use), { scope: "test" }] });
export const inboxTest = base.extend({ app: [({}, use) => appFixture({ inbox: true }, use), { scope: "test" }] });

export { expect };
