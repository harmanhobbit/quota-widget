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

- [ ] The root `LICENSE` contains the canonical Apache License, Version 2.0,
      and workspace/package metadata names the same license.
- [ ] README and ADR-0002 say that source is public, while retaining ADR-0002's
      decisions about a signed public distribution repository, the Ubuntu 22.04
      AppImage compatibility floor, per-user desktop integration, and
      artifact-qualified updates.
- [ ] The Linux secret-store tests cover restrictive creation permissions,
      atomic replacement, and permission/write failures. Windows Credential
      Manager behavior remains untouched.
- [ ] The configuration tests distinguish missing configuration from malformed
      or unreadable existing configuration, retain the latter, and prove an
      ordinary save cannot overwrite it without an explicit recovery action.
- [ ] `cargo fmt --all -- --check`, strict workspace Clippy, `cargo test -p
      quota-core`, `npm run check-versions`, `npm run build`, and `npm run
      smoke-mount` pass on the merged tree.

## Distribution invariants

Source visibility does not relax the distribution trust boundary. Before
publication, confirm that:

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
