import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";

const npmCli = process.env.npm_execpath;
assert.ok(npmCli, "npm_execpath must be available");

const result = spawnSync(
  process.execPath,
  [npmCli, "pack", "--dry-run", "--json", "--ignore-scripts"],
  { encoding: "utf8" },
);

assert.equal(result.status, 0, result.stderr || result.stdout);
const [manifest] = JSON.parse(result.stdout);
const files = new Set(manifest.files.map(({ path }) => path));

for (const required of [
  "dist/index.d.ts",
  "dist/index.js",
  "package.json",
  "schemas/events.schema.json",
  "schemas/openapi.yaml",
]) {
  assert.ok(files.has(required), `package is missing ${required}`);
}
