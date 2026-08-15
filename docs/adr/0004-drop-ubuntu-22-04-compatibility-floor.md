# Drop the Ubuntu 22.04 compatibility floor; build the AppImage on Ubuntu 24.04

ADR-0002 set the Linux AppImage's compatibility floor at **Ubuntu 22.04** and
pinned `ubuntu-22.04` in `release.yml` so that a changing `ubuntu-latest` label
could not silently narrow it; ADR-0003 carried that decision forward unchanged.
This ADR supersedes **only** that floor decision. Every other distribution
decision in ADR-0002 and ADR-0003 still stands.

## Context

The 22.04 floor was chosen to maximise the range of hosts a single AppImage
could start on. In practice that trade-off no longer pays:

- The AppImage is a **best-effort** Linux route, not the primary one. The
  project is distributed from source on NixOS and as the signed Windows build;
  the AppImage exists for Linux users who want a direct download.
- GitHub's `ubuntu-22.04` runner is on a **retirement path**. Staying pinned to
  it is itself a standing source of breakage, independent of anything here.
- The 22.04-era **bundled WebKitGTK** is the direct cause of a
  `Could not create default EGL display: EGL_BAD_PARAMETER` abort on
  virgl/virtualized-GPU stacks. A newer WebKit base is the actual fix.
- No known user depends on a pre-24.04 glibc. Anyone on an older userspace can
  build from source or use the reproducible Nix flake.

## Decision

1. **No compatibility-floor promise.** The AppImage is best-effort on
   glibc-based `x86_64` Linux. There is no advertised minimum distribution.
2. **Build on `ubuntu-24.04`.** `release.yml`'s `build-linux` (and `publish`)
   move from `ubuntu-22.04` to `ubuntu-24.04`, bundling a newer GTK/WebKit/GLib.
   The base stays *pinned* — never `ubuntu-latest` — so which userspace is
   bundled remains a deliberate, reviewed choice rather than a drifting one.
   24.04 restricts the unprivileged user namespaces linuxdeploy's FUSE mount
   needs, so the AppImage build runs with `APPIMAGE_EXTRACT_AND_RUN=1`.
3. **Validate on the platforms the project actually runs on**, dropping the
   dedicated Kubuntu 22.04 floor VM:
   - **NixOS + KDE Plasma** (source build) — the SNI tray, popup and
     mini-summary placement against a Plasma panel.
   - **Windows** — the signed release build.
   - **Debian 13 XFCE** — that the shipped AppImage binary starts on a
     mainstream, non-Nix distro.
4. **Known limitation.** The AppImage does not start under virgl/virtualized-GPU
   VMs. That is a virtualized-GPU artifact, not a real-hardware failure, so a
   virgl VM is **not** a valid environment for validating the AppImage.

## Consequences

- Hosts older than the 24.04 userspace may no longer run the AppImage. That is
  accepted: such users build from source (Nix) or use the flake, exactly as
  before.
- The startup shim in `crates/quota-core/src/linux_launch.rs` stays as
  defensive version-skew handling for hosts *newer* than the build base; its
  rationale comment is updated to the 24.04 base rather than removed.
- Documentation no longer states or tests an Ubuntu 22.04 floor. The
  "compatibility floor" glossary term in `CONTEXT.md`, the download guidance in
  `docs/dist-README.md` and `README.md`, and the manual pass in
  `docs/linux-release-validation.md` are updated to the tested-platforms model
  above.
- Nothing else changes: the single signed `x86_64` AppImage, its updater
  signature, and the artifact-qualified `linux-x86_64-appimage` manifest key
  all stand as in ADR-0002/0003.
