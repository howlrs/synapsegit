import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const scriptDirectory = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
let root;
for (let index = 0; index < args.length; index += 1) {
  if (args[index] === "--bundle-root") { root = args[index + 1]; index += 1; }
  else throw new Error(`unknown argument: ${args[index]} (expected --bundle-root PATH)`);
}
if (!root) throw new Error("--bundle-root PATH is required");
const result = spawnSync(path.join(scriptDirectory, "node_modules/.bin/playwright"), ["test", "publication-accessibility.spec.mjs"], {
  cwd: scriptDirectory,
  env: { ...process.env, SYNAPSEGIT_PUBLICATION_BUNDLE_ROOT: path.resolve(root) },
  stdio: "inherit",
});
process.exit(result.status ?? 1);
