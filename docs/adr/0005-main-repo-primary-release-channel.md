# The main repo is the primary release and update channel; the dist repo is a temporary mirror

ADR-0002 published every distribution artifact to the separate public dist
repository (`harmanhobbit/quota-widget-dist`) and made `release.yml` its single
publisher — a decision taken while the source repository was **private**, so
attaching releases to the source repo was not an option. ADR-0003 made the
source public but carried that publishing decision forward unchanged, explicitly:
"Release artifacts are published to the separate public dist repository, not
attached to the source repo. `release.yml` remains the single publisher."

This ADR supersedes **only** that publishing-location decision. Every other
distribution decision in ADR-0002, ADR-0003 and ADR-0004 still stands: one
signed `x86_64` AppImage, the artifact-qualified `linux-x86_64-appimage`
manifest key, per-user opt-in desktop integration, the pinned Ubuntu 24.04 base
with no compatibility-floor promise, and the credentials staying CI secrets.

## Context

The premise that forced downloads into a separate repo — a private source tree —
no longer holds (ADR-0003). With the source public, releases can live on the
source repo itself, which is where users, contributors and the CI that builds
them already are. Keeping the canonical download and update channel on a second,
downloads-only repo is now indirection without a reason: an extra repo to keep
public, an extra token (`DIST_REPO_TOKEN`) with cross-repo write scope, and a
manifest URL that points away from the project's actual home.

The obstacle is not new builds but old ones. Every client installed before this
change has a manifest endpoint baked into its binary
(`tauri.conf.json` / `updates.rs`) pointing at the dist repo's
`latest/download/latest.json`. That URL is compiled in and cannot be changed
remotely. If the dist repo simply stopped receiving releases, those clients
would silently stop seeing updates.

## Decision

1. **The main repo is the primary release/update channel.** New builds poll and
   download from `harmanhobbit/quota-widget`. Both compiled endpoints move
   there: `tauri.conf.json`'s updater endpoint and `updates.rs`'s `MANIFEST_URL`.
   The generated `latest.json` carries main-repo asset URLs and main-repo
   release notes.

2. **`release.yml` dual-publishes.** On a tag it creates the release on the main
   repo **first**, using the built-in `GITHUB_TOKEN` (granted `contents: write`
   on the publish job only), then mirrors the identical assets and `latest.json`
   to the dist repo using `DIST_REPO_TOKEN`. Main first, so that if the mirror
   step ever fails the channel new builds actually poll is already live.

3. **Two tokens, two scopes, never crossed.** The main-repo release uses only
   `GITHUB_TOKEN`, which can write releases in this repo and nowhere else. The
   dist mirror uses only `DIST_REPO_TOKEN`, whose sole reason to exist is that
   the built-in token cannot write to another repository. `contents: write` is
   scoped to the `publish` job; the top-level default stays `contents: read`.

4. **The dist repo stays a compatibility mirror, not retired.** Dual publishing
   continues so already-installed clients polling the old endpoint keep
   updating. The dist README republish is preserved for the same reason. The
   assets in both repos are byte-identical, so a client that fetched its manifest
   from either repo downloads the same signed files (from the main repo URLs the
   manifest carries); signatures are over file bytes, not URLs, so verification
   is unaffected.

5. **Dry-run and safety invariants are unchanged.** A `workflow_dispatch`
   defaults to a dry run that signs but publishes nothing (neither repo), the
   tag/version `verify-tag` gate still blocks a mislabelled tree, and both
   publish steps are gated on a real `v*.*.*` tag and `dry_run != true`.

## Consequences

- The transition ends when it is judged that no meaningful number of clients
  still poll the dist endpoint. Retiring the dist repo — dropping the mirror
  step, `DIST_REPO_TOKEN`, and the dist README republish — is a **separate**
  future decision and is explicitly **not** taken here.
- `DIST_REPO_TOKEN` remains a required CI secret for as long as the mirror runs.
  A public repo makes the "no key material in the tree" rule more load-bearing,
  not less (ADR-0003).
- Documentation that described the dist repo as *the* release channel is updated
  to name the main repo as primary and the dist repo as a temporary mirror:
  `README.md`, `docs/dist-README.md`, `CONTEXT.md`'s "Distribution artifact"
  glossary term, and the "Releases" / "Project status" sections of `AGENTS.md`.
- `docs/plan-updates-and-providers.md` is a completed historical record and is
  left as-is; where it says releases go to the dist repo, that was true when
  written. This ADR is the current decision.
