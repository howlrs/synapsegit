import { test as base, expect } from "@playwright/test";
import { spawn, execFileSync } from "node:child_process";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../../", import.meta.url));
const binaries = path.resolve(root, process.env.CARGO_TARGET_DIR || "target", "debug");
const assets = path.join(root, "docs/tutorial/assets");
export const original = path.join(assets, "mural-original.png");
export const current = path.join(assets, "mural-current.png");
export const output = path.join(assets, "mural-ai-proposal.png");

async function appFixture({}, use) {
    const directory = await mkdtemp(path.join(tmpdir(), "synapse-browser-"));
    let server;
    try {
      const opaque = path.join(directory, "opaque.txt");
      const broken = path.join(directory, "broken.png");
      await writeFile(opaque, "opaque attachment, not an image");
      await writeFile(broken, Buffer.from([137, 80, 78, 71, 13, 10, 26, 10, 0]));
      const cli = (...args) => execFileSync(path.join(binaries, "synapse"), args, { encoding: "utf8" });
      for (const [key, files] of [
        ["complete", [original, current, output]],
        ["mixed", [original, opaque, output]],
        ["broken", [broken, opaque, broken]],
      ]) {
        cli("creator-run", path.join(directory, key), "sample", ...files,
          "--subject", "Comparison browser fixture", "--creator", "Browser tester",
          "--decision", "defer", "--rationale", "Review fixture");
      }
      for (const key of ["pending", "reviews"]) cli("init", path.join(directory, key));
      server = spawn(path.join(binaries, "synapse-local"), ["--port", "0",
        ...["complete", "mixed", "broken", "pending", "reviews"].flatMap((key) => ["--project", `${key}=${path.join(directory, key)}`]),
      ], { stdio: ["ignore", "ignore", "pipe"] });
      const origin = await new Promise((resolve, reject) => {
        let log = "";
        const timeout = setTimeout(() => reject(new Error("localhost test server did not start")), 15_000);
        server.once("error", (error) => { clearTimeout(timeout); reject(error); });
        server.once("exit", (code) => { clearTimeout(timeout); reject(new Error(`localhost exited ${code}: ${log}`)); });
        server.stderr.on("data", (chunk) => {
          log += chunk.toString();
          const match = log.match(/http:\/\/127\.0\.0\.1:\d+/u);
          if (match) { clearTimeout(timeout); resolve(match[0]); }
        });
      });
      await use({ origin });
    } finally {
      if (server && server.exitCode === null) {
        const stopped = new Promise((resolve) => server.once("exit", resolve));
        server.kill("SIGTERM");
        await stopped;
      }
      await rm(directory, { recursive: true, force: true });
    }
}

export const test = base.extend({ app: [appFixture, { scope: "worker" }] });
// New multi-session workflows use their own repository so unrelated tests do not
// change their bounded verification cost or leave records behind after failure.
export const isolatedTest = base.extend({ app: [appFixture, { scope: "test" }] });

export { expect };
