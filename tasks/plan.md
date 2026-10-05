# Implementation Plan: `config` module

> Spec: [SPEC-config.md](../SPEC-config.md) · Project rules: [SPEC.md](../SPEC.md) · Tasks: [todo.md](todo.md)
> Status: **complete; awaiting final human review** · Created 2026-10-05

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
- [x] `cargo build`, `cargo test`, fmt, clippy clean locally; CI file validated

### Phase 2: Server tree
- [x] T3: Add a root site and persist it (M)
- [x] T4: Folders + rename / move / duplicate / delete (M)
- [x] T5: Safe multi-instance writes + corrupt-file handling (M)

### Checkpoint B: Tree is durable
- [x] AC1–AC5 pass; proptest round-trip green at 1,000 cases; human review of the file format and API

### Phase 3: Settings and secrets
- [x] T6: Settings: defaults, validation, persistence (S)
- [x] T7: Secrets: store trait, keychain, cleanup cascade, no-leak test (M)

### Checkpoint C: Secrets
- [x] AC6, AC7 pass; manual keychain smoke run on macOS

### Phase 4: Import
- [x] T8: FileZilla `sitemanager.xml` parser + fixture (S)
- [x] T9: FileZilla import into the store (M)

### Checkpoint D: Module complete
- [x] All 9 acceptance criteria green (traceability below), fmt + clippy clean, CI green
- [x] Coverage ≥ 80 % lines on `filecargo-config`: **90.6 %** (`cargo llvm-cov --package filecargo-config`; lowest is `secrets.rs` 55 %, platform keychain code covered by the manual smoke run)
- [x] SPEC-config status → done; SPEC.md map updated; hand-off notes appended below
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

### T1 Workspace + `Paths` (done)
- Root `Cargo.toml`: resolver 3, edition 2024, `rust-version = "1.92"`, all versions in `[workspace.dependencies]`, `default-members = ["crates/filecargo-config"]` (add new non-GUI crates here), `unsafe_code = "forbid"`, clippy `unwrap_used`/`expect_used` = warn. `clippy.toml` allows both inside `#[test]`/`cfg(test)`. Integration-test helper fns need a file-level `#![allow(clippy::unwrap_used, clippy::expect_used)]`.
- `paths.rs`: `Paths::resolve()` (honors `FILECARGO_CONFIG_DIR`, else `etcetera::choose_base_strategy()` + `filecargo/`), `Paths::from_override(Option<PathBuf>)` is the pure constructor tests use.

### T2 CI (done)
- `.github/workflows/ci.yml`: `check` job on ubuntu/macos/windows (fmt, clippy `-D warnings`, `cargo test`), toolchain via `jdx/mise-action`. `actionlint` is not installed (needs approval); the first real run is the validation. Repo: private `sebcbi1/filecargo`, pushed with `jj git push --bookmark main`.

### T3 Sites + persistence (done)
- `model.rs` (ids, `Protocol`, `Auth`, `FtpMode`, `Site`, `Folder`), `tree.rs` (`ServerTree`, whole-tree `validate()`), `fsio.rs` (`read_optional`, `write_atomic`), `store.rs` (`ConfigStore`).
- **Deviation:** `Auth` serializes as a sub-table `[site.auth]` (toml crate output), not the inline table shown in the spec example. Format is otherwise as specified; golden test pins it.
- `ConfigStore::open(paths)` takes no `SecretStore` yet; T7 adds it (breaking change to the signature is expected).

### T4 Folders and ops (done)
- `TreeOp`: `AddFolder`, `AddSite`, `UpdateSite`, `Rename`, `Move { node, parent }`, `Duplicate`, `Delete`. `ServerTree::apply` returns a crate-private `Outcome { node, removed_sites, duplicated_from }`, which T7 will use for secret cleanup; `ConfigStore::apply` returns just the `NodeId`.
- Sites and folders share one sibling name space (case-insensitive, trimmed).
- Property test (`tests/tree_props.rs`) runs 256 cases by default (about 12 s); `PROPTEST_CASES=1000` takes about 50 s, because every accepted op fsyncs and the file is reopened.

### T5 Multi-instance + corrupt files (done)
- `ConfigStore::apply` = exclusive lock on `<config>/.lock` (`std::fs::File::lock`) → `reload()` if bytes differ → apply on clone → validate → atomic write. `reload()` is public.
- Corrupt file → `ConfigError::Parse { path, line, msg }`, newer version → `UnsupportedVersion`; neither is ever overwritten.
- **Deviation:** reset is the associated fn `ConfigStore::reset(&Paths) -> Result<Option<PathBuf>>` (spec updated), because `open` fails on a corrupt file.
- Mutation check: removing the lock makes `concurrent_writers_lose_no_operation` fail 3/3 runs.
- A rejected op still creates `<config>/` and `.lock` (the lock is taken before validation); `servers.toml` is untouched.

### T6 Settings (done)
- `settings.rs`: `Settings { transfers, connection, ui, log }` plus `ConflictRule` and `LogLevel`. Every struct has serde defaults, so a missing file or key falls back to defaults and unknown keys are ignored. `ConfigStore::settings()` and `update_settings(|s| ..)` use the same lock → reload → validate → atomic-write path as the tree. `reload()` now refreshes both files.
- Validation (`ValidationError::InvalidSetting { field, rule }`): `max_concurrent` 1..=10, `timeout_secs` > 0, `keepalive_secs` > 0. An invalid value in the *file* is an error on `open`, not a silent default.
- The file struct uses explicit fields, **not** `#[serde(flatten)]`: flatten discards the error span, so parse errors reported line 1 instead of the real line (caught by a test).
- A corrupt `settings.toml` makes `open` fail, and `ConfigStore::reset` only backs up `servers.toml`. Not needed yet; add a settings reset when a front-end needs it.

