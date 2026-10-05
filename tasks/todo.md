# Tasks: `config` module

> Plan: [plan.md](plan.md) · Spec: [SPEC-config.md](../SPEC-config.md)
> Standing Definition of Done for every task: acceptance criteria met; new behavior has tests that
> fail without it; `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings`
> clean; no dead code or debug output; any spec deviation is written into the spec first.

---

## Phase 1: Foundation

### Task 1: Workspace scaffold + `Paths`

**Description:** Create the Cargo workspace (edition 2024, `rust-version = "1.92"`, resolver 3,
`[workspace.dependencies]`, `[workspace.lints]` for clippy, `default-members`) and the
`filecargo-config` crate with `Paths`, which resolves the config and data dirs through
`etcetera::choose_base_strategy()` and honors `FILECARGO_CONFIG_DIR`.

**Acceptance criteria:**
- [ ] `Paths::resolve()` returns `<base>/filecargo` config and data dirs (XDG on macOS/Linux, `%APPDATA%` on Windows)
- [ ] `Paths::from_override(Some(dir))` puts config **and** data under `dir`; tests use this, never `std::env::set_var`
- [ ] `Paths` exposes `servers()`, `settings()`, `lock()`, `known_hosts()`, `trusted_certs()`, `queue()`

**Verification:**
- [ ] `cargo build` and `cargo test -p filecargo-config` pass
- [ ] fmt + clippy clean

**Dependencies:** None
**Files:** `Cargo.toml`, `crates/filecargo-config/Cargo.toml`, `crates/filecargo-config/src/lib.rs`, `crates/filecargo-config/src/paths.rs`
**Scope:** S

---

### Task 2: GitHub Actions CI (`check` job)

**Description:** Add `.github/workflows/ci.yml` running on push and PR. The `check` job runs a
matrix of `ubuntu-latest`, `macos-latest` and `windows-latest`, with the toolchain from
`jdx/mise-action` and caching from `Swatinem/rust-cache`. Steps: fmt check → clippy `-D warnings` → `cargo test`. The `gui` and `integration` jobs are added later by their modules.

**Acceptance criteria:**
- [ ] Workflow uses the repo's `mise.toml` (no duplicated toolchain version)
- [ ] `fail-fast: false` so one OS failing still reports the others
- [ ] Least-privilege `permissions: contents: read`

**Verification:**
- [ ] Workflow YAML validated locally (`actionlint`; installing it needs approval at task time)
- [ ] After a push to a GitHub remote: all three matrix legs green

**Dependencies:** T1 (needs something to build)
**Files:** `.github/workflows/ci.yml`
**Scope:** XS

### Checkpoint A: Foundation
- [ ] Clean build + tests locally; CI validated (green once a remote exists)

---

## Phase 2: Server tree

### Task 3: Add a root site and persist it

**Description:** First end-to-end slice. Covers the model types (`SiteId`, `FolderId`,
`Protocol`, `Auth`, `FtpMode`, `Site`, `Folder`), `ServerTree` with read access (`site()`,
`children(None)`), `TreeOp::AddSite` and `TreeOp::UpdateSite`, site-field validation, the
`servers.toml` serde mapping, atomic write, and `ConfigStore::open` / `tree` / `apply`.

**Acceptance criteria:**
- [ ] AC1: a fresh dir opens with an empty tree and **no file is created** until the first `apply`
- [ ] A site added → store reopened from disk → identical `Site`. The serialized file matches the spec's format (golden string test).
- [ ] Rejected with **file bytes unchanged**: empty or whitespace name, empty host, port 0, `KeyFile`/`Agent` on FTP, `Anonymous` on SFTP

**Verification:**
- [ ] `cargo test -p filecargo-config`
- [ ] Manual: inspect a generated `servers.toml` for readability

**Dependencies:** T1
**Files:** `src/model.rs`, `src/tree.rs`, `src/store.rs`, `src/fsio.rs`, `src/error.rs`
**Scope:** M

---

### Task 4: Folders + rename / move / duplicate / delete

**Description:** Add `AddFolder`, `Rename`, `Move { node, parent }`, `Duplicate` and recursive
`Delete`, plus tree validation: case-insensitive sibling uniqueness, no move into self or a
descendant, and no dangling `parent`/`folder` ids on load. `children()` returns folders first,
then sites, each sorted alphabetically (case-insensitive). `Delete` returns the removed
`SiteId`s, which T7 uses for secret cleanup.

**Acceptance criteria:**
- [ ] AC2: proptest, where a random op sequence → save → reopen gives an identical tree, and invariants (no cycles, unique siblings, no dangling ids) hold after every op
- [ ] AC3, remaining rules: each rejected op leaves the file bytes unchanged
- [ ] `Duplicate` creates a new id named `"<name> (copy)"`, with `(copy 2)` and so on on collision

**Verification:**
- [ ] `cargo test -p filecargo-config` (proptest at 1,000 cases)

**Dependencies:** T3
**Files:** `src/tree.rs`, `src/model.rs`, `tests/tree_props.rs`
**Scope:** M

---

### Task 5: Safe multi-instance writes + corrupt-file handling

**Description:** Wrap every write in lock (`<config>/.lock`, `File::lock`) → reload-if-bytes-changed
→ apply → validate → atomic write. Map `toml` span errors to `ConfigError::Parse { path, line, msg }`.
Reject `version` > 1 with no write. Add `reload()` and `reset()`, which renames the file to
`servers.toml.bak-<unix-secs>` and starts empty.

