#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const mode = process.argv[2];
if (
  !new Set(["--validate", "--check", "--apply"]).has(mode) ||
  process.argv.length !== 3
) {
  console.error(
    "usage: node scripts/manage_github_security.mjs --validate|--check|--apply",
  );
  process.exit(2);
}

const root = process.cwd();
const baselinePath = path.join(
  root,
  "deploy",
  "github",
  "security-baseline.json",
);
const baseline = JSON.parse(fs.readFileSync(baselinePath, "utf8"));
const fixedRepository = "howlrs/synapsegit";

if (
  baseline.schema_version !== 1 ||
  baseline.repository !== fixedRepository ||
  baseline.api_version !== "2022-11-28"
) {
  console.error(
    "github_security_error: the GitHub security baseline identity or schema is invalid",
  );
  process.exit(1);
}

function api(endpoint, { method = "GET", body, notFound } = {}) {
  const args = [
    "api",
    "--header",
    "Accept: application/vnd.github+json",
    "--header",
    `X-GitHub-Api-Version: ${baseline.api_version}`,
  ];
  if (method !== "GET") {
    args.push("--method", method);
  }
  if (body !== undefined) {
    args.push("--input", "-");
  }
  args.push(endpoint);
  const result = spawnSync("gh", args, {
    cwd: root,
    encoding: "utf8",
    input: body === undefined ? undefined : `${JSON.stringify(body)}\n`,
    maxBuffer: 4 * 1024 * 1024,
    timeout: 30_000,
  });
  if (result.error) {
    throw new Error(`could not execute gh: ${result.error.message}`);
  }
  if (result.status !== 0) {
    const diagnostic = (result.stderr || result.stdout || "unknown gh error").trim();
    if (
      notFound !== undefined &&
      /(?:HTTP\s+404|"status"\s*:\s*"?404"?)/i.test(diagnostic)
    ) {
      return notFound;
    }
    throw new Error(`gh api ${method} ${endpoint} failed: ${diagnostic}`);
  }
  const output = result.stdout.trim();
  return output ? JSON.parse(output) : null;
}

function fail(label, expected, actual) {
  throw new Error(
    `${label} drifted: expected ${JSON.stringify(expected)}, got ${JSON.stringify(actual)}`,
  );
}

function assertSubset(actual, expected, label) {
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || JSON.stringify(actual) !== JSON.stringify(expected)) {
      fail(label, expected, actual);
    }
    return;
  }
  if (expected !== null && typeof expected === "object") {
    if (actual === null || typeof actual !== "object" || Array.isArray(actual)) {
      fail(label, expected, actual);
    }
    for (const [key, value] of Object.entries(expected)) {
      assertSubset(actual[key], value, `${label}.${key}`);
    }
    return;
  }
  if (actual !== expected) {
    fail(label, expected, actual);
  }
}

function repositoryEndpoint(suffix = "") {
  return `repos/${baseline.repository}${suffix}`;
}

function validateBaseline() {
  const requiredSettings = [
    "allow_squash_merge",
    "allow_merge_commit",
    "allow_rebase_merge",
    "delete_branch_on_merge",
  ];
  for (const name of requiredSettings) {
    if (typeof baseline.repository_settings?.[name] !== "boolean") {
      throw new Error(`repository setting is not boolean: ${name}`);
    }
  }
  for (const name of [
    "secret_scanning",
    "secret_scanning_push_protection",
  ]) {
    if (baseline.security_and_analysis?.[name] !== "enabled") {
      throw new Error(`security feature is not enabled in the baseline: ${name}`);
    }
  }
  for (const name of [
    "vulnerability_alerts",
    "automated_security_fixes",
    "private_vulnerability_reporting",
  ]) {
    if (baseline[name] !== true) {
      throw new Error(`repository security feature is not enabled: ${name}`);
    }
  }
  if (!Array.isArray(baseline.rulesets) || baseline.rulesets.length === 0) {
    throw new Error("the baseline must contain repository rulesets");
  }
  const names = new Set();
  for (const ruleset of baseline.rulesets) {
    if (!ruleset.name || names.has(ruleset.name)) {
      throw new Error(`baseline ruleset name is absent or duplicated: ${ruleset.name}`);
    }
    names.add(ruleset.name);
    if (
      !new Set(["branch", "tag"]).has(ruleset.target) ||
      ruleset.enforcement !== "active" ||
      !Array.isArray(ruleset.bypass_actors) ||
      ruleset.bypass_actors.length !== 0 ||
      !Array.isArray(ruleset.conditions?.ref_name?.include) ||
      !Array.isArray(ruleset.conditions?.ref_name?.exclude) ||
      !Array.isArray(ruleset.rules) ||
      ruleset.rules.length === 0
    ) {
      throw new Error(`baseline ruleset is malformed: ${ruleset.name}`);
    }
    const types = ruleset.rules.map((rule) => rule.type);
    if (types.some((type) => !type) || new Set(types).size !== types.length) {
      throw new Error(`baseline ruleset has absent or duplicate rule types: ${ruleset.name}`);
    }
  }
}

