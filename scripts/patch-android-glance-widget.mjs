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

// The Compose approach. Tauri pins Kotlin 1.9.25 + AGP 8.11, and on that
// toolchain AGP no longer honours `composeOptions.kotlinCompilerExtensionVersion`
// to apply the Compose compiler — the Kotlin compiler then can't inline
// `CompositionLocal.current` and IR lowering fails. AGP 8.11's supported path is
// the standalone **Compose Compiler Gradle plugin**, which requires Kotlin 2.0+.
// So the patch bumps the project's Kotlin to 2.0.21 (Kotlin 2.0 recompiles
// Tauri's own Kotlin fine) and applies that plugin — the deterministic, current
// way to get Compose, no compiler-extension guessing.
const KOTLIN = '2.0.21';
const COMPOSE = '1.7.5';

if (gradle.includes('// quota-widget:glance')) {
  console.log(`${gradlePath} already wired for Glance — leaving it untouched.`);
} else {
  bumpKotlinAndComposePlugin(KOTLIN);

  // Apply the Compose compiler plugin in the app module's plugins block.
  gradle = gradle.replace(
    /id\("org\.jetbrains\.kotlin\.android"\)([^\n]*)/,
    `id("org.jetbrains.kotlin.android")$1\n    id("org.jetbrains.kotlin.plugin.compose") // quota-widget:glance`,
  );

  // Enable Compose and add dependencies by APPENDING a second `android {}` and
  // `dependencies {}` block. The Kotlin Gradle DSL merges repeated extension
  // blocks, so this configures the same android extension without any fragile
  // in-place edit of the generated file's shape. With the compose plugin
  // applied above, `buildFeatures.compose = true` is all that is needed — no
  // composeOptions. Compose 1.7.x pairs with Kotlin 2.0 and compileSdk 35+
  // (the template uses 36); glance-material3 is intentionally absent —
  // GlanceTheme ships in glance-appwidget.
  const addition = `
// quota-widget:glance — Compose + Glance + WorkManager host (issue #113)
android {
    buildFeatures {
        compose = true
    }
    // The widget host formats the last-update instant with java.time (issue
    // #195). The generated project's minSdk is 24 (Tauri's default; java.time
    // is API 26+), so core library desugaring is required for API 24/25
    // devices — without it those throw NoClassDefFoundError on first render.
    compileOptions {
        isCoreLibraryDesugaringEnabled = true
    }
}
dependencies {
    implementation("androidx.glance:glance-appwidget:1.1.1")
    implementation("androidx.work:work-runtime-ktx:2.9.1")
    // Notification posting lives on the native host since #112 (AlertNotifier
    // / NotificationAccess): NotificationCompat and friends are androidx.core.
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.compose.runtime:runtime:${COMPOSE}")
    implementation("androidx.compose.ui:ui-unit:${COMPOSE}")
    implementation("androidx.compose.ui:ui-graphics:${COMPOSE}")
    coreLibraryDesugaring("com.android.tools:desugar_jdk_libs:2.1.5")
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20240303")
}
`;
  gradle = gradle.trimEnd() + '\n' + addition;
  writeFileSync(gradlePath, gradle);
  console.log(`wired ${gradlePath} for Compose + Glance + WorkManager (kotlin ${KOTLIN})`);
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

// Bump the project's Kotlin Gradle plugin to [kotlin] and add the matching
// Compose Compiler Gradle plugin to the root buildscript classpath, so the app
// module's `id("org.jetbrains.kotlin.plugin.compose")` resolves. Tauri pins the
// Kotlin plugin in the root buildscript `classpath(...)`; this rewrites that
// version and appends the compose-compiler classpath beside it. Idempotent via
// a marker comment.
function bumpKotlinAndComposePlugin(kotlin) {
  const root = join(GEN, 'build.gradle.kts');
  if (!existsSync(root)) {
    fail(`${root} not found — cannot wire the Compose compiler plugin.`);
  }
  let text = readFileSync(root, 'utf8');
  if (text.includes('// quota-widget:kotlin')) {
    console.log(`${root} already bumped for the Compose compiler — leaving it untouched.`);
    return;
  }
  const kgp = /classpath\("org\.jetbrains\.kotlin:kotlin-gradle-plugin:[0-9.]+"\)/;
  if (!kgp.test(text)) {
    fail(`could not find the kotlin-gradle-plugin classpath to bump in ${root}`);
  }
  text = text.replace(
    kgp,
    `classpath("org.jetbrains.kotlin:kotlin-gradle-plugin:${kotlin}") // quota-widget:kotlin\n` +
      `        classpath("org.jetbrains.kotlin:compose-compiler-gradle-plugin:${kotlin}")`,
  );
  writeFileSync(root, text);
  console.log(`bumped Kotlin to ${kotlin} and added the compose-compiler plugin classpath in ${root}`);
}
