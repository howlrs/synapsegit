#!/usr/bin/env node

import fs from "node:fs";
import { spawnSync } from "node:child_process";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const markdownFiles = [];

function collect(relativePath) {
  const absolutePath = path.join(root, relativePath);
  if (!fs.existsSync(absolutePath)) {
    return;
  }
  const stat = fs.statSync(absolutePath);
  if (stat.isDirectory()) {
    for (const entry of fs.readdirSync(absolutePath, { withFileTypes: true })) {
      if (entry.name === ".git" || entry.name === "target" || entry.name === "node_modules") {
        continue;
      }
      collect(path.join(relativePath, entry.name));
    }
    return;
  }
  if (relativePath.endsWith(".md")) {
    markdownFiles.push(relativePath);
  }
}

for (const entry of [
  "README.md",
  "README.ja.md",
  "CHANGELOG.md",
  "CONTRIBUTING.md",
  "SECURITY.md",
  "SUPPORT.md",
  ".github",
  "deploy",
  "docs",
  "spec",
]) {
  collect(entry);
}
markdownFiles.sort();

const failures = [];
let localLinks = 0;
let mermaidBlocks = 0;
const trackedMarkdown = spawnSync("git", ["ls-files", "--", "*.md"], {
  cwd: root,
  encoding: "utf8",
});
const bashMarkdownFiles = trackedMarkdown.status === 0
  ? trackedMarkdown.stdout.split(/\r?\n/).filter(Boolean)
  : markdownFiles;

