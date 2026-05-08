import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

const cargoLockPath = path.join(repoRoot, "src-tauri", "Cargo.lock");
const pnpmLockPath = path.join(repoRoot, "pnpm-lock.yaml");

const packagePairs = [
  ["tauri", "@tauri-apps/api"],
  ["tauri-plugin-dialog", "@tauri-apps/plugin-dialog"],
  ["tauri-plugin-fs", "@tauri-apps/plugin-fs"],
  ["tauri-plugin-opener", "@tauri-apps/plugin-opener"],
  ["tauri-plugin-process", "@tauri-apps/plugin-process"],
  ["tauri-plugin-updater", "@tauri-apps/plugin-updater"],
];

function majorMinor(version) {
  const match = version.match(/^(\d+)\.(\d+)\./);
  if (!match) {
    throw new Error(`Invalid semver version: ${version}`);
  }

  return `${match[1]}.${match[2]}`;
}

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function readCargoPackageVersionFromLock(cargoLock, packageName) {
  const regex = new RegExp(
    `\\[\\[package\\]\\]\\nname = "${escapeRegex(packageName)}"\\nversion = "([^"]+)"`,
    "m"
  );
  return cargoLock.match(regex)?.[1] ?? null;
}

function readPnpmImporterVersionFromLock(pnpmLock, packageName) {
  const lines = pnpmLock.split("\n");
  const quotedName = `'${packageName}':`;
  const unquotedName = `${packageName}:`;

  for (let i = 0; i < lines.length; i += 1) {
    const trimmed = lines[i].trim();
    if (trimmed !== quotedName && trimmed !== unquotedName) {
      continue;
    }

    for (let j = i + 1; j < Math.min(i + 5, lines.length); j += 1) {
      const versionMatch = lines[j].trim().match(/^version:\s*([^\s(]+)/);
      if (versionMatch) {
        return versionMatch[1];
      }
    }
  }

  return null;
}

export function findTauriVersionMismatches(cargoLock, pnpmLock) {
  const mismatches = [];

  for (const [cargoPackage, npmPackage] of packagePairs) {
    const cargoVersion = readCargoPackageVersionFromLock(cargoLock, cargoPackage);
    const npmVersion = readPnpmImporterVersionFromLock(pnpmLock, npmPackage);

    if (!cargoVersion || !npmVersion) {
      continue;
    }

    if (majorMinor(cargoVersion) !== majorMinor(npmVersion)) {
      mismatches.push(`${cargoPackage} (${cargoVersion}) != ${npmPackage} (${npmVersion})`);
    }
  }

  return mismatches;
}

function main() {
  const cargoLock = fs.readFileSync(cargoLockPath, "utf8");
  const pnpmLock = fs.readFileSync(pnpmLockPath, "utf8");
  const mismatches = findTauriVersionMismatches(cargoLock, pnpmLock);

  if (mismatches.length > 0) {
    console.error("Found mismatched Tauri Rust/NPM package major-minor versions:");
    for (const mismatch of mismatches) {
      console.error(`  ${mismatch}`);
    }
    process.exit(1);
  }

  console.log("Tauri Rust/NPM package versions are aligned.");
}

if (process.argv[1] === __filename) {
  main();
}
