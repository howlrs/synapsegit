#!/usr/bin/env node

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const failures = [];

function read(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!fs.existsSync(absolutePath)) {
    failures.push(`${relativePath}: missing required file`);
    return "";
  }
  return fs.readFileSync(absolutePath, "utf8");
}

function requireText(relativePath, content, expected, description) {
  if (!content.includes(expected)) {
    failures.push(`${relativePath}: missing ${description}: ${JSON.stringify(expected)}`);
  }
}

function requireLine(relativePath, content, expected, description) {
  const lines = content.split(/\r?\n/).map((line) => line.trim());
  if (!lines.includes(expected)) {
    failures.push(`${relativePath}: missing ${description}: ${JSON.stringify(expected)}`);
  }
}

const license = read("LICENSE");
requireLine(
  "LICENSE",
  license,
  "SynapseGit Source-Available License 1.0",
  "license title",
);
requireLine(
  "LICENSE",
  license,
  "Copyright (c) 2026 howlrs and K-Terashima. All rights reserved.",
  "rights-holder notice",
);
requireText("LICENSE", license, "including v0.1.0", "application to v0.1.0");

const thirdPartyNotices = read("THIRD_PARTY_NOTICES.md");
requireText(
  "THIRD_PARTY_NOTICES.md",
  thirdPartyNotices,
  "# Third-party notices",
  "third-party notice heading",
);

const workspaceManifest = read("Cargo.toml");
requireLine(
  "Cargo.toml",
  workspaceManifest,
  'license-file = "LICENSE"',
  "workspace license-file metadata",
);

const cratesDirectory = path.join(root, "crates");
const crateManifests = fs.existsSync(cratesDirectory)
  ? fs
      .readdirSync(cratesDirectory, { withFileTypes: true })
      .filter((entry) => entry.isDirectory())
      .map((entry) => path.join("crates", entry.name, "Cargo.toml"))
      .filter((relativePath) => fs.existsSync(path.join(root, relativePath)))
      .sort()
  : [];

if (crateManifests.length !== 12) {
  failures.push(
    `crates: expected 12 crate manifests, found ${crateManifests.length}`,
  );
}
for (const relativePath of crateManifests) {
  requireLine(
    relativePath,
    read(relativePath),
    "license-file.workspace = true",
    "inherited workspace license-file metadata",
  );
}

for (const relativePath of ["README.md", "README.ja.md"]) {
  const readme = read(relativePath);
  if (!/\]\(\.\/LICENSE(?:\s+["'][^"']*["'])?\)/.test(readme)) {
    failures.push(`${relativePath}: missing Markdown link to ./LICENSE`);
  }
}

const packageScript = read("scripts/package_release.sh");
requireText(
  "scripts/package_release.sh",
  packageScript,
  "if [[ ! -s LICENSE ]]; then",
  "non-empty LICENSE preflight",
);
requireLine(
  "scripts/package_release.sh",
  packageScript,
  'install -m 0644 LICENSE "$bundle_directory/LICENSE"',
  "release archive LICENSE installation",
);
requireText(
  "scripts/package_release.sh",
  packageScript,
  "if [[ ! -s THIRD_PARTY_NOTICES.md ]]; then",
  "non-empty third-party notices preflight",
);
requireLine(
  "scripts/package_release.sh",
  packageScript,
  'install -m 0644 THIRD_PARTY_NOTICES.md "$bundle_directory/THIRD_PARTY_NOTICES.md"',
  "release archive third-party notice installation",
);
if (/if\s+\[\[\s+-[ef]\s+LICENSE\s+\]\]/.test(packageScript)) {
  failures.push(
    "scripts/package_release.sh: LICENSE installation must not be optional",
  );
}

if (failures.length > 0) {
  for (const failure of failures) {
    console.error(`license_verification_error: ${failure}`);
  }
  process.exit(1);
}

console.log(
  `verified license contract, ${crateManifests.length} crate manifests, README links, and release packaging`,
);
