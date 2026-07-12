#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
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

for (const entry of ["README.md", "CONTRIBUTING.md", "docs", "spec"]) {
  collect(entry);
}

let combined = "# SynapseGit Mermaid render verification\n\n";
let count = 0;
for (const relativePath of markdownFiles.sort()) {
  const markdown = fs.readFileSync(path.join(root, relativePath), "utf8");
  const pattern = /^```mermaid\s*\n([\s\S]*?)^```\s*$/gm;
  for (const match of markdown.matchAll(pattern)) {
    count += 1;
    combined +=
      "## " +
      relativePath.replace(/[^\p{L}\p{N}._/-]/gu, "_") +
      " diagram " +
      count +
      "\n\n```mermaid\n" +
      match[1] +
      "```\n\n";
  }
}

if (count === 0) {
  console.error("mermaid_error: no Mermaid blocks found");
  process.exit(1);
}

const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "synapsegit-mermaid-"));
const input = path.join(temporary, "diagrams.md");
const output = path.join(temporary, "rendered.md");
fs.writeFileSync(input, combined);

const command = process.env.MERMAID_CLI || "npx";
const cliArguments = process.env.MERMAID_CLI
  ? ["-i", input, "-o", output, "--quiet"]
  : [
      "--yes",
      "@mermaid-js/mermaid-cli@11.16.0",
      "-i",
      input,
      "-o",
      output,
      "--quiet",
    ];

const result = spawnSync(command, cliArguments, {
  cwd: temporary,
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
});

if (result.status !== 0) {
  process.stderr.write(result.stdout || "");
  process.stderr.write(result.stderr || "");
  console.error("mermaid_error: render failed; input retained at " + input);
  process.exit(result.status ?? 1);
}

fs.rmSync(temporary, { recursive: true, force: true });
console.log("mermaid_ok: blocks=" + count);