### T7 Secrets (done)
- `secrets.rs`: `SecretKey { Password(SiteId), Passphrase(SiteId) }` (account `site:<uuid>:password`), `SecretStore` trait (`Debug + Send + Sync`), `MemoryStore` (tests), `KeyringStore` (keyring-core, service `filecargo`), `UnavailableStore`, and `default_secret_store()`, which falls back to `UnavailableStore` with one warning when there is no keychain. `SecretString` and `ExposeSecret` are re-exported.
- **API change:** `ConfigStore::open(paths, secrets: Arc<dyn SecretStore>)`; `ConfigStore::secrets()` exposes the store. Integration tests open through `tests/common/mod.rs`.
- **Cascade, always after the file write succeeds; keychain failures are warnings, never errors:** `Delete` (sites and folder contents) removes both secret kinds; `UpdateSite` that turns `remember` / `remember_passphrase` off removes that secret; `Duplicate` copies both kinds to the new site.
- Platform stores: `apple-native-keyring-store` (feature `keychain`; file keychain, no entitlements needed), `windows-native-keyring-store`, `zbus-secret-service-keyring-store` (feature `crypto-rust`, so no system OpenSSL). All three are target-specific dependencies.
- `keyring-core` error mapping never formats variants that can carry secret bytes (`BadEncoding`, ...).
- Verified: leak test fails when a secret is logged (mutation check); `cargo run -p filecargo-config --example keyring_smoke` passes on the real macOS Keychain (service `filecargo-smoke`, deleted afterwards).
- `cargo update` was needed once: the local registry index was stale (`security-framework` 3.7). `Cargo.lock` is committed.

### T8 FileZilla parser (done)
- `import/filezilla.rs`: quick-xml → small element tree → `FzNode` (`Folder` / `Site`). Codes verified against FileZilla's source (`server.h`, via Debian sources): `ServerProtocol` FTP=0, SFTP=1, HTTP=2, FTPS(implicit)=3, FTPES(explicit)=4, HTTPS=5, INSECURE_FTP=6, S3=7, ... ; `LogonType` anonymous=0, normal=1, ask=2, interactive=3, account=4, key=5. `RemoteDir` = `CServerPath::GetSafePath` (`<type+1> <prefix-len> [<prefix> ](<len> <segment> )*`), decoded by `decode_remote_dir`.
- **Not verified against source** (FileZilla's forum and `ReadServerElement` were not reachable): exact folder-name storage and the `crypt` password format. The parser accepts a folder name as either text or a `<Name>` child, and treats every `Pass` encoding other than base64/none as unreadable (skipped, reported). A sample from a real `sitemanager.xml` confirmed the element names `Host Port Protocol Logontype User Pass PasvMode Name Comments LocalDir RemoteDir` and `<Pass encoding="base64">`; `Keyfile` is from search-result descriptions. **Worth one manual import of a real file before relying on it.**
- Passwords are wrapped in `Pw` (redacted `Debug`). Fixture `tests/fixtures/filezilla/sitemanager.xml` is hand-written with fake credentials.

### T9 Import into the store (done)
- `ConfigStore::import_filezilla(path, ImportOptions { import_passwords, folder_name }) -> Result<ImportReport, ImportError>`. Everything is planned in memory (`import::plan`), validated with the existing tree, and written once under the config lock; keychain writes happen after the file write and a failure is reported in `passwords_skipped` ("keychain error: ..."), not raised. A file that fails to parse, or a `servers.toml` that is corrupt, leaves everything untouched.
- Mapping: Ftp/InsecureFtp → `Ftp`; Normal/Account → `Password{remember:true}`; Ask/Interactive → `Password{remember:false}`; Key (SFTP only, needs a key file) → `KeyFile`; `MODE_ACTIVE` → `FtpMode::Active`; a port equal to the protocol default is stored as `None`. A readable password becomes the site's password (or key passphrase) secret only when `import_passwords` is set. Unsupported protocols/logons, no host, anonymous-on-SFTP are skipped with a reason.
- Duplicate names in one folder get ` (2)`, ` (3)`; the root folder gets the same suffix if the name is taken. Nothing is written when there is nothing to import.
- `default_filezilla_path()` returns FileZilla's own file location (path only, never read by code or tests).
- `ImportError { Io, Parse { path, line, msg }, Format, Config }`.

### Checkpoint D: acceptance criteria → tests
| AC | Test |
|---|---|
| 1 | `store::tests::fresh_dir_opens_empty_and_writes_nothing` |
| 2 | `tests/tree_props.rs` (1,000 cases verified; 256 by default) |
| 3 | `store::tests::invalid_sites_*`, `rejected_update_*`, `tests/tree_ops.rs` |
| 4 | `tests/store_concurrency.rs::stale_store_merges_*`, `concurrent_writers_lose_no_operation` |
| 5 | `tests/store_concurrency.rs::corrupt_file_reports_line_*`, `apply_after_external_corruption_*` |
| 6 | `tests/secrets.rs::deleting_a_folder_with_three_sites_removes_their_secrets` |
| 7 | `tests/secrets.rs::sentinel_secret_never_reaches_files_debug_output_or_logs` (mutation-checked) |
| 8 | `tests/import.rs::fixture_imports_to_the_exact_expected_tree_and_report`, parser tests |
| 9 | `Paths::from_override` + every test uses temp dirs and `MemoryStore` |

Totals: 69 tests, line coverage 90.6 %, fmt/clippy/actionlint clean.
