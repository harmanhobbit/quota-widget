#!/usr/bin/env node
// `tauri android init` generates src-tauri/gen/android fresh on every build
// (it's gitignored — see .github/workflows/build.yml's comment on that step),
// so any Android-specific manifest customization Tauri's own config doesn't
// expose has to be reapplied here, right after `tauri android init` and
// before the build step. Right now that's just one thing: issue #109's
// acceptance criterion that all application data is excluded from Android
// automatic backup (docs/adr/0006-…) — `android:allowBackup="false"` on the
// generated <application> tag. There is no per-file `dataExtractionRules`/
// `fullBackupContent` to also strip: those XML rulesets only apply when
// `allowBackup="true"`, and `allowBackup="false"` alone disables both cloud
// backup and Android 12+ device-to-device transfer.
import { readFileSync, writeFileSync } from 'node:fs';

const path = 'src-tauri/gen/android/app/src/main/AndroidManifest.xml';
const xml = readFileSync(path, 'utf8');

if (/android:allowBackup\s*=/.test(xml)) {
  console.log(`${path} already sets android:allowBackup — leaving it untouched.`);
  process.exit(0);
}

const patched = xml.replace(/<application\b/, '<application android:allowBackup="false"');
if (patched === xml) {
  console.error(`could not find an <application> tag to patch in ${path}`);
  process.exit(1);
}

writeFileSync(path, patched);
console.log(`patched ${path}: added android:allowBackup="false"`);
