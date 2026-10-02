#!/usr/bin/env node
// The single source of truth for release-tag parsing and GitHub publication
// policy. Keep the validation used before packaging and the publication mode
// derived from the same parsed tag.
import { appendFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const RELEASE_TAG = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.([1-9][0-9]*))?$/u;

export function parseReleaseVersion(tag) {
  const match = RELEASE_TAG.exec(tag);
  if (!match) return null;

  const [, major, minor, patch, releaseCandidate] = match;
  const isPrerelease = major === "0" || releaseCandidate !== undefined;
  const publication = releaseCandidate !== undefined
    ? "release candidate"
    : major === "0"
      ? "Stage 0 preview"
      : "release";

  return {
    tag,
    version: tag.slice(1),
    major: Number(major),
    minor: Number(minor),
    patch: Number(patch),
    isPrerelease,
    isLatest: !isPrerelease,
    publication,
    title: publication === "release" ? `SynapseGit ${tag}` : `SynapseGit ${tag} — ${publication}`,
  };
}

function usage() {
  console.error("usage: node scripts/release-version.mjs <vX.Y.Z[-rc.N]> [--github-output <path>]");
}

const isMain = process.argv[1] !== undefined
  && fileURLToPath(import.meta.url) === path.resolve(process.argv[1]);

if (isMain) {
  const [tag, ...args] = process.argv.slice(2);
  const parsed = parseReleaseVersion(tag);
  if (!parsed) {
    usage();
    process.exitCode = 1;
  } else if (args.length === 0) {
    console.log(parsed.version);
  } else if (args.length === 2 && args[0] === "--github-output") {
    const output = [
      `title=${parsed.title}`,
      `prerelease=${parsed.isPrerelease}`,
      `latest=${parsed.isLatest}`,
    ].join("\n");
    await appendFile(args[1], `${output}\n`);
  } else {
    usage();
    process.exitCode = 1;
  }
}
