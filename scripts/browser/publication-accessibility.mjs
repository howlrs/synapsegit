import { access, readFile, stat } from "node:fs/promises";
import { createHash } from "node:crypto";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const repositoryRoot = fileURLToPath(new URL("../../", import.meta.url));
export const defaultBundleRoot = path.join(repositoryRoot, "docs/evaluation/publication-comprehension/v1/bundles");

// Approved exception for one immutable artifact, not a blanket expected failure.
// See docs/publication_accessibility_gate.md for evidence and removal conditions.
const knownReflowDefect = Object.freeze({
  id: "publication-v1-incomplete-disclosure-wrap",
  token: "projection_fingerprint_unavailable",
  sha256: "b64054b99407398ec75b7aeeb64993fb2e8e58dd0f529445a7c1d0d9f5187906",
});

export async function publicationReflowWaiver(bundle) {
  const frozenIndex = path.join(defaultBundleRoot, "incomplete-only", "index.html");
  if (path.resolve(bundle.index) !== frozenIndex) return undefined;
  const digest = createHash("sha256").update(await readFile(frozenIndex)).digest("hex");
  if (digest !== knownReflowDefect.sha256) {
    throw new Error("known reflow waiver artifact changed; review or remove the waiver");
  }
  return knownReflowDefect;
}

export function bundleRoot(value = process.env.SYNAPSEGIT_PUBLICATION_BUNDLE_ROOT) {
  return path.resolve(value || defaultBundleRoot);
}

export async function publicationBundles(root = bundleRoot()) {
  const metadata = await stat(root);
  if (!metadata.isDirectory()) throw new Error(`publication bundle root is not a directory: ${root}`);
  const standaloneIndex = path.join(root, "index.html");
  try {
    await access(standaloneIndex);
    return [{ name: path.basename(root), index: standaloneIndex, url: pathToFileURL(standaloneIndex).href }];
  } catch { /* A paired corpus root has index.html in each named bundle. */ }
  const names = ["complete", "incomplete-only"];
  await Promise.all(names.map(async name => {
    const index = path.join(root, name, "index.html");
    try { await access(index); } catch { throw new Error(`publication bundle is missing index.html: ${index}`); }
  }));
  return names.map(name => ({ name, index: path.join(root, name, "index.html"), url: pathToFileURL(path.join(root, name, "index.html")).href }));
}

export function headingLevels(headings) {
  return headings.map(heading => Number(heading.tagName.slice(1)));
}

export function hasSkippedHeadingLevel(levels) {
  return levels.some((level, index) => index > 0 && level > levels[index - 1] + 1);
}
