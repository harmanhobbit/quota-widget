# Domain Docs

How engineering skills consume this repository's domain documentation.

## Before exploring, read these

- `CONTEXT.md` at the repository root, if it exists.
- `docs/adr/` entries relevant to the area being changed.

Missing domain documents are not errors. The repository uses a single-context layout: one root `CONTEXT.md` and repository-wide ADRs under `docs/adr/`.

## Use the glossary's vocabulary

When output names a domain concept, use terms from `CONTEXT.md`. If a needed concept is absent, note the gap for domain modelling rather than silently inventing terminology.

## Flag ADR conflicts

Surface any conflict with an existing ADR explicitly rather than silently overriding it.