**Acceptance criteria:**
- [ ] AC4: two stores on one dir; A adds X, then the stale B adds Y → the file holds X and Y. Also run as a two-thread stress test with 50 interleaved ops each, with no lost op.
- [ ] AC5: a corrupt file gives `Parse` with the correct line number, and the file bytes are unchanged after `open` and after an attempted `apply`
- [ ] `version = 2` → `UnsupportedVersion` error, no write; `reset()` keeps a backup and the store works afterwards

**Verification:**
- [ ] `cargo test -p filecargo-config` (and in CI on all three OSes, because locking and rename semantics differ)

**Dependencies:** T4
**Files:** `src/store.rs`, `src/fsio.rs`, `src/error.rs`, `tests/store_concurrency.rs`
**Scope:** M

### Checkpoint B: Tree is durable
- [ ] All tests pass; AC1–AC5 green
- [ ] **Human review**: file format, `TreeOp` API, error messages

---

## Phase 3: Settings and secrets

### Task 6: Settings

**Description:** A `Settings` struct with serde defaults matching the spec. A missing file or
missing keys fall back to defaults. Values are range-validated (`max_concurrent` 1..=10,
timeouts > 0, enum values). `update_settings(f)` goes through the same lock → reload → apply →
validate → atomic-write path as the tree.

**Acceptance criteria:**
- [ ] A missing file loads defaults; a partial file merges with defaults; unknown keys are ignored
- [ ] An out-of-range value is rejected with a field-named error and nothing is written
- [ ] update → reopen → same values

**Verification:**
- [ ] `cargo test -p filecargo-config`

**Dependencies:** T5
**Files:** `src/settings.rs`, `src/store.rs`, `src/lib.rs`
**Scope:** S

---

### Task 7: Secrets

**Description:** Start by verifying the `keyring-core` 1.0 API and per-OS store crates on
docs.rs/context7. Then implement `SecretKey`, the `SecretStore` trait, `MemoryStore`, and
`KeyringStore` (service `filecargo`, store chosen per `cfg(target_os)`, with "unavailable"
degrading to `remember = false` plus one warning). Hook cleanup into the store, always after the
file write succeeds: `Delete` (cascade), `UpdateSite` turning `remember` off, and `Duplicate`
copying secrets. All values are `SecretString`.

**Acceptance criteria:**
- [ ] AC6: deleting a folder with 3 sites removes all 3 sites and their secrets from `MemoryStore`
- [ ] AC7: a sentinel password never appears in `servers.toml`, `settings.toml`, the `{:?}` output of the store or its types, or captured `tracing` output
- [ ] A failed keychain delete is logged as a warning and the op still succeeds

**Verification:**
- [ ] `cargo test -p filecargo-config`
- [ ] Manual, macOS: `cargo run -p filecargo-config --example keyring_smoke` sets, gets and deletes a dummy secret under service `filecargo-smoke` in the real Keychain

**Dependencies:** T5 (T6 not required)
**Files:** `src/secrets.rs`, `src/store.rs`, `crates/filecargo-config/Cargo.toml`, `tests/secrets.rs`, `examples/keyring_smoke.rs`
**Scope:** M

### Checkpoint C: Secrets
- [ ] AC6 and AC7 green; keychain smoke run done on macOS

---

## Phase 4: Import

### Task 8: FileZilla parser + fixture

**Description:** Start by verifying FileZilla 3.x's `ServerProtocol` and `LogonType` enums, the
`PasvMode` strings, the `RemoteDir` segment encoding and the `<Pass encoding>` variants against
its source. Write the fixture by hand with fake credentials. Parse `sitemanager.xml` (quick-xml)
into an intermediate `FzNode` tree, base64-decoding passwords, flagging `crypt`, and decoding
`RemoteDir`.

**Acceptance criteria:**
- [ ] Fixture covers nested folders, SFTP with a key file, explicit FTPS, implicit FTPS, anonymous FTP, a base64 password, a crypt password, and an S3 site
- [ ] Parser output for the fixture equals a hand-written expected `FzNode` tree
- [ ] Malformed XML → `ImportError::Parse` naming the file

**Verification:**
- [ ] `cargo test -p filecargo-config import::`

**Dependencies:** T1 (independent of T3–T7)
**Files:** `src/import/mod.rs`, `src/import/filezilla.rs`, `tests/fixtures/filezilla/sitemanager.xml`, `crates/filecargo-config/Cargo.toml`
**Scope:** S

---

### Task 9: FileZilla import into the store

**Description:** Map `FzNode` to folders and sites per the spec. Create the root folder from
`ImportOptions.folder_name`, adding ` (2)` and so on on collision. Skip unsupported protocols
with a reason. When `import_passwords` is set, store base64 passwords in the `SecretStore`;
otherwise, and always for `crypt` entries, the password is skipped and noted. Build the whole
result in memory and write it **once**, setting secrets after the write. Also provide
`default_filezilla_path()`.

**Acceptance criteria:**
- [ ] AC8: the fixture imports to the exact expected tree and an `ImportReport` listing imported, skipped (with reason) and passwords-skipped
- [ ] Any failure (bad XML, validation error) → nothing written, no secrets set
- [ ] Importing twice gives `… (2)` folder naming and no validation error

**Verification:**
- [ ] `cargo test -p filecargo-config`

**Dependencies:** T5, T7, T8
**Files:** `src/import/mod.rs`, `src/store.rs`, `tests/import.rs`
**Scope:** M

### Checkpoint D: Module complete
- [ ] All 9 SPEC-config acceptance criteria green; fmt + clippy clean; CI green on all three OSes
- [ ] Coverage ≥ 80 % lines (if `cargo-llvm-cov` is approved)
- [ ] SPEC-config → done; SPEC.md map status updated; hand-off notes appended to plan.md
- [ ] **Human review** → next: write `SPEC-remote-fs.md`
