#!/usr/bin/env node
// The native Glance home-screen widget host (issue #113) lives as committed
// source under `src-tauri/android-widget/`, but the Gradle project it compiles
// into — `src-tauri/gen/android` — is generated fresh by `tauri android init`
// on every build and is gitignored (see build.yml's comment on that step). So,
// exactly like `patch-android-manifest.mjs` reapplies the backup exclusion, this
// script reapplies the widget host after `tauri android init` and before the
// build: it copies the Kotlin sources, resources and unit tests into the
// generated project, adds the Glance/WorkManager/Compose Gradle wiring, and
// registers the widget receiver, its configuration activity and the app's
// deep-link filter in the generated manifest.
//
// Idempotent: every step checks for a marker and no-ops if it already ran, so a
// second invocation (or a partially-patched tree) is safe.
//
// Run from the repo root: `node scripts/patch-android-glance-widget.mjs`.
import { readFileSync, writeFileSync, mkdirSync, cpSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const GEN = 'src-tauri/gen/android';
const SRC = 'src-tauri/android-widget';
const PKG_PATH = 'tech/allaway/quotawidget/widget';

function fail(msg) {
  console.error(`patch-android-glance-widget: ${msg}`);
  process.exit(1);
}

if (!existsSync(GEN)) {
  fail(`${GEN} not found — run \`tauri android init\` first.`);
}

// ---- 1. Copy Kotlin sources, resources and unit tests ----------------------

const mainJava = join(GEN, 'app/src/main/java', PKG_PATH);
const testJava = join(GEN, 'app/src/test/java', PKG_PATH);
const resXml = join(GEN, 'app/src/main/res/xml');

mkdirSync(mainJava, { recursive: true });
mkdirSync(testJava, { recursive: true });
mkdirSync(resXml, { recursive: true });

cpSync(join(SRC, 'kotlin', PKG_PATH), mainJava, { recursive: true });
cpSync(join(SRC, 'test', PKG_PATH), testJava, { recursive: true });
cpSync(join(SRC, 'res/xml'), resXml, { recursive: true });
console.log('copied widget Kotlin sources, resources and tests into the generated project');

// ---- 2. Gradle wiring: Compose + Glance + WorkManager + test deps ----------

const gradlePath = join(GEN, 'app/build.gradle.kts');
let gradle = readFileSync(gradlePath, 'utf8');

if (gradle.includes('// quota-widget:glance')) {
  console.log(`${gradlePath} already wired for Glance — leaving it untouched.`);
} else {
  // Glance requires the Compose compiler. Kotlin 2.0+ uses the dedicated
  // compose plugin; 1.9.x uses composeOptions with a matched extension version.
  // Detect the Kotlin version the generated project pins so the right path is
  // taken rather than guessed.
  const kotlinVersion = detectKotlinVersion();
  const isKotlin2 = kotlinVersion && Number(kotlinVersion.split('.')[0]) >= 2;

  if (isKotlin2) {
    // Apply the compose compiler plugin (its version tracks the Kotlin plugin,
    // resolved from the same place the kotlin.android plugin is).
    gradle = gradle.replace(
      /id\("org\.jetbrains\.kotlin\.android"\)([^\n]*)/,
      `id("org.jetbrains.kotlin.android")$1\n    id("org.jetbrains.kotlin.plugin.compose") // quota-widget:glance`,
    );
    ensurePluginVersion(kotlinVersion);
  }

  // Enable Compose and add the dependencies by APPENDING second `android {}`
  // and `dependencies {}` blocks. The Kotlin Gradle DSL merges repeated
  // extension blocks, so this configures the same android extension without any
  // fragile in-place edit of the generated file (whose exact shape — whether it
  // already has a `buildFeatures {}` block, and where — we cannot assume). The
  // earlier in-place approach silently failed to land `composeOptions`, so AGP
  // used no Compose compiler and IR lowering blew up on `CompositionLocal.current`.
  //
  // Compose artifacts are pinned to one consistent version the compiler the
  // Kotlin plugin pins supports (Glance 1.1.1 only drags in ancient compose, so
  // these explicit, matched versions win): 1.6.8 with compiler 1.5.x on the
  // Kotlin 1.9.x path; a current line on the Kotlin 2.0+ path, whose
  // version-tracking compose plugin owns the compiler. glance-material3 is
  // intentionally absent — GlanceTheme ships in glance-appwidget.
  const compose = isKotlin2 ? '1.7.5' : '1.6.8';
  const composeOptionsBlock = isKotlin2
    ? ''
    : `
    composeOptions {
        kotlinCompilerExtensionVersion = "${composeCompilerFor(kotlinVersion)}"
    }`;
  const addition = `
// quota-widget:glance — Compose + Glance + WorkManager host (issue #113)
android {
    buildFeatures {
        compose = true
    }${composeOptionsBlock}
}
dependencies {
    implementation("androidx.glance:glance-appwidget:1.1.1")
    implementation("androidx.work:work-runtime-ktx:2.9.1")
    implementation("androidx.compose.runtime:runtime:${compose}")
    implementation("androidx.compose.ui:ui-unit:${compose}")
    implementation("androidx.compose.ui:ui-graphics:${compose}")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}
`;
  gradle = gradle.trimEnd() + '\n' + addition;
  writeFileSync(gradlePath, gradle);
  console.log(`wired ${gradlePath} for Compose + Glance + WorkManager (kotlin ${kotlinVersion ?? 'unknown'})`);
  // Dump the appended wiring so a CI failure has the exact Gradle to look at.
  console.log(`--- quota-widget:glance appended to build.gradle.kts ---${addition}--- end ---`);
}

// ---- 3. Manifest: receiver, config activity, deep-link filter --------------

const manifestPath = join(GEN, 'app/src/main/AndroidManifest.xml');
let manifest = readFileSync(manifestPath, 'utf8');

if (manifest.includes('QuotaWidgetReceiver')) {
  console.log(`${manifestPath} already registers the widget — leaving it untouched.`);
} else {
  const entries = `
        <!-- quota-widget:glance — home-screen widget (issue #113) -->
        <receiver
            android:name=".widget.QuotaWidgetReceiver"
            android:exported="true">
            <intent-filter>
                <action android:name="android.appwidget.action.APPWIDGET_UPDATE" />
            </intent-filter>
            <meta-data
                android:name="android.appwidget.provider"
                android:resource="@xml/quota_glance_widget_info" />
        </receiver>
        <activity
            android:name=".widget.WidgetConfigActivity"
            android:exported="true">
            <intent-filter>
                <action android:name="android.appwidget.action.APPWIDGET_CONFIGURE" />
            </intent-filter>
        </activity>
`;
  // Insert just before </application>.
  if (!manifest.includes('</application>')) {
    fail(`no </application> tag to patch in ${manifestPath}`);
  }
  manifest = manifest.replace('</application>', `${entries}    </application>`);

  // Deep link: a row tap opens the app at quotawidget://account/<id>. Add the
  // scheme filter to MainActivity, the launcher activity Tauri generates.
  const deepLink = `
            <intent-filter android:autoVerify="false">
                <action android:name="android.intent.action.VIEW" />
                <category android:name="android.intent.category.DEFAULT" />
                <category android:name="android.intent.category.BROWSABLE" />
                <data android:scheme="quotawidget" android:host="account" />
            </intent-filter>
`;
  const mainActivity = /(<activity\b[^>]*android:name="\.MainActivity"[^>]*>)/;
  if (mainActivity.test(manifest)) {
    manifest = manifest.replace(mainActivity, `$1${deepLink}`);
  } else {
    console.warn(
      'patch-android-glance-widget: could not find MainActivity to add the deep-link filter; ' +
        'the widget still renders, but row taps will not deep-link until it is added.',
    );
  }

  writeFileSync(manifestPath, manifest);
  console.log(`registered the widget receiver, config activity and deep-link filter in ${manifestPath}`);
}

// ---- helpers ---------------------------------------------------------------

function detectKotlinVersion() {
  // Look wherever Tauri might pin the Kotlin Gradle plugin.
  const candidates = [
    join(GEN, 'build.gradle.kts'),
    join(GEN, 'settings.gradle.kts'),
    join(GEN, 'buildSrc/src/main/java/com/tauri/Config.kt'),
    'src-tauri/gen/android/gradle/libs.versions.toml',
  ];
  const patterns = [
    /kotlin-gradle-plugin:([0-9]+\.[0-9]+\.[0-9]+)/,
    /org\.jetbrains\.kotlin[^"]*"\s+version\s+"([0-9]+\.[0-9]+\.[0-9]+)"/,
    /kotlin\s*=\s*"([0-9]+\.[0-9]+\.[0-9]+)"/,
  ];
  for (const file of candidates) {
    if (!existsSync(file)) continue;
    const text = readFileSync(file, 'utf8');
    for (const re of patterns) {
      const m = text.match(re);
      if (m) return m[1];
    }
  }
  return null;
}

// The Compose compiler extension version compatible with a given Kotlin 1.9.x.
// Only used on the pre-2.0 path; the map covers the versions Tauri has shipped.
function composeCompilerFor(kotlinVersion) {
  const map = {
    '1.9.25': '1.5.15',
    '1.9.24': '1.5.14',
    '1.9.23': '1.5.13',
    '1.9.22': '1.5.10',
    '1.9.20': '1.5.4',
  };
  return map[kotlinVersion] ?? '1.5.15';
}

// Ensure the compose plugin is resolvable with the Kotlin version on the
// classpath (Kotlin 2.0+ path). Tauri declares the kotlin.android plugin in the
// root buildscript/plugins; the compose plugin ships in the same coordinates
// group, so if kotlin.android resolves, so does this once declared above.
function ensurePluginVersion(kotlinVersion) {
  const root = join(GEN, 'build.gradle.kts');
  if (!existsSync(root)) return;
  let text = readFileSync(root, 'utf8');
  if (text.includes('kotlin-compose') || text.includes('org.jetbrains.kotlin.plugin.compose')) return;
  // If the root declares the kotlin plugin classpath, add the compose one too,
  // so the app module's `id(... compose)` resolves without its own version.
  const m = text.match(/classpath\("org\.jetbrains\.kotlin:kotlin-gradle-plugin:([0-9.]+)"\)/);
  if (m) {
    text = text.replace(
      m[0],
      `${m[0]}\n        classpath("org.jetbrains.kotlin:compose-compiler-gradle-plugin:${m[1]}")`,
    );
    writeFileSync(root, text);
    console.log(`added the compose-compiler-gradle-plugin classpath (${m[1]}) to ${root}`);
  }
}
