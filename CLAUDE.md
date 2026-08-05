# CLAUDE.md

## Versioning

Do **not** bump the version on a feature branch. Versions are SemVer and are
set once, on `main`, in a dedicated release commit that is then annotated-tagged
`v<version>` — that tag is what builds and publishes the Windows assets. The
workspace `Cargo.toml` is the single source of truth; do not add version copies
elsewhere, and run `npm run check-versions` after a bump. See "Releases" in
AGENTS.md for the full sequence and for resolving version conflicts on older
branches.

## Frontend changes

`npm run build` succeeding does **not** mean the UI renders — Svelte 5 throws
some errors only at runtime, and a mid-render throw leaves the old DOM on
screen instead of reporting anything. Verify with:

```sh
npm i -D jsdom --no-save   # not a package.json dep on purpose; see AGENTS.md
npm run smoke-mount
```

Do not claim a frontend fix is verified on the strength of a clean build.
See "Build and test" in AGENTS.md for the specific traps (`structuredClone` on
a `$state` proxy, state writes inside `{@const}`/`$derived`).
