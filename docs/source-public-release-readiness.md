# Source-public release readiness

Issue #81 is an integration release-readiness change. Its implementation is
deliberately split into focused issues:

- #82 makes Linux secret writes private before data is written and atomic.
- #83 makes an unreadable configuration recoverable without silently replacing
  it.
- #84 publishes the Apache-2.0 terms, updates the public-source narrative, and
  makes the automated quality gates enforce formatting and Clippy.

Land and validate those changes together before changing the source repository's
visibility. This checklist is intentionally not a replacement for either
child's tests or for [Linux release validation](linux-release-validation.md).

## Integration gate

Verified on 2026-08-14 against this tree; see the note after the list for the
one check that is CI/dev-shell-gated rather than reproducible on a bare Linux
checkout.

- [x] The root `LICENSE` contains the canonical Apache License, Version 2.0,
      and workspace/package metadata names the same license. (`LICENSE`,
      `Cargo.toml` `license = "Apache-2.0"`, `package.json` `"license":
      "Apache-2.0"`.)
- [x] README and ADR-0002 say that source is public, while retaining ADR-0002's
      decisions about a signed public distribution repository, the AppImage
      compatibility floor (since dropped — ADR-0004), per-user desktop
      integration, and
      artifact-qualified updates. (README states the source is public under
      Apache-2.0; ADR-0002 carries a "partly superseded by ADR-0003" note over
      unchanged distribution decisions; ADR-0003 records the public-source
      decision.)
- [x] The Linux secret-store tests cover restrictive creation permissions,
      atomic replacement, and permission/write failures. Windows Credential
      Manager behavior remains untouched. (`crates/quota-core/src/secret_store.rs`
      tests: owner-only creation and pre-write mode check, group/world-bit
      rejection, atomic update leaving no temp files, and unwritable-location /
      corrupt-store reporting.)
- [x] The configuration tests distinguish missing configuration from malformed
      or unreadable existing configuration, retain the latter, and prove an
      ordinary save cannot overwrite it without an explicit recovery action.
      (`crates/quota-core/src/config.rs` tests: missing file as first run;
      malformed/unreadable/permission-denied files run on defaults and are kept;
      an ordinary save refuses to replace an unreadable config; recovery keeps
      the original aside.)
- [x] `cargo fmt --all -- --check`, strict `quota-core` Clippy, `cargo test -p
      quota-core`, `npm run check-versions`, `npm run build`, and `npm run
      smoke-mount` pass on the merged tree.

`cargo fmt --all -- --check`, `cargo clippy -p quota-core --all-targets --
-D warnings`, `cargo test -p quota-core` (181 passed), `npm run check-versions`,
`npm run build`, and `npm run smoke-mount` were all green here on 2026-08-14.
Clippy over `src-tauri` is not reproducible on a bare Linux checkout — its
GTK/WebKit `-sys` crates cannot compile without the flake dev shell — so, as in
CI, the strict Clippy gate is exercised over `quota-core`; the full-workspace
build/lint is verified under `nix develop` or in CI, not from a plain checkout.

## Distribution invariants

Source visibility does not relax the distribution trust boundary. These are
external/manual checks and remain **unverified** here — a release-workflow dry
run and the manual Linux pass are needed before publication. Before publication,
confirm that:

- `.github/workflows/release.yml` still receives signing and publication
  credentials only from GitHub Actions secrets, never repository values.
- `src-tauri/tauri.conf.json` retains updater artifact creation, the committed
  public updater key, and the existing distribution endpoint; no private
  signing material is committed.
- the release workflow dry run still produces signed Windows installer and
  AppImage artifacts plus `latest.json`, and keeps the
  `linux-x86_64-appimage` manifest entry.
- the existing manual checklist in
  [Linux release validation](linux-release-validation.md) is used for any
  release that changes an AppImage, launcher, or updater path.

The visibility change itself is an external repository setting, not a release
artifact change. Do not tag a release solely to make the source public.
