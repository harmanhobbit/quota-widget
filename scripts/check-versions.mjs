// The workspace Cargo.toml is the single source of truth for the app version.
// Everything else derives from it:
//
//   src-tauri/tauri.conf.json  omits `version` -> tauri reads Cargo.toml
//   nix/package.nix            lib.importTOML ../Cargo.toml
//   package.json               no version field (private, nothing reads it)
//
// This guard fails if anything reintroduces a hardcoded copy, which is how
// nix/package.nix silently sat at 0.2.0 through a 0.3.0 release.
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const read = (p) => readFileSync(join(root, p), 'utf8');

const version = read('Cargo.toml').match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) {
  console.error('check-versions: no [workspace.package] version in Cargo.toml');
  process.exit(1);
}

// Files that must NOT carry their own copy, and how a copy would look.
const MUST_NOT_PIN = [
  ['package.json', /^\s*"version"\s*:/m],
  ['src-tauri/tauri.conf.json', /^\s*"version"\s*:/m],
  ['nix/package.nix', /^\s*version\s*=\s*"/m],
];

const offenders = MUST_NOT_PIN.filter(([file, pattern]) => pattern.test(read(file)));
if (offenders.length > 0) {
  console.error('check-versions: hardcoded version reintroduced in:');
  for (const [file] of offenders) console.error(`  ${file}`);
  console.error('\nRemove it — these derive from Cargo.toml. See this script’s header.');
  process.exit(1);
}

console.log(`check-versions: ok — single source of truth, version ${version}`);
