# CI-driven crates.io releases across the quarto-* repos

- Braid: epic `qy-j7jz8rxi`; children `qy-kkm4fm5o` (quarto-yaml),
  `qy-tbrwpken` (quarto-source-map), `qy-wz25mg0l` (quarto-error-reporting),
  `qy-gwe1qb79` (crates.io trusted-publishing config, Carlos)
- Repos: `posit-dev/quarto-yaml`, `posit-dev/quarto-source-map`,
  `posit-dev/quarto-error-reporting`
- Status: **approved 2026-07-30, in progress** (recommended design and
  open-question recommendations accepted as-is)

## Overview

Replace the manual workstation `cargo publish` with a uniform GitHub Actions
release pipeline in all three repos: merge a version-bump PR to `main` → CI
verifies, publishes to crates.io, tags `vX.Y.Z`, and creates a GitHub
Release. No long-lived tokens — crates.io Trusted Publishing (OIDC).

## Findings

1. **Current state.** All releases so far were manual `cargo publish` from
   the workstation (credentials in `~/.cargo/credentials.toml`). No release
   automation anywhere; the only pre-existing tag in any repo is
   `quarto-error-reporting v0.2.0` (manual), plus `quarto-yaml v0.1.1` from
   today. Published crates: quarto-yaml 0.1.1, quarto-yaml-validation 0.1.1,
   quarto-source-map 0.1.0, quarto-error-reporting 0.2.0.

2. **The repos are structurally uniform already.** Near-identical `ci.yml`
   in all three (same jobs; minor per-repo feature-flag differences).
   Layouts: quarto-yaml is a 2-crate lockstep workspace
   (quarto-yaml-validation depends on quarto-yaml, publish order matters);
   quarto-source-map is a single crate; quarto-error-reporting is a single
   publishable crate plus an `xtask` member with `publish = false`.

3. **Trusted Publishing is the sound auth mechanism.** crates.io supports
   GitHub Actions OIDC (GA since mid-2025). Per crate, an owner configures on
   crates.io: repository owner + name, workflow filename, and (optionally but
   recommended) a GitHub Actions environment name. The workflow exchanges its
   OIDC identity for a short-lived token via `rust-lang/crates-io-auth-action@v1`
   (verified: outputs `token`, auto-revoked at job end; needs
   `permissions: id-token: write`), consumed as `CARGO_REGISTRY_TOKEN`. No
   secret to store or rotate; publishing is cryptographically bound to the
   specific repo + workflow. Config is per-crate and owner-only — this is the
   one part Carlos must do in the crates.io UI (4 crates).

4. **`cargo publish --workspace` (Rust 1.90) is not resumable.** It orders
   members correctly but errors if any member's version already exists, so a
   partial failure (first crate published, second failed) can't be re-run.
   A small per-crate loop — topo order from `cargo metadata`, skip versions
   already on crates.io — is idempotent and identical across all three
   layouts. Chosen over the built-in flag for that reason.

5. **No pending releases to test with.** Both siblings have only README
   tweaks since their last release, and quarto-yaml just shipped 0.1.1. The
   workflow therefore sits dormant until the next real bump — so it needs a
   `workflow_dispatch` dry-run mode to be exercisable now.

## Design (recommended)

One `release.yml`, byte-identical across the three repos:

- **Triggers:** `push` to `main`, and `workflow_dispatch` with a `dry_run`
  boolean input.
- **Gate:** read the workspace version via `cargo metadata`; query crates.io
  for each publishable member; if every member already has that version, the
  run is a cheap no-op. A release is triggered *by merging a version bump* —
  no other push publishes anything.
- **Release job** (only when something is unpublished): runs in the
  `release` GitHub environment, `permissions: id-token: write,
  contents: write`, concurrency-grouped so releases serialize.
  1. `cargo test --locked` (fresh verification at the release commit).
  2. Mint a token with `rust-lang/crates-io-auth-action@v1`.
  3. For each publishable member in dependency order: skip if the version is
     already on crates.io, else `cargo publish -p <crate> --locked`.
     (Idempotent: a failed run is fixed by re-running the workflow.)
  4. `gh release create vX.Y.Z --generate-notes` — creates the git tag and
     the GitHub Release in one step, pointing at the released commit.
  - `dry_run` mode stops after `cargo publish --dry-run` for each member and
    skips auth, tagging, and the GitHub Release.

**The release process then becomes, uniformly:** open a PR bumping the
version (in quarto-yaml: both `[workspace.package] version` and the
intra-workspace `quarto-yaml` dep version) → merge → CI does the rest.

### Alternatives considered

- **Tag-push trigger** (`on: push: tags: v*`): keeps a manual tagging step,
  which is exactly what we want CI to own. Rejected.
- **release-plz** (automated release PRs, changelogs, publish on merge):
  more machinery and conventions than three small lockstep repos need; the
  version-bump PR is already our natural unit of release intent. Can
  revisit if the repo count grows.

## One-time setup

- **Carlos, on crates.io (owner-only, web UI)** — for each of quarto-yaml,
  quarto-yaml-validation, quarto-source-map, quarto-error-reporting:
  Settings → Trusted Publishing → New publisher: owner `posit-dev`,
  repository (the crate's repo), workflow `release.yml`, environment
  `release`. Existing API tokens keep working (fallback) until removed.
- **GitHub `release` environments** in the three repos — I can create these
  via `gh api` (falls back to a manual step if my token lacks admin).

## Plan

### Phase 1 — quarto-yaml (reference implementation)
- [x] Write `.github/workflows/release.yml` per the design.
      (Validated locally: actionlint clean; planner script tested against
      all three repos and against a synthetic unpublished bump.)
- [x] Create the `release` environment in the repo.
- [ ] PR, review, merge. (PR #7 open.)
- [ ] Validate via `workflow_dispatch` dry-run. (Requires the workflow on
      the default branch, i.e. after #7 merges.)

### Phase 2 — replicate to siblings
- [x] Same file + environment + PR in `quarto-source-map`.
      (quarto-source-map#1 open; environment created.)
- [x] Same file + environment + PR in `quarto-error-reporting`.
      (quarto-error-reporting#2 open; environment created.)
- [ ] Dry-run dispatch in each. (After the PRs merge; suggest merging
      quarto-yaml#7 and dry-running it first.)

### Phase 3 — trusted publishing config (Carlos)
- [ ] Configure the 4 crates on crates.io as above.

### Phase 4 — documentation and memory
- [x] Add a short "Releasing" section to each repo's README or CLAUDE.md
      describing the bump-PR process. (CLAUDE.md in quarto-yaml and
      quarto-error-reporting, README in quarto-source-map; in the PRs.)
- [ ] Update my project memory (release-process.md) to the new process.
      (After the pipeline is merged and dry-run-validated.)
- [ ] First real release through the pipeline (whenever one is next due)
      confirms end-to-end; until then dry-run coverage is the validation.

## Open questions

- Environment protection: the `release` environment can later get required
  reviewers (a human click before any publish). Start without protection?
  (Recommend: yes, start without; the version-bump-PR merge is already the
  human gate.)
- Should the workflow also verify the bump PR's version is a semver
  increase? (Recommend: no — crates.io rejects re-publishing existing
  versions, which covers the dangerous case.)
