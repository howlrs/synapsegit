#!/usr/bin/env node
// Waits until the CI workflow has succeeded on `main` for one exact commit.
// The release workflow runs this before building so that a tag cannot publish
// a commit whose main CI (including the browser suite) failed, was cancelled,
// or never ran.
import assert from "node:assert/strict";
import { setTimeout as sleep } from "node:timers/promises";

const WORKFLOW_PATH = ".github/workflows/ci.yml";
const ACCEPTED_EVENTS = new Set(["push", "workflow_dispatch"]);

export function mainCiRuns(runs, sha) {
  return runs.filter((run) => (
    run.head_sha === sha && run.head_branch === "main" && ACCEPTED_EVENTS.has(run.event)
      && (run.path ?? "").split("@")[0] === WORKFLOW_PATH
  ));
}

export function evaluateMainCi(runs, sha) {
  const relevant = mainCiRuns(runs, sha);
  const describe = (run) => `run ${run.id} (${run.event}, attempt ${run.run_attempt ?? 1}): ${run.status}${run.conclusion ? `/${run.conclusion}` : ""} ${run.html_url ?? ""}`.trim();
  const succeeded = relevant.find((run) => run.status === "completed" && run.conclusion === "success");
  if (succeeded) return { state: "success", detail: describe(succeeded) };
  const active = relevant.filter((run) => run.status !== "completed");
  if (active.length > 0) return { state: "pending", detail: active.map(describe).join("; ") };
  if (relevant.length > 0) return { state: "failed", detail: relevant.map(describe).join("; ") };
  return { state: "missing", detail: `no ${WORKFLOW_PATH} run on main for ${sha}` };
}

function parseArguments(args) {
  const options = { timeoutSeconds: 4800, missingTimeoutSeconds: 600, intervalSeconds: 30 };
  for (let index = 0; index < args.length; index += 2) {
    const [flag, value] = [args[index], args[index + 1]];
    if (value === undefined) throw new Error(`missing value for ${flag}`);
    if (flag === "--repo") options.repo = value;
    else if (flag === "--sha") options.sha = value;
    else if (flag === "--timeout-seconds") options.timeoutSeconds = Number(value);
    else if (flag === "--missing-timeout-seconds") options.missingTimeoutSeconds = Number(value);
    else if (flag === "--interval-seconds") options.intervalSeconds = Number(value);
    else throw new Error(`unknown option ${flag}`);
  }
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(options.repo ?? "")) throw new Error("--repo must be owner/name");
  if (!/^[0-9a-f]{40}$/.test(options.sha ?? "")) throw new Error("--sha must be a full commit SHA");
  for (const key of ["timeoutSeconds", "missingTimeoutSeconds", "intervalSeconds"]) {
    if (!Number.isFinite(options[key]) || options[key] <= 0) throw new Error(`${key} must be a positive number`);
  }
  return options;
}

async function listRuns({ repo, sha }) {
  const token = process.env.GITHUB_TOKEN || process.env.GH_TOKEN;
  if (!token) throw new Error("GITHUB_TOKEN or GH_TOKEN is required");
  const api = process.env.GITHUB_API_URL || "https://api.github.com";
  const url = `${api}/repos/${repo}/actions/workflows/ci.yml/runs?branch=main&head_sha=${sha}&per_page=100`;
  const response = await fetch(url, {
    headers: { Accept: "application/vnd.github+json", Authorization: `Bearer ${token}`, "X-GitHub-Api-Version": "2022-11-28" },
  });
  if (!response.ok) throw new Error(`GitHub API ${response.status} for ${url}: ${(await response.text()).slice(0, 300)}`);
  const body = await response.json();
  if (!Array.isArray(body.workflow_runs)) throw new Error("GitHub API response has no workflow_runs array");
  return body.workflow_runs;
}

async function wait(options) {
  const started = Date.now();
  for (;;) {
    const elapsed = (Date.now() - started) / 1000;
    const { state, detail } = evaluateMainCi(await listRuns(options), options.sha);
    if (state === "success") {
      console.log(`main CI succeeded for ${options.sha}: ${detail}`);
      return;
    }
    if (state === "failed") {
      throw new Error(`main CI did not succeed for ${options.sha}: ${detail}. Re-run that main CI run to success, then re-run this release workflow.`);
    }
    if (state === "missing" && elapsed >= options.missingTimeoutSeconds) {
      throw new Error(`${detail} after ${options.missingTimeoutSeconds}s. Tag a merge commit that has its own main CI push run.`);
    }
    if (elapsed >= options.timeoutSeconds) throw new Error(`main CI is still ${state} for ${options.sha} after ${options.timeoutSeconds}s: ${detail}`);
    console.log(`waiting for main CI (${state}): ${detail}`);
    await sleep(options.intervalSeconds * 1000);
  }
}

function selfTest() {
  const sha = "a".repeat(40);
  const run = (fields) => ({ id: 1, head_sha: sha, head_branch: "main", event: "push", path: WORKFLOW_PATH, status: "completed", conclusion: "success", ...fields });
  assert.equal(evaluateMainCi([run({})], sha).state, "success");
  assert.equal(evaluateMainCi([run({ path: `${WORKFLOW_PATH}@refs/heads/main` })], sha).state, "success");
  assert.equal(evaluateMainCi([run({ event: "workflow_dispatch" })], sha).state, "success");
  assert.equal(evaluateMainCi([run({ conclusion: "cancelled" }), run({ id: 2 })], sha).state, "success");
  assert.equal(evaluateMainCi([run({ conclusion: "failure" }), run({ id: 2, status: "in_progress", conclusion: null })], sha).state, "pending");
  assert.equal(evaluateMainCi([run({ status: "queued", conclusion: null })], sha).state, "pending");
  assert.equal(evaluateMainCi([run({ conclusion: "cancelled" })], sha).state, "failed");
  assert.equal(evaluateMainCi([run({ conclusion: "failure" })], sha).state, "failed");
  assert.equal(evaluateMainCi([run({ conclusion: "skipped" })], sha).state, "failed");
  for (const other of [
    { head_sha: "b".repeat(40) },
    { head_branch: "feature" },
    { event: "pull_request" },
    { path: ".github/workflows/release.yml" },
  ]) {
    assert.equal(evaluateMainCi([run(other)], sha).state, "missing", JSON.stringify(other));
  }
  assert.equal(evaluateMainCi([], sha).state, "missing");
  assert.throws(() => parseArguments(["--repo", "howlrs/synapsegit", "--sha", "abc"]), /full commit SHA/);
  assert.throws(() => parseArguments(["--repo", "bad", "--sha", sha]), /owner\/name/);
  assert.throws(() => parseArguments(["--repo", "howlrs/synapsegit", "--sha", sha, "--interval-seconds", "0"]), /positive/);
  assert.equal(parseArguments(["--repo", "howlrs/synapsegit", "--sha", sha]).timeoutSeconds, 4800);
  console.log("main CI gate self-test passed");
}

const args = process.argv.slice(2);
try {
  if (args.length === 1 && args[0] === "--self-test") selfTest();
  else await wait(parseArguments(args));
} catch (error) {
  console.error(`main_ci_gate_error: ${error.message}`);
  process.exitCode = 1;
}
