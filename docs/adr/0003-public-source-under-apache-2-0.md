# The source repository is public under Apache-2.0

ADR-0002 was written while the source repository was private, and its opening
premise — "its Nix flake is available only to source-repository collaborators…
without opening the source" — no longer holds. The source repository is public
and the whole tree is licensed Apache-2.0, whose complete text is `LICENSE` at
the repo root. That grant is the authoritative one; the workspace `Cargo.toml`,
`package.json` and `nix/package.nix` metadata all name it and must stay
consistent with it.

**Only that premise is superseded.** Every distribution decision ADR-0002 makes
still stands, unchanged — with one later exception noted inline below:

- Release artifacts are published to the separate public dist repository, not
  attached to the source repo. `release.yml` remains the single publisher.
  (**Superseded by [ADR-0005](0005-main-repo-primary-release-channel.md):** the
  main repo is now the primary publish target and `release.yml` publishes to
  both, with the dist repo kept as a temporary compatibility mirror.)
- The Linux artifact is one signed `x86_64` AppImage; `x86_64` is the only
  published-binary promise, whatever architectures the flake describes.
- Every published artifact keeps its updater signature, and the manifest's
  Linux entry keeps the artifact-qualified `linux-x86_64-appimage` target.
- The AppImage compatibility floor stays Ubuntu 22.04, pinned in the workflow
  rather than inherited from `ubuntu-latest`, and Linux releases still need the
  manual validation pass in `docs/linux-release-validation.md`. (The floor
  itself is since dropped — see
  [ADR-0004](0004-drop-ubuntu-22-04-compatibility-floor.md); the pinned-runner
  rule and the manual-validation requirement both stand.)
- Desktop integration stays self-managed, per-user and opt-in, with the
  ownership-marker rules for removal.

Publishing the source does not turn a source build into a supported
distribution channel. The Nix flake is now readable and buildable by anyone,
but it remains the reproducible source route rather than a published binary:
Nix builds stay non-installable by the updater and direct users to their normal
Nix upgrade, exactly as before.

## Consequences

Documentation may no longer explain a gap by pointing at repository visibility.
The reason to prefer a signed release artifact over a source build is that it
is signed, reproducible in CI and validated on the project's tested platforms
(ADR-0004) — not that the source is unavailable. `docs/dist-README.md` in particular is still
written for downloaders rather than builders, but for that reason and not
because the code is unreadable.

Release credentials are unaffected: `TAURI_SIGNING_PRIVATE_KEY`, its password
and `DIST_REPO_TOKEN` stay CI secrets and never appear in the tree. A public
repository makes that rule more load-bearing, not less.
