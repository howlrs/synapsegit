import { spawnSync } from "node:child_process";

export function assertRevisionHistory(currentContract, currentRegistry, baseContract, baseRegistry) {
  if (baseRegistry) {
    if (!Array.isArray(baseRegistry.revisions)) throw new Error("invalid base revision registry");
    for (const [index, revision] of baseRegistry.revisions.entries()) {
      const current = currentRegistry.revisions[index];
      if (current?.version !== revision.version || current?.sha256 !== revision.sha256) {
        throw new Error("published contract revisions are immutable; append a new version instead of replacing or removing " + revision.version);
      }
    }
  }
  // Also protects the transition from the old, unregistered 0.4.0-draft contract.
  if (currentContract.info?.version === baseContract.info?.version) {
    const normalize = (value) => Array.isArray(value) ? value.map(normalize) : value !== null && typeof value === "object"
      ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, normalize(value[key])])) : value;
    if (JSON.stringify(normalize(currentContract)) !== JSON.stringify(normalize(baseContract))) {
      throw new Error("contract content changed without a new info.version relative to the base revision");
    }
  }
}

export function readRevisionBase(reference) {
  if (!/^[a-f0-9]{40}$/.test(reference)) throw new Error("LOCAL_API_BASE_REF must be a full Git commit SHA");
  const read = (relative, optional = false) => {
    const listing = spawnSync("git", ["ls-tree", "--name-only", reference, "--", relative], { encoding: "utf8" });
    if (listing.status !== 0) throw new Error("cannot inspect contract base: " + (listing.stderr || listing.error?.message));
    if (!listing.stdout.trim() && optional) return undefined;
    const result = spawnSync("git", ["show", `${reference}:${relative}`], { encoding: "utf8" });
    if (result.status !== 0) throw new Error("cannot read contract base: " + (result.stderr || result.error?.message));
    return JSON.parse(result.stdout);
  };
  return {
    contract: read("api/local/v1/openapi.json"),
    registry: read("api/local/v1/revisions.json", true),
  };
}
