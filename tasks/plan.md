# Implementation Plan: `config` module

> Spec: [SPEC-config.md](../SPEC-config.md) · Project rules: [SPEC.md](../SPEC.md) · Tasks: [todo.md](todo.md)
> Status: **awaiting review** · Created 2026-10-05

## Overview

Build `crates/filecargo-config`, the first crate of the workspace. It is a synchronous library
that owns the server tree, settings, keychain secrets and FileZilla import. This plan also
scaffolds the Cargo workspace and CI, because every later module builds on them.

Work is sliced vertically: each task adds one capability end-to-end (model → validation →
disk → test) instead of building all types first and all I/O later.

## Architecture Decisions

| Decision | Rationale |
|---|---|
| **Sync API, `&mut self` store** | Spec says no async. File I/O on a few KB of TOML is microseconds. `app-core` will wrap the store in a `Mutex` later. |
| **In memory: two `Vec`s (folders, sites)** that mirror the flat file | Same shape as the file means a trivial serde mapping. `children(parent)` is a filtered scan, O(n), fine for hundreds of sites. |
| **Alphabetical display order, folders first** (spec updated) | No manual ordering to persist. Removes `index` from `Move`. FileZilla's site manager also sorts. |
| **Lock → reload-if-changed → apply → validate → write** | Applying the op *after* the reload makes the multi-instance merge fall out naturally, with no replay logic. Change detection compares the stored bytes, so no hash crate is needed. |
| **Exclusive lock with `std::fs::File::lock`** on `<config>/.lock` | Stable since Rust 1.89, no dependency. Closes the time-of-check/time-of-use window between reload and rename. Locking a separate file avoids Windows mandatory-lock problems on the data file. |
| **Atomic write in std**: temp file in the same dir → `sync_all` → `fs::rename` | `rename` replaces the target on all three OSes. No dependency. |
| **Parse-error line numbers** from `toml::de::Error::span()` | Gives an accurate `path:line` without a custom parser. |
| **Secrets cleaned up after the file write succeeds** | The file is the source of truth. An orphaned keychain entry is harmless, but a site pointing at a deleted secret is a bug. |
| **`Paths` has a pure constructor** (`Paths::from_override(Option<PathBuf>)`) alongside `resolve()` | Tests never mutate process env vars, which are global and race under the parallel test runner. |
| **Import folder name supplied by the caller** (spec updated) | Keeps a date crate out of `config`. `app-core` formats the date. |
| **`reset()` backup suffix = Unix seconds** | Unique and sortable without a date crate. |

### Dependencies (pinned in `[workspace.dependencies]`, versions via `cargo add` at scaffold)
- Normal: `serde` (derive), `toml`, `etcetera`, `uuid` (v4, serde), `thiserror`, `tracing`,
  `secrecy`, `keyring-core` + per-OS native stores, `quick-xml`, `base64`
- Dev: `tempfile`, `proptest`, `tracing-subscriber` (captures logs for the no-leak test)

## Dependency Graph

```
T1 workspace + Paths ─┬─ T2 CI workflow (independent)
                      │
                      └─ T3 root site persists ── T4 folders + tree ops ── T5 multi-instance + corrupt files
                                                                                  │
                                                         T6 settings ─────────────┤ (reuse lock/atomic write)
                                                                                  │
                                                         T7 secrets ──────────────┤ (hooks into Delete/Update/Duplicate)
                                                                                  │
                         T8 FileZilla parser (needs only T1) ── T9 import into store (needs T5, T7)
```

## Task List (details in [todo.md](todo.md))

### Phase 1: Foundation
- [x] T1: Workspace scaffold + `Paths` (S)
- [x] T2: GitHub Actions CI, `check` job matrix (XS)

### Checkpoint A: Foundation
- [ ] `cargo build`, `cargo test`, fmt, clippy clean locally; CI file validated

### Phase 2: Server tree
- [x] T3: Add a root site and persist it (M)
- [x] T4: Folders + rename / move / duplicate / delete (M)
- [ ] T5: Safe multi-instance writes + corrupt-file handling (M)

### Checkpoint B: Tree is durable
- [ ] AC1–AC5 pass; proptest round-trip green at 1,000 cases; human review of the file format and API

### Phase 3: Settings and secrets
- [ ] T6: Settings: defaults, validation, persistence (S)
- [ ] T7: Secrets: store trait, keychain, cleanup cascade, no-leak test (M)

### Checkpoint C: Secrets
- [ ] AC6, AC7 pass; manual keychain smoke run on macOS

### Phase 4: Import
- [ ] T8: FileZilla `sitemanager.xml` parser + fixture (S)
- [ ] T9: FileZilla import into the store (M)

### Checkpoint D: Module complete
- [ ] All 9 acceptance criteria green (traceability below), fmt + clippy clean, CI green
- [ ] Coverage ≥ 80 % lines on `filecargo-config` (if the coverage tool is approved)
- [ ] SPEC-config status → done; SPEC.md map updated; hand-off notes appended below
- [ ] Human review → then write `SPEC-remote-fs.md`

## Acceptance-Criteria Traceability (SPEC-config)

| AC | Covered by |
|---|---|
| 1 Fresh dir → empty, nothing written | T3 |
| 2 Op round-trip (property test) | T4 |
| 3 Each validation rule rejects + writes nothing | T3 (site fields), T4 (tree rules) |
| 4 Two stores merge edits | T5 |
| 5 Corrupt file → line-numbered error, bytes unchanged | T5 |
| 6 Folder delete cascades sites + secrets | T7 |
| 7 Sentinel secret never in files / Debug / logs | T7 |
| 8 FileZilla fixture → exact tree + report | T8, T9 |
| 9 Env override honored; tests isolated | T1 (+ every test uses temp dirs + `MemoryStore`) |

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| `keyring-core` 1.0 API and per-OS store crate names not yet verified | Med | Start T7 by reading docs.rs/context7. Fallback: the `keyring` 4.2 umbrella crate (same backends). |
| FileZilla protocol/logon codes or `RemoteDir` encoding wrong | Med | T8 begins by verifying the enums and XML writer in FileZilla's source. Fixture is hand-written from that source. |
| `File::lock` / `rename` behave differently on Windows | Med | Lock a separate `.lock` file; Windows runs in the CI matrix from T2; T5 concurrency test runs on all three OSes. |
| CI can't run without a GitHub remote | Low | T2 validates locally; see Open Question 1. |
| `etcetera` paths differ from the spec on some OS | Low | T1 asserts the path shape per `cfg(target_os)`. |

## Open Questions

1. **GitHub remote**: none exists yet, so CI can't run. Should I create a **private** repo with `gh repo create` and push, or will you add the remote yourself?
2. **Commits**: one jj commit per task (`jj describe -m "…" && jj new`), conventional-commit style, no AI attribution. OK?
3. **Coverage tool**: OK to install `cargo-llvm-cov` at Checkpoint D? Otherwise the 80 % goal goes unmeasured for now.

## Hand-off Notes
_Appended per task during implementation: what changed, where, and any deviation from the spec._
