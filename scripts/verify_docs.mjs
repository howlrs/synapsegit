#!/usr/bin/env node

import fs from "node:fs";
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

for (const entry of ["README.md", "CONTRIBUTING.md", "deploy", "docs", "spec"]) {
  collect(entry);
}
markdownFiles.sort();

const failures = [];
let localLinks = 0;
let mermaidBlocks = 0;

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
