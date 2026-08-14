# Contributing to Quota Widget

Thanks for your interest. Quota Widget is a small system-tray widget for
**Windows 11 and Linux** that shows AI-provider quotas — Tauri 2 (Rust) +
Svelte 5, around 3,600 lines. It is small enough to read end to end, and doing
so is the fastest way to get oriented.

[`AGENTS.md`](AGENTS.md) is the full engineering guide (architecture, platform
constraints, release process). This file is the short version for contributors;
where the two overlap, `AGENTS.md` is authoritative.

## Layout

```
crates/quota-core/   pure Rust: config, providers, alerts, model. Holds the tests.
src-tauri/           Tauri shell: tray, poller, secrets, OAuth, IPC commands.
src/                 Svelte 5 frontend (App, Settings, ProviderCard, MiniSummary).
nix/package.nix      Linux packaging; also emits the .desktop entry.
```

`README.md` is user-facing and kept accurate: update it when behaviour changes,
including the platform-differences table and the Caveats section.

## Getting started

`crates/quota-core` is pure Rust and holds most of the logic and all the tests —
it builds and tests with a stock Rust toolchain and is your main feedback loop:

```sh
cargo test -p quota-core
```

Building `src-tauri` needs GTK/WebKit system libraries. The flake's dev shell
supplies them:

```sh
nix develop                       # or automatically via direnv + .envrc
nix develop -c cargo check --workspace
```

Without Nix you cannot compile `src-tauri`; the `pkg-config --libs gdk-3.0`
step in the `-sys` crates' build scripts fails on a bare checkout. That is
expected — lean on `quota-core`, and if you cannot build the Tauri crate in your
environment, say so in your PR rather than claiming the change is verified.

## Checks before you open a PR

Run what applies to your change; CI runs the same gates on Linux:

```sh
cargo fmt --all                                          # formatting is a CI gate
cargo clippy -p quota-core --all-targets -- -D warnings  # no warnings allowed
cargo test -p quota-core
npm run build                                            # Svelte frontend
npm run check-versions                                   # version consistency
npm i -D jsdom --no-save && npm run smoke-mount          # does the UI render?
```

Two notes that catch people out:

- **A clean `npm run build` does not mean the UI renders.** Svelte 5 has
  runtime-only failure modes that compile without a warning and throw during
  render. Always run `smoke-mount` for a frontend change; add any new top-level
  component to `CASES` in `scripts/smoke-mount.mjs`. See "Build and test" in
  `AGENTS.md` for the specific traps.
- **Prefer a `#[allow]` on a single item** over a crate-wide `#![allow(…)]`,
  and only where the lint is genuinely wrong. `-D warnings` means the gate
  stays useful for the next lint too.

## Branching, commits, and PRs

- **Never push to `main`.** Work on a feature branch cut from the latest
  `origin/main`, and open a PR targeting `main`.
- **Do not bump the version on a feature branch.** The version lives once in the
  workspace `Cargo.toml` and is set as a separate release step on `main` — see
  "Releases" in `AGENTS.md`. `npm run check-versions` enforces the single
  source of truth.
- **Commit at logical intervals** rather than one large commit at the end; keep
  each commit a coherent, reviewable unit whose checks pass.
- Match the surrounding code. Comments explain *why*, especially where a
  platform quirk forced a decision — preserve that reasoning in place.

## Reporting bugs and security issues

Functional bugs and feature requests go to the
[issue tracker](https://github.com/harmanhobbit/quota-widget/issues).

**Suspected vulnerabilities do not** — report those privately as described in
[`SECURITY.md`](SECURITY.md).

## License

By contributing, you agree that your contributions are licensed under the
project's [Apache-2.0](LICENSE) license.