function githubAnchors(markdown) {
  const anchors = new Set();
  const occurrences = new Map();
  for (const line of markdown.split(/\r?\n/)) {
    const match = /^(#{1,6})\s+(.+?)\s*#*\s*$/.exec(line);
    if (!match) {
      continue;
    }
    let heading = match[2]
      .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
      .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
      .replace(/<[^>]+>/g, "")
      .replace(/[`*_~]/g, "")
      .trim()
      .toLowerCase();
    let slug = heading
      .replace(/[^\p{L}\p{N}\s_-]/gu, "")
      .replace(/\s/g, "-");
    if (!slug) {
      continue;
    }
    const count = occurrences.get(slug) ?? 0;
    occurrences.set(slug, count + 1);
    if (count > 0) {
      slug = slug + "-" + count;
    }
    anchors.add(slug);
  }
  return anchors;
}

const anchorCache = new Map();

function anchorsFor(relativePath) {
  if (!anchorCache.has(relativePath)) {
    anchorCache.set(
      relativePath,
      githubAnchors(fs.readFileSync(path.join(root, relativePath), "utf8")),
    );
  }
  return anchorCache.get(relativePath);
}

function checkMermaid(relativePath, markdown) {
  const lines = markdown.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    if (!/^```mermaid\s*$/.test(lines[index])) {
      continue;
    }
    mermaidBlocks += 1;
    const start = index + 1;
    let end = start;
    while (end < lines.length && !/^```\s*$/.test(lines[end])) {
      end += 1;
    }
    if (end === lines.length) {
      failures.push(relativePath + ":" + (index + 1) + " unclosed Mermaid fence");
      return;
    }
    const first = lines
      .slice(start, end)
      .map((line) => line.trim())
      .find(Boolean);
    if (!first || !/^[A-Za-z][A-Za-z0-9_-]*(?:\s|$)/.test(first)) {
      failures.push(relativePath + ":" + (index + 1) + " empty or invalid Mermaid block");
    }
    index = end;
  }
}

function checkBash(relativePath, markdown) {
  const lines = markdown.split(/\r?\n/);
  for (let index = 0; index < lines.length; index += 1) {
    if (!/^```bash\s*$/i.test(lines[index])) {
      continue;
    }
    const start = index + 1;
    let end = start;
    while (end < lines.length && !/^```\s*$/.test(lines[end])) {
      end += 1;
    }
    if (end === lines.length) {
      failures.push(relativePath + ":" + (index + 1) + " unclosed bash fence");
      return;
    }
    const result = spawnSync("bash", ["-n"], {
      input: lines.slice(start, end).join("\n"),
      encoding: "utf8",
    });
    if (result.status !== 0) {
      const detail = (result.stderr || "bash syntax error").trim().split("\n")[0];
      failures.push(relativePath + ":" + (index + 1) + " invalid bash block: " + detail);
    }
    index = end;
  }
}

for (const relativePath of markdownFiles) {
  const markdown = fs.readFileSync(path.join(root, relativePath), "utf8");
  checkMermaid(relativePath, markdown);

  const linkPattern = /!?\[[^\]]*]\(([^)]+)\)/g;
  for (const match of markdown.matchAll(linkPattern)) {
    let target = match[1].trim();
    const titleStart = target.search(/\s+["']/);
    if (titleStart >= 0) {
      target = target.slice(0, titleStart);
    }
    if (target.startsWith("<") && target.endsWith(">")) {
      target = target.slice(1, -1);
    }
    if (
      !target ||
      /^(?:https?:|mailto:|data:)/i.test(target)
    ) {
      continue;
    }

    localLinks += 1;
    const [rawPath, rawFragment = ""] = target.split("#", 2);
    let decodedPath;
    let decodedFragment;
    try {
      decodedPath = decodeURIComponent(rawPath);
      decodedFragment = decodeURIComponent(rawFragment).toLowerCase();
    } catch {
      failures.push(relativePath + ": invalid percent encoding in " + target);
      continue;
    }

    let linkedPath = rawPath
      ? path.normalize(path.join(path.dirname(relativePath), decodedPath))
      : relativePath;
    const absoluteTarget = path.join(root, linkedPath);

    if (!fs.existsSync(absoluteTarget)) {
      failures.push(relativePath + ": missing link target " + target);
      continue;
    }

    const stat = fs.statSync(absoluteTarget);
    if (stat.isDirectory()) {
      const indexPath = path.join(linkedPath, "README.md");
      if (!fs.existsSync(path.join(root, indexPath))) {
        failures.push(relativePath + ": linked directory has no README.md: " + target);
      }
      linkedPath = indexPath;
    }

    if (decodedFragment && linkedPath.endsWith(".md")) {
      const fragment = decodedFragment.replace(/^user-content-/, "");
      if (!anchorsFor(linkedPath).has(fragment)) {
        failures.push(relativePath + ": missing heading #" + fragment + " in " + linkedPath);
      }
    }
  }
}

for (const relativePath of bashMarkdownFiles) {
  checkBash(relativePath, fs.readFileSync(path.join(root, relativePath), "utf8"));
}

// Release metadata that a person reads before trusting an archive. The release
// notes double as the bundled archive README.md, so each release after v0.13.1
// must keep the verification, backup, and license essentials in that file.
const RELEASE_NOTES_REQUIREMENTS_SINCE = [0, 13, 2];
const RELEASE_NOTES_REQUIRED = [
  [/Do not extract or install an unverified archive\./u, "the rule not to extract or install an unverified archive"],
  [/Export important repositories with the old binary before updating/u, "the export-before-update backup precaution"],
  [/not an OSI-approved open-source license/u, "that the license is not OSI-approved"],
  [/requires separate written permission/u, "the separate written permission requirement"],
  [/THIRD_PARTY_NOTICES\.md/u, "the THIRD_PARTY_NOTICES.md reference"],
];
const RELEASE_NOTES_FORBIDDEN = [
  [/After the tag workflow publishes/u, "pre-publication wording that stays in the published Release body"],
];

function cliVersion() {
  const manifest = fs.readFileSync(path.join(root, "crates", "synapse-cli", "Cargo.toml"), "utf8");
  const match = /^version\s*=\s*"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-rc\.([1-9][0-9]*))?"\s*$/mu.exec(manifest);
  return match ? [...match.slice(1, 4).map(Number), match[4] === undefined ? null : Number(match[4])] : null;
}

function checkReleaseMetadata() {
  const version = cliVersion();
  if (!version) {
    failures.push("crates/synapse-cli/Cargo.toml: missing X.Y.Z or X.Y.Z-rc.N package version");
    return;
  }
  const [major, minor, patch, candidate] = version;
  const security = fs.readFileSync(path.join(root, "SECURITY.md"), "utf8");
  const supported = [...security.matchAll(/^\|\s*Latest v(\d+)\.(\d+)\.x prerelease\s*\|/gmu)];
  if (supported.length !== 1) {
    failures.push("SECURITY.md: Supported versions must have exactly one 'Latest vX.Y.x prerelease' row");
  } else if (Number(supported[0][1]) !== major || Number(supported[0][2]) !== minor) {
    failures.push(
      `SECURITY.md: Supported versions names Latest v${supported[0][1]}.${supported[0][2]}.x prerelease, ` +
        `but synapse-cli is ${major}.${minor}.${patch}; use Latest v${major}.${minor}.x prerelease`,
    );
  }

  const since = RELEASE_NOTES_REQUIREMENTS_SINCE;
  const current = version.slice(0, 3).findIndex((value, index) => value !== since[index]);
  if (current >= 0 && version[current] < since[current]) return;
  const tag = `v${major}.${minor}.${patch}${candidate === null ? "" : `-rc.${candidate}`}`;
  const notesPath = path.join("docs", "releases", `${tag}.md`);
  if (!fs.existsSync(path.join(root, notesPath))) {
    failures.push(`${notesPath}: release notes for the current synapse-cli version are missing`);
    return;
  }
  const notes = fs.readFileSync(path.join(root, notesPath), "utf8");
  // Compare prose with line wrapping collapsed so authors may reflow paragraphs.
  const prose = notes.replace(/\s+/gu, " ");
  for (const [pattern, label] of RELEASE_NOTES_REQUIRED) {
    if (!pattern.test(prose)) failures.push(`${notesPath}: missing ${label}`);
  }
  const installGuide = `https://github.com/howlrs/synapsegit/blob/${tag}/docs/install.md`;
  if (!notes.includes(installGuide)) failures.push(`${notesPath}: missing the tag-pinned installation guide link ${installGuide}`);
  for (const [pattern, label] of RELEASE_NOTES_FORBIDDEN) {
    if (pattern.test(prose)) failures.push(`${notesPath}: contains ${label}`);
  }
}

checkReleaseMetadata();

if (failures.length > 0) {
  for (const failure of failures) {
    console.error("docs_error: " + failure);
  }
  process.exitCode = 1;
} else {
  console.log(
    "docs_ok: files=" +
      markdownFiles.length +
      " local_links=" +
      localLinks +
      " mermaid_blocks=" +
      mermaidBlocks,
  );
}
