#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const root = process.cwd();
const diagrams = [
  "CONTRIBUTING.md",
  "docs/runtime_architecture.md",
  "docs/localhost_application_architecture.md",
];
const failures = [];

function fail(message) {
  failures.push(message);
}

function cargoMetadata() {
  const result = spawnSync(
    "cargo",
    ["metadata", "--no-deps", "--format-version", "1"],
    { cwd: root, encoding: "utf8" },
  );
  if (result.status !== 0) {
    throw new Error((result.stderr || result.stdout || "cargo metadata failed").trim());
  }
  return JSON.parse(result.stdout);
}

function mermaidBlocks(markdown) {
  return [...markdown.matchAll(/^```mermaid\s*\n([\s\S]*?)^```\s*$/gm)].map(
    (match) => match[1],
  );
}

function workspaceNodes(diagram) {
  const nodes = new Map();
  const nodePattern = /\b([A-Za-z][A-Za-z0-9_-]*)\s*\[(?:"([^"]*)"|([^\]]*))\]/g;
  for (const match of diagram.matchAll(nodePattern)) {
    const crate = /\b(synapse-[a-z0-9-]+)\b/.exec(match[2] ?? match[3]);
    if (crate) {
      nodes.set(match[1], crate[1]);
    }
  }
  return nodes;
}

function solidEdges(diagram, nodes) {
  const edges = new Set();
  const graph = diagram.replace(
    /\b([A-Za-z][A-Za-z0-9_-]*)\s*\[(?:"[^"]*"|[^\]]*)\]/g,
    "$1",
  );
  const edgePattern = /\b([A-Za-z][A-Za-z0-9_-]*)\s*-->(?:\|[^|\n]*\|\s*)?\s*([A-Za-z][A-Za-z0-9_-]*)\b/g;
  for (const match of graph.matchAll(edgePattern)) {
    const from = nodes.get(match[1]);
    const to = nodes.get(match[2]);
    if (from && to) {
      edges.add(from + " -> " + to);
    }
  }
  return edges;
}

let metadata;
try {
  metadata = cargoMetadata();
} catch (error) {
  console.error("workspace_diagram_error: " + error.message);
  process.exit(1);
}

const workspace = new Set(metadata.packages.map((pkg) => pkg.name));
const dependencies = new Map(
  metadata.packages.map((pkg) => [
    pkg.name,
    new Set(
      pkg.dependencies
        .filter((dependency) => dependency.kind !== "dev" && workspace.has(dependency.name))
        .map((dependency) => dependency.name),
    ),
  ]),
);

for (const relativePath of diagrams) {
  const absolutePath = path.join(root, relativePath);
  if (!fs.existsSync(absolutePath)) {
    fail(relativePath + ": missing diagram document");
    continue;
  }
  const blocks = mermaidBlocks(fs.readFileSync(absolutePath, "utf8"));
  if (blocks.length === 0) {
    fail(relativePath + ": no Mermaid diagram");
    continue;
  }

  const nodes = workspaceNodes(blocks[0]);
  if (nodes.size === 0) {
    fail(relativePath + ": no workspace crate nodes in the first Mermaid diagram");
    continue;
  }
  const crates = new Set(nodes.values());
  const edges = solidEdges(blocks[0], nodes);
  for (const from of crates) {
    for (const to of dependencies.get(from) ?? []) {
      if (crates.has(to) && !edges.has(from + " -> " + to)) {
        fail(relativePath + ": missing direct dependency " + from + " -> " + to);
      }
    }
  }
  for (const edge of edges) {
    const [from, to] = edge.split(" -> ");
    if (!(dependencies.get(from) ?? new Set()).has(to)) {
      fail(relativePath + ": solid edge is not a direct dependency " + edge);
    }
  }
}

if (failures.length > 0) {
  for (const failure of failures) {
    console.error("workspace_diagram_error: " + failure);
  }
  process.exitCode = 1;
} else {
  console.log("workspace_diagram_ok: diagrams=" + diagrams.length);
}
