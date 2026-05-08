import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { findTauriVersionMismatches } from '../../scripts/check-tauri-versions.js';

const REPO_ROOT = resolve(__dirname, '..', '..');

describe('Tauri package version alignment', () => {
  it('keeps Rust and NPM Tauri packages on matching major-minor versions', () => {
    const cargoLock = readFileSync(resolve(REPO_ROOT, 'src-tauri', 'Cargo.lock'), 'utf8');
    const pnpmLock = readFileSync(resolve(REPO_ROOT, 'pnpm-lock.yaml'), 'utf8');

    expect(findTauriVersionMismatches(cargoLock, pnpmLock)).toEqual([]);
  });

  it('flags the tauri crate and @tauri-apps/api mismatch that broke builds', () => {
    const cargoLock = `[[package]]
name = "tauri"
version = "2.11.1"
`;
    const pnpmLock = `importers:

  .:
    dependencies:
      '@tauri-apps/api':
        specifier: ~2.10.0
        version: 2.10.1
`;

    expect(findTauriVersionMismatches(cargoLock, pnpmLock)).toEqual([
      'tauri (2.11.1) != @tauri-apps/api (2.10.1)',
    ]);
  });
});
