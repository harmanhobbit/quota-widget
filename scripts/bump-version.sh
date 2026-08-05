#!/usr/bin/env bash
#
# Cut a release: bump the version, commit, tag, and push.
#
#   ./scripts/bump-version.sh            # prompts for the version
#   ./scripts/bump-version.sh 0.18.0     # or takes it as an argument
#
# The sequence is in AGENTS.md under "Releases"; this exists so it does not
# have to be remembered. Everything up to the push is local and reversible, and
# nothing is pushed without an explicit confirmation.
set -euo pipefail

cd "$(dirname "$0")/.."

die() { printf '\nerror: %s\n' "$*" >&2; exit 1; }

current=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$current" ] || die "no version found in Cargo.toml"

# --- checks that must pass before anything is touched ----------------------

branch=$(git rev-parse --abbrev-ref HEAD)
[ "$branch" = "main" ] || die "releases are cut from main, not $branch (AGENTS.md, \"Releases\")"

[ -z "$(git status --porcelain)" ] || die "working tree is dirty — commit or stash first, so the release commit contains only the version"

git fetch --quiet origin main
behind=$(git rev-list --count HEAD..origin/main)
[ "$behind" -eq 0 ] || die "main is $behind commit(s) behind origin — pull first"

# --- pick the version ------------------------------------------------------

printf 'current version: %s\n' "$current"
if [ $# -ge 1 ]; then
  version="$1"
else
  read -rp 'new version: ' version
fi

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "'$version' is not bare MAJOR.MINOR.PATCH (no 'v' prefix — the tag adds that)"

[ "$version" != "$current" ] || die "$version is already the current version"

# String-compare guard against going backwards. sort -V orders versions
# properly, unlike a plain lexical compare where 0.9.0 > 0.10.0.
older=$(printf '%s\n%s\n' "$current" "$version" | sort -V | head -1)
if [ "$older" = "$version" ]; then
  printf 'warning: %s is OLDER than the current %s\n' "$version" "$current"
  read -rp 'continue anyway? [y/N] ' reply
  [ "$reply" = "y" ] || exit 1
fi

git rev-parse "v$version" >/dev/null 2>&1 && die "tag v$version already exists locally"

# --- make the changes ------------------------------------------------------

printf '\nbumping %s -> %s\n' "$current" "$version"

# Only the first [package]/[workspace.package] version line, which is the
# single source of truth. Dependency version pins must not be touched.
awk -v v="$version" '
  !done && /^version = "/ { sub(/"[^"]*"/, "\"" v "\""); done = 1 }
  { print }
' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

cargo update -w --offline --quiet
npm run --silent check-versions

printf '\n--- changes to be committed ---\n'
git --no-pager diff --stat
printf '\n'
git --no-pager diff -- Cargo.toml

# --- confirm, then commit and tag ------------------------------------------

printf '\nAbout to commit "Release %s", tag v%s, and push to origin/main.\n' "$version" "$version"
printf 'Pushing the tag triggers release.yml, which PUBLISHES to the public\n'
printf 'quota-widget-dist repo. That is not easily undone.\n\n'
read -rp "type the version again to confirm, or anything else to abort: " confirm

if [ "$confirm" != "$version" ]; then
  printf '\naborted — reverting local changes\n'
  git checkout -- Cargo.toml Cargo.lock
  exit 1
fi

git commit --quiet -am "Release $version"
git tag -a "v$version" -m "Release $version"
git push --quiet origin main --follow-tags

printf '\nreleased %s\n\n' "v$version"
printf 'release.yml is now building. Watch it with:\n'
printf '  gh run watch -R harmanhobbit/quota-widget\n\n'
printf 'When it finishes, the release lands at:\n'
printf '  https://github.com/harmanhobbit/quota-widget-dist/releases/latest\n'
