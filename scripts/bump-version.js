import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

const packageJsonPath = path.join(repoRoot, "package.json");
const semverPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
const currentVersion = packageJson.version;

if (!currentVersion || !semverPattern.test(currentVersion)) {
  console.error("Invalid or missing version in package.json");
  process.exit(1);
}

const rl = readline.createInterface({
  input: process.stdin,
  output: process.stdout,
});

const prompt = `\nCurrent version: ${currentVersion}\nSelect bump type:\n1) patch\n2) minor\n3) major\n4) custom\nEnter choice [1]: `;

const ask = (question) => new Promise((resolve) => rl.question(question, resolve));

const bumpVersion = (version, type) => {
  const match = version.match(/^(\d+)\.(\d+)\.(\d+)/);
  if (!match) {
    return null;
  }

  let major = Number.parseInt(match[1], 10);
  let minor = Number.parseInt(match[2], 10);
  let patch = Number.parseInt(match[3], 10);

  if (type === "major") {
    major += 1;
    minor = 0;
    patch = 0;
  } else if (type === "minor") {
    minor += 1;
    patch = 0;
  } else {
    patch += 1;
  }

  return `${major}.${minor}.${patch}`;
};

const run = async () => {
  const choice = (await ask(prompt)).trim() || "1";
  let nextVersion = null;

  if (choice === "4" || /^custom$/i.test(choice)) {
    const custom = (await ask("Enter version: ")).trim();
    if (!semverPattern.test(custom)) {
      console.error("Invalid semver value");
      process.exit(1);
    }
    nextVersion = custom;
  } else if (choice === "3" || /^major$/i.test(choice)) {
    nextVersion = bumpVersion(currentVersion, "major");
  } else if (choice === "2" || /^minor$/i.test(choice)) {
    nextVersion = bumpVersion(currentVersion, "minor");
  } else if (choice === "1" || /^patch$/i.test(choice)) {
    nextVersion = bumpVersion(currentVersion, "patch");
  } else {
    console.error("Unknown selection");
    process.exit(1);
  }

  if (!nextVersion) {
    console.error("Could not compute next version");
    process.exit(1);
  }

  packageJson.version = nextVersion;
  fs.writeFileSync(packageJsonPath, `${JSON.stringify(packageJson, null, 2)}\n`, "utf8");

  const syncResult = spawnSync("node", ["scripts/sync-version.js"], {
    cwd: repoRoot,
    stdio: "inherit",
  });

  if (syncResult.status !== 0) {
    process.exit(syncResult.status ?? 1);
  }

  console.log(`Bumped version to ${nextVersion}`);
  rl.close();
};

run().catch((error) => {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