function currentRulesets() {
  const summaries = api(
    repositoryEndpoint("/rulesets?includes_parents=false&per_page=100"),
  );
  if (!Array.isArray(summaries)) {
    throw new Error("GitHub returned an invalid repository ruleset list");
  }
  const expectedNames = new Set(baseline.rulesets.map((ruleset) => ruleset.name));
  const unexpected = summaries.filter((ruleset) => !expectedNames.has(ruleset.name));
  if (unexpected.length > 0) {
    throw new Error(
      `refusing unexpected repository rulesets: ${unexpected
        .map((ruleset) => ruleset.name)
        .join(", ")}`,
    );
  }
  for (const expected of baseline.rulesets) {
    const matches = summaries.filter((ruleset) => ruleset.name === expected.name);
    if (matches.length > 1) {
      throw new Error(`ruleset name is duplicated: ${expected.name}`);
    }
  }
  return summaries;
}

function applyBaseline() {
  const summaries = currentRulesets();
  api(repositoryEndpoint(), {
    method: "PATCH",
    body: baseline.repository_settings,
  });
  api(repositoryEndpoint("/vulnerability-alerts"), { method: "PUT" });
  api(repositoryEndpoint("/automated-security-fixes"), { method: "PUT" });
  api(repositoryEndpoint("/private-vulnerability-reporting"), { method: "PUT" });
  api(repositoryEndpoint(), {
    method: "PATCH",
    body: {
      security_and_analysis: Object.fromEntries(
        Object.entries(baseline.security_and_analysis)
          .map(([name, status]) => [name, { status }]),
      ),
    },
  });

  for (const expected of baseline.rulesets) {
    const existing = summaries.find((ruleset) => ruleset.name === expected.name);
    api(
      existing
        ? repositoryEndpoint(`/rulesets/${existing.id}`)
        : repositoryEndpoint("/rulesets"),
      {
        method: existing ? "PUT" : "POST",
        body: expected,
      },
    );
  }
}

function checkBaseline() {
  const repository = api(repositoryEndpoint());
  if (repository.visibility !== "public" || repository.default_branch !== "main") {
    fail(
      "repository identity",
      { visibility: "public", default_branch: "main" },
      {
        visibility: repository.visibility,
        default_branch: repository.default_branch,
      },
    );
  }
  assertSubset(repository, baseline.repository_settings, "repository settings");
  for (const [name, expectedStatus] of Object.entries(
    baseline.security_and_analysis,
  )) {
    const actualStatus = repository.security_and_analysis?.[name]?.status;
    if (actualStatus !== expectedStatus) {
      fail(`security_and_analysis.${name}`, expectedStatus, actualStatus);
    }
  }

  const alertsEnabled = api(repositoryEndpoint("/vulnerability-alerts"), {
    notFound: false,
  });
  if (alertsEnabled === false) {
    fail("vulnerability alerts", baseline.vulnerability_alerts, false);
  }
  const fixes = api(repositoryEndpoint("/automated-security-fixes"));
  if (fixes?.enabled !== baseline.automated_security_fixes) {
    fail("automated security fixes", baseline.automated_security_fixes, fixes?.enabled);
  }
  const privateReporting = api(repositoryEndpoint("/private-vulnerability-reporting"));
  if (privateReporting?.enabled !== baseline.private_vulnerability_reporting) {
    fail(
      "private vulnerability reporting",
      baseline.private_vulnerability_reporting,
      privateReporting?.enabled,
    );
  }

  const summaries = currentRulesets();
  if (summaries.length !== baseline.rulesets.length) {
    fail(
      "repository ruleset count",
      baseline.rulesets.length,
      summaries.length,
    );
  }
  for (const expected of baseline.rulesets) {
    const summary = summaries.find((ruleset) => ruleset.name === expected.name);
    if (!summary) {
      throw new Error(`required ruleset is absent: ${expected.name}`);
    }
    const actual = api(repositoryEndpoint(`/rulesets/${summary.id}`));
    assertSubset(actual, {
      name: expected.name,
      target: expected.target,
      enforcement: expected.enforcement,
      bypass_actors: expected.bypass_actors,
      conditions: expected.conditions,
    }, `ruleset ${expected.name}`);
    const expectedTypes = expected.rules.map((rule) => rule.type).sort();
    const actualTypes = actual.rules.map((rule) => rule.type).sort();
    if (JSON.stringify(actualTypes) !== JSON.stringify(expectedTypes)) {
      fail(`ruleset ${expected.name} rule types`, expectedTypes, actualTypes);
    }
    for (const expectedRule of expected.rules) {
      const actualRule = actual.rules.find((rule) => rule.type === expectedRule.type);
      assertSubset(
        actualRule,
        expectedRule,
        `ruleset ${expected.name}.${expectedRule.type}`,
      );
    }
  }

  console.log(
    `github_security_ok: repository=${baseline.repository} rulesets=${baseline.rulesets.length}`,
  );
}

try {
  validateBaseline();
  if (mode === "--validate") {
    console.log(
      `github_security_baseline_ok: repository=${baseline.repository} rulesets=${baseline.rulesets.length}`,
    );
  } else {
    if (mode === "--apply") {
      applyBaseline();
    }
    checkBaseline();
  }
} catch (error) {
  console.error(`github_security_error: ${error.message}`);
  process.exitCode = 1;
}
