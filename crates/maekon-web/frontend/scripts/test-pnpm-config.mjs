import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");

function fail(message) {
  console.error(message);
  process.exit(1);
}

const packageJson = JSON.parse(
  readFileSync(resolve(rootDir, "package.json"), "utf8"),
);

if (packageJson.pnpm && Object.keys(packageJson.pnpm).length > 0) {
  fail("pnpm settings must live in pnpm-workspace.yaml for pnpm 11.");
}

const configResult = spawnSync(
  "pnpm",
  ["config", "get", "overrides", "--json"],
  {
    cwd: rootDir,
    encoding: "utf8",
  },
);

if (configResult.status !== 0) {
  fail(
    configResult.stderr?.trim() ||
      configResult.error?.message ||
      "Failed to read pnpm overrides.",
  );
}

if (configResult.stderr.includes("package.json is no longer read by pnpm")) {
  fail(configResult.stderr.trim());
}

const rawOverrides = configResult.stdout.trim();

if (!rawOverrides) {
  fail("pnpm overrides are not configured for this project.");
}

let overrides;
try {
  overrides = JSON.parse(rawOverrides);
} catch (error) {
  fail(`pnpm overrides are not valid JSON: ${error.message}`);
}

if (overrides["serialize-javascript"] !== "7.0.5") {
  fail("serialize-javascript must be overridden to 7.0.5.");
}

const allowBuildsResult = spawnSync(
  "pnpm",
  ["config", "get", "allowBuilds", "--json"],
  {
    cwd: rootDir,
    encoding: "utf8",
  },
);

if (allowBuildsResult.status !== 0) {
  fail(
    allowBuildsResult.stderr?.trim() ||
      allowBuildsResult.error?.message ||
      "Failed to read pnpm allowBuilds.",
  );
}

let allowBuilds;
try {
  allowBuilds = JSON.parse(allowBuildsResult.stdout.trim());
} catch (error) {
  fail(`pnpm allowBuilds are not valid JSON: ${error.message}`);
}

const expectedBuildApprovals = {
  edgedriver: false,
  esbuild: true,
  geckodriver: false,
};

for (const [packageName, expectedApproval] of Object.entries(
  expectedBuildApprovals,
)) {
  if (allowBuilds[packageName] !== expectedApproval) {
    fail(`${packageName} must be set to ${expectedApproval} in allowBuilds.`);
  }
}
