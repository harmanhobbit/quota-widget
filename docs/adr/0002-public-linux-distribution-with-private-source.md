# Publish Linux distribution artifacts while keeping the source private

Quota Widget supports Linux but its Nix flake is available only to source-repository
collaborators. We will publish Linux distribution artifacts from the private source
repository to the public distribution repository, without opening the source.

The first artifact is one AppImage. This gives Linux users a public installation
path without immediately committing the release workflow and support documentation
to separate Debian-family packages; Nix remains the reproducible route.

The public Linux binary target is `x86_64` only. The flake can continue to
describe additional architectures, but they are not a published-binary promise.

The signed AppImage participates in in-app updates when the running build is
itself an AppImage; the updater replaces it in place and the user relaunches.
Nix builds remain non-installable and direct users to their normal Nix upgrade.
Its manifest entry is `linux-x86_64-appimage`, the updater's artifact-qualified
target, rather than the generic `linux-x86_64` key; this leaves future package
formats their own target entries.

The AppImage compatibility floor is Ubuntu 22.04 or an equivalent-or-newer
userspace. The release workflow must pin that baseline rather than inherit the
changing `ubuntu-latest` runner.

Before publication, the first Linux release is manually validated in a Kubuntu
22.04 VM for launch, tray interaction, popup and mini-summary placement, and
one in-app AppImage update. Later Linux releases still require launch, tray,
and popup validation there.

The public download page documents manual verification of the AppImage with its
release signature and the existing public minisign key, as well as automatic
verification during in-app updates.

Desktop integration is self-managed and per-user: Quota Widget installs its
own launcher and icons under the user's data directory. It neither relies on an
external AppImage integration daemon nor writes system-wide state.

The first AppImage launch asks before adding that integration and remembers a
deferral. Settings always exposes explicit add and remove actions afterward.
The launcher targets the user's original AppImage path rather than a copied,
app-managed executable; replacing that file during an update preserves the
launcher target.
If a manually launched AppImage has moved, Quota Widget detects that its
registered launcher points elsewhere and offers, rather than silently performs,
the repair.

After an AppImage update, the app offers **Restart now** and **Later**. Linux
does not restart automatically, so the replacement takes effect on the next
launch when the user defers it.

Removal only deletes unmodified, app-owned launcher and icon files. An ownership
marker distinguishes them from user changes; modified files are preserved and
the user is told how to remove them manually.
