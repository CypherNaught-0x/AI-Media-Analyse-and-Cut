import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "..");

const packageJsonPath = path.join(repoRoot, "package.json");
const tauriConfPath = path.join(repoRoot, "src-tauri", "tauri.conf.json");
const cargoTomlPath = path.join(repoRoot, "src-tauri", "Cargo.toml");

const semverPattern = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, "utf8"));
const version = packageJson.version;

if (!version || !semverPattern.test(version)) {
  console.error("Invalid or missing version in package.json");
  process.exit(1);
}

const tauriConf = fs.readFileSync(tauriConfPath, "utf8");
const tauriVersionPattern = /("version"\s*:\s*")([^"]+)(")/;

if (!tauriVersionPattern.test(tauriConf)) {
  console.error("No version field found in src-tauri/tauri.conf.json");
  process.exit(1);
}

const tauriUpdated = tauriConf.replace(
  tauriVersionPattern,
  `$1${version}$3`
);

fs.writeFileSync(tauriConfPath, tauriUpdated, "utf8");

const cargoToml = fs.readFileSync(cargoTomlPath, "utf8");
const cargoVersionPattern = /(^version\s*=\s*")([^"]+)(")/m;

if (!cargoVersionPattern.test(cargoToml)) {
  console.error("No version field found in src-tauri/Cargo.toml");
  process.exit(1);
}

const cargoUpdated = cargoToml.replace(
  cargoVersionPattern,
  `$1${version}$3`
);

fs.writeFileSync(cargoTomlPath, cargoUpdated, "utf8");

console.log(`Synced Tauri versions to ${version}`);
