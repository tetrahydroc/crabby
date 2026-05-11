# Changelog

Per-release changelog files, one per version. Written by
`.github/scripts/prepare-release.sh` during the development -> master
release-prep workflow.

Filename convention: `<version>.md`, e.g. `0.1.5004.md`. Patch is a
monotonic global counter so every filename is unique forever; no
collision risk between major versions.

## Why per-file rather than one CHANGELOG.md

- Each release PR commits exactly one new file. Merge conflicts on a
  shared CHANGELOG.md across concurrent PRs disappear (the prepare-
  release flow can't really produce them under normal use, but it's
  one less moving piece).
- `release.yml` reads only the current version's file for the GitHub
  Release body, no parsing of a multi-version master document needed.
- Historical entries stay immutable. A stale changelog line can't get
  edited by a later release accidentally.

## Aggregating

A top-level CHANGELOG.md (if/when we want one) is just
`cat .github/changelog/*.md` in reverse order. Worth adding when there
are enough releases to warrant the rollup; not needed for alpha.
