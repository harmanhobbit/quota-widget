#!/usr/bin/env node
// `tauri android init` generates src-tauri/gen/android fresh on every build
// (it's gitignored — see .github/workflows/build.yml's comment on that step),
// so any Android-specific manifest customization Tauri's own config doesn't
// expose has to be reapplied here, right after `tauri android init` and
// before the build step. Each patch below is independent and idempotent (it
// checks for its own marker and no-ops), so a partially-patched tree is safe:
//
// 1. Issue #109's acceptance criterion that all application data is excluded
//    from Android automatic backup: `android:allowBackup="false"` on the
//    generated <application> tag. There is no per-file `dataExtractionRules`/
//    `fullBackupContent` to also strip: those XML rulesets only apply when
//    `allowBackup="true"`, and `allowBackup="false"` alone disables both cloud
//    backup and Android 12+ device-to-device transfer.
//
// 2. Issue #112: the POST_NOTIFICATIONS permission declaration. Android 13+
//    requires it to exist before the runtime request can show a dialog, and
//    NotificationManagerCompat's enabled-check reads it; without it the whole
//    notification path silently no-ops. Declaring it requests nothing by
//    itself — the one-time runtime ask is decided in `mobile.rs` /
//    `should_request_notification_permission`.
import { readFileSync, writeFileSync } from 'node:fs';

const path = 'src-tauri/gen/android/app/src/main/AndroidManifest.xml';
let xml = readFileSync(path, 'utf8');
let changed = false;

if (/android:allowBackup\s*=/.test(xml)) {
  console.log(`${path} already sets android:allowBackup — leaving it untouched.`);
} else {
  const patched = xml.replace(/<application\b/, '<application android:allowBackup="false"');
  if (patched === xml) {
    console.error(`could not find an <application> tag to patch in ${path}`);
    process.exit(1);
  }
  xml = patched;
  changed = true;
  console.log(`patched ${path}: added android:allowBackup="false"`);
}

if (/android\.permission\.POST_NOTIFICATIONS/.test(xml)) {
  console.log(`${path} already declares POST_NOTIFICATIONS — leaving it untouched.`);
} else {
  // <uses-permission> is a <manifest> child; immediately before the
  // <application> tag is inside <manifest> and ahead of every component.
  const patched = xml.replace(
    /<application\b/,
    '<uses-permission android:name="android.permission.POST_NOTIFICATIONS" />\n    <application',
  );
  if (patched === xml) {
    console.error(`could not find an <application> tag to patch in ${path}`);
    process.exit(1);
  }
  xml = patched;
  changed = true;
  console.log(`patched ${path}: added the POST_NOTIFICATIONS uses-permission`);
}

if (changed) writeFileSync(path, xml);
