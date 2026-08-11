# Linux release validation

The Linux half of a release is not covered by CI beyond "it built". The tray is
a native StatusNotifierItem, window placement depends on the session's display
server, the desktop integration writes files into a real home directory, and an
AppImage update rewrites the running executable — none of that is observable
from a build log. This is the manual pass that covers it, and what each step is
actually evidence *of*.

Two audiences: whoever cuts a release runs the short set; whoever ships a
change to the AppImage, the updater, or the launcher runs the full set.

- **First Linux release of a change to distribution** — everything below.
- **Every later Linux release** — [Reduced later-release
  checks](#reduced-later-release-checks): launch, tray, popup.

The environment is a **Kubuntu 22.04 VM**. That is deliberate: Ubuntu 22.04 is
the compatibility floor the AppImage is built against (`release.yml` pins
`ubuntu-22.04` for `build-linux`), and Kubuntu puts KDE Plasma in front of it,
which is the desktop whose SNI tray and panel geometry the widget is written
for. Testing on anything newer proves the app runs *above* the floor, not at
it. See [NixOS is supplemental](#nixos-appimage-run-is-supplemental-coverage)
for what the other Linux box here can and cannot tell you.

---

## 1. Release dry run: inspect the signed manifest and artifacts

A `workflow_dispatch` of `release.yml` defaults to a dry run — it builds and
signs with the real key, then attaches the results as workflow artifacts
instead of creating a public release. This is the only way to see the signed
output before a tag exists, and the only way to exercise the signing secrets,
which no agent and no local checkout can read.

```sh
gh workflow run release.yml --ref <branch-or-main>   # dry_run defaults to true
gh run watch                                          # or: gh run list --workflow release.yml
gh run download <run-id> -n release-dry-run
```

A dry run still builds Windows, which bills at a **2x** minute multiplier, so
dispatch it once when you mean to inspect the result — not as a smoke test.
`verify-tag` is skipped on a dispatch (there is no tag to compare), which is
expected; a *failed* `verify-tag` still blocks everything.

The `release-dry-run` artifact holds the NSIS installer and its `.sig`, the
portable EXE, the AppImage and its `.sig`, and `latest.json`. What to check:

- [ ] Both platform jobs succeeded — `Signed Windows artifacts` and
      `Signed AppImage (Ubuntu 22.04)`. `publish` will not run otherwise, and
      the point of the dry run is the *combined* manifest.
- [ ] `latest.json` carries **both** platform entries, each with a non-empty
      `signature` and a `url`:
      - `platforms["windows-x86_64"]` — the bare key, because Windows publishes
        one installer.
      - `platforms["linux-x86_64-appimage"]` — artifact-qualified, because Linux
        has several mutually exclusive package formats and a bare
        `linux-x86_64` would let a future `.deb` entry collide with or replace
        the AppImage's. This is the key
        `UpdateTarget::keys()` looks for first, so a build running as an
        AppImage selects this entry and nothing else.
- [ ] `version` is bare SemVer with no `v`, and matches `Cargo.toml`. The tag
      keeps its `v`; the manifest does not.
- [ ] Each `url` names the dist repo's download path for tag `v<version>` and
      the asset's real basename. A dry run publishes nothing, so these URLs are
      *predictions* — they only resolve once the tag ships. Check the shape,
      not that they 200.
- [ ] Each `signature` is the verbatim contents of the matching `.sig` file
      (the manifest is built with `jq --rawfile`, so it should match byte for
      byte, newlines included).
- [ ] The AppImage's signature verifies against the public key baked into
      `tauri.conf.json`:

      ```sh
      minisign -Vm QuotaWidget_<version>_amd64.AppImage \
        -x QuotaWidget_<version>_amd64.AppImage.sig \
        -P RWQHEg24HhWu6QFITG26y7995k+xW1CG3IHAplDddbIF1LahMc7G7fsz
      ```

      This is the same key the updater verifies with before installing, so a
      failure here is a failure of every in-app update, not a packaging detail.

Keep this AppImage. It is the "old version" the update test in step 4 needs.

---

## 2. Direct AppImage launch on the Kubuntu 22.04 VM

Copy the AppImage to the VM — a normal download location, not a path you would
never use — and start it the way the download page tells a user to:

```sh
chmod +x QuotaWidget_<version>_amd64.AppImage
./QuotaWidget_<version>_amd64.AppImage
```

- [ ] It starts on Kubuntu 22.04 with no extra packages installed. This is the
      compatibility-floor claim; anything you had to `apt install` first
      invalidates it and needs recording.
- [ ] An icon appears in the panel. Both windows start hidden, so nothing is
      presented unless the first poll trips an auto-popup alert.
- [ ] Settings' footer reads the version you built.

**Session type matters, and direct launch is the uninstrumented case.** A
direct `./…AppImage` on a Wayland session runs natively on Wayland, which has
no always-on-top and cannot position its own windows. Only the launcher this
app writes adds `env GDK_BACKEND=x11`. So:

- [ ] On a **Wayland** session, Settings shows the "You're on Wayland…" note,
      and the popup slipping behind other windows is the known upstream gap
      (tao#1134), not a regression.
- [ ] Run the placement checks in step 3 from an **X11 session**, or from a
      launcher-started instance, or with `GDK_BACKEND=x11 ./…AppImage`. That is
      the configuration the shipped launcher produces and therefore the one
      users get.

---

## 3. Tray, popup, and mini-summary placement

The tray on Linux is `ksni` (`src-tauri/src/tray_linux.rs`), not the
appindicator path — hence the tooltip and left-click activation, neither of
which appindicator delivers.

- [ ] **Left-click** the tray icon toggles the mini summary near the tray.
- [ ] **Hover** shows a Plasma-drawn tooltip listing each account's windows and
      balances. It is Plasma rendering the SNI `ToolTip` property; the app never
      learns a hover happened, so there is no window to inspect here.
- [ ] **Right-click** offers exactly: Open, Refresh now, Settings, Quit — and
      each does what it says.
- [ ] The icon's colour tracks worst-case status (green / amber / red / grey
      when stale).
- [ ] The popup opens near the tray and **entirely inside the work area** — not
      underneath the Plasma panel. Placement uses `work_area()` precisely so
      that panel-excluded space is respected; a popup under the panel means
      that regressed.
- [ ] The mini summary snaps to the nearest corner of the screen it is dropped
      on, and reopens there.
- [ ] Pinning (the circle button) holds the summary exactly where it is and
      stops it hiding on focus loss; it does not jump to a stored position.
- [ ] With a second virtual display attached, dragging the summary to it and
      reopening puts it back on that screen.

---

## 4. Opt-in desktop integration

An AppImage is a file the user downloaded; nothing in the system knows it
exists. All of this is per-user, under `$XDG_DATA_HOME` (`~/.local/share` by
default) — there is no system-wide state, no `appimaged`, and no daemon.

**First-run prompt.** Shown only when the app is a running AppImage, no
launcher exists at our path, and the question has not been put before.

- [ ] On the first launch, the popup shows "Add Quota Widget to your
      applications menu?" with **Not now** and **Add it**.
- [ ] **Not now** dismisses it, and it never returns — including after a
      restart. The record is that the user was *asked*, so a decline sticks as
      firmly as an accept (`desktop_integration_prompted` in
      `~/.config/quota-widget/config.json`).
- [ ] **Add it** writes, and nothing else:
      - `~/.local/share/applications/quota-widget.desktop`
      - `~/.local/share/icons/hicolor/128x128/apps/quota-widget.png`
      - `~/.local/share/icons/hicolor/32x32/apps/quota-widget.png`
- [ ] The `.desktop` file's `Exec` is `env GDK_BACKEND=x11 "<path to your
      AppImage>"` — the XWayland workaround, matching what `nix/package.nix`
      emits — and it carries `X-QuotaWidget-Managed=true`.
- [ ] "Quota Widget" appears in the Plasma application launcher, with its icon,
      and starting it from there works.

To re-run the prompt: quit the app, delete the launcher, and set
`desktop_integration_prompted` back to `false` in `config.json`.

**Settings → Applications menu.** The section appears only for a running
AppImage; every other build already owns its menu entry from an installer or a
package.

- [ ] With no launcher: **Add to applications menu**, and it creates one.
- [ ] With a current launcher: **Remove from applications menu**, and it deletes
      the launcher and both icons.
- [ ] After *moving* the AppImage and relaunching from its new path, the button
      reads **Repair launcher** and the note names the old target. Repairing
      rewrites `Exec` to the new path. Nothing is retargeted silently — the
      file may have been pointed somewhere deliberately.
- [ ] Edit the `.desktop` file by hand (change a line, keep the marker), then
      reopen Settings: the section offers no buttons and says the file is not
      one this app wrote, or has been edited. Ownership is a marker *plus* a
      byte-for-byte match, so an edited file is the user's.
- [ ] Edit one icon's bytes, then Remove: the launcher goes, the edited icon
      stays, and Settings names the preserved file so it can be deleted by hand.

---

## 5. One end-to-end signed in-app update

**Sequencing constraint, read this first.** The app fetches its manifest from a
fixed URL — the dist repo's *latest* release — so this test needs an installed
AppImage older than a published release that carries a
`linux-x86_64-appimage` entry. Two published Linux releases, or one published
release plus a retained older AppImage from a dry run of an earlier ref, are
the only ways to get there. Until such a pair exists, this step cannot be run,
and the honest record is "not yet exercised", not a pass.

With an older AppImage running on the VM:

- [ ] Settings shows **Update available: v\<latest\>**, and **Check now**
      produces it on demand even with **Check for updates** unticked (pressing
      the button is itself the consent).
- [ ] **Install update** is offered. It appears only when the running build is
      an *installable artifact* — the updater's bundle type says AppImage — not
      merely because a download exists.
- [ ] Pressing it shows `Downloading…`, then `Installing…` — **not** the
      "close and reopen" wording, which is the Windows ending.
- [ ] It finishes with `Version <latest> installed. Restart to use it.` and a
      **Restart now** / **Later** pair. Nothing has relaunched: replacing an
      AppImage rewrites the file underneath a process still running the old
      code.
- [ ] The AppImage **at its original path** has been replaced — same path, new
      size and mtime. A new file elsewhere is a bug: the launcher targets the
      original path, and in-place replacement is what keeps it valid.
- [ ] An app-owned launcher created in step 4 still works after the update, with
      its `Exec` unchanged and Settings still reporting the launcher as current.

Then verify both endings. Do **Later** first, because it is the one that can
silently be wrong:

- [ ] **Later** clears the prompt and says the new version is installed and
      starts next time. It must not undo, revert, or re-download anything.
- [ ] Quit from the tray, start the same file again: it comes up as the new
      version (Settings footer, and the popup header's version). This is the
      whole claim of *Later* — that the install already happened.
- [ ] Repeat the update from the older AppImage and choose **Restart now**: the
      app relaunches itself and comes back as the new version, without the user
      touching the file.

If a signature ever fails to verify, the install must fail rather than run
anything — the message surfaces as `Update failed: …` in the same row.

---

## Reduced later-release checks

Every subsequent Linux release, on the same Kubuntu 22.04 VM, once the tag has
published:

- [ ] **Launch** — the released AppImage starts on the VM with nothing extra
      installed, and puts its icon in the panel.
- [ ] **Tray** — left-click toggles the mini summary, hover shows the tooltip,
      right-click gives Open / Refresh now / Settings / Quit.
- [ ] **Popup** — opens near the tray, inside the work area, and Settings reads
      the released version.

The full set above is for releases that change the AppImage build, the updater
path, or the desktop integration. A release that only touches provider adapters
does not need the update and launcher passes re-run — the core tests cover that
change, and this pass costs a VM boot.

### Recording the result

Record the outcome where the release can be traced from: a comment on the issue
covering that release's Linux work, naming the version, the VM's session type
(X11 or Wayland), and each box's result — including any that could not be run
and why. "Not run" is a legitimate and useful entry; a checklist with silent
gaps is not.

---

## NixOS `appimage-run` is supplemental coverage

The other Linux machine here is NixOS, where an AppImage is normally run with
`appimage-run`. That is worth doing, and it is **not** the compatibility-floor
test.

`appimage-run` extracts the image and runs its payload against a runtime that
Nix supplies — its own glibc, GTK, WebKitGTK and friends, chosen to make
foreign binaries work. A success therefore proves the binary is correct and the
app behaves under Nix's shims. It says nothing about whether the AppImage's
linkage fits the userspace a stock Ubuntu 22.04 system provides, which is the
actual promise on the download page. A binary that quietly picked up a newer
glibc symbol from the build runner would pass under `appimage-run` and fail on
the machine the floor was written for.

So:

- The **compatibility floor is Ubuntu 22.04, or an equivalent-or-newer
  userspace**, and it is tested by launching the AppImage directly on the
  Kubuntu 22.04 VM with no extra packages installed. That is the only check
  that establishes it.
- NixOS `appimage-run` is **supplemental**: a second desktop environment and a
  second way of starting the image, useful for catching behaviour that is not
  about linkage.
- A NixOS pass never substitutes for the VM pass. If only the NixOS run was
  done, the floor is untested and the record must say so.

Note also that the Nix *package* is a different thing again: a reproducible
source build that pins its own GTK/WebKit and is not an installable artifact,
so it shows upgrade guidance rather than an install button. Running the Nix
package proves nothing about the AppImage at all.
