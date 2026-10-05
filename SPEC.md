# Spec: filecargo

> Status: **approved** (open questions resolved 2026-10-05) · Last updated: 2026-10-05
> This file is the project-wide spec and the **index** of module specs (see Capability Map).

## Objective

A FileZilla-style **FTP / FTPS / SFTP client** written in Rust, for use on the developer's own
machine, with **two front-ends over one shared core**:

- `filecargo-tui`: terminal UI (ratatui), keyboard-driven, works over SSH
- `filecargo`: native GUI (gpui + gpui-kit)

Servers, folders and settings are shared: a site added in one UI exists in the other.

### Layout (both front-ends)

```
┌ Servers ───────┬ Local: ~/projects ──────────┬ Remote: /var/www ───────────┐
│ ▾ Work         │ Name        Size   Modified │ Name        Size   Modified │
│   ● prod-web   │ ..                          │ ..                          │
│     staging    │ src/              10-01     │ html/              09-30    │
│ ▸ Personal     │ README.md   2.1K  10-01     │ index.php   4.3K   09-30    │
├────────────────┴─────────────────────────────┴─────────────────────────────┤
│ [Queue 3] [Completed] [Failed 1] [Log] [Terminal]                          │
│ ↑ README.md → /var/www/README.md      42%   1.2 MB/s   00:03 left          │
└────────────────────────────────────────────────────────────────────────────┘
```
- **Left**: server tree, sites organized in nested folders.
- **Center/right**: local and remote file panes.
- **Bottom tabs**: Queue (pending + active), Completed, Failed, Log, and Terminal (SFTP sessions only; an interactive shell on the same SSH connection).

### v1 scope (decided 2026-10-05)

| Area | Decision |
|---|---|
| Protocols | SFTP, plain FTP (passive + active), FTPS explicit and implicit |
| Auth | Password / keyboard-interactive, private key (incl. passphrase), ssh-agent, anonymous FTP |
| Secrets | OS keychain only; no secrets in any file |
| Platforms | macOS, Linux, Windows |
| Sessions | One connected server at a time (connecting elsewhere replaces it); queued transfers use their own connections |
| Queue | Persisted across restarts (pending + failed restored; interrupted active items return to pending) |
| File ops | Upload/download (recursive), resume, conflict rules (ask / overwrite / overwrite-if-newer / resume / skip / rename), remote rename / delete / mkdir / chmod |
| Import | FileZilla `sitemanager.xml` |
| Shipping | Two binaries: `filecargo` (GUI), `filecargo-tui`; building the TUI never compiles gpui |
| **Out of v1** | Remote file editing, directory sync/compare, multiple simultaneous sessions, proxies, speed limits, S3/WebDAV, live config file watching |

## Capability Map (approved 2026-10-05)

| Module id | Responsibility | Depends on | Spec | Status |
|---|---|---|---|---|
| `config` | Server tree (folders + sites), settings, TOML persistence, keychain secrets, FileZilla import | — | [SPEC-config.md](SPEC-config.md) | approved; planning |
| `remote-fs` | Async filesystem trait (list, stat, ranged read/write, rename, delete, mkdir, chmod) + **local**, **FTP/FTPS**, **SFTP** backends; connect, auth, host-key / TLS-cert verification | `config` | — | not started |
| `transfer` | Queue engine: pending/active/completed/failed, N concurrent workers, recursion, resume, conflict rules, progress events, queue persistence | `remote-fs`, `config` | — | not started |
| `terminal` | PTY shell channel on the SFTP session's SSH connection; VT parsing into a screen grid both UIs render | `remote-fs` | — | not started |
| `app-core` | UI-agnostic app state + commands: session, pane state, server-tree ops, log bus, event stream: the single API both front-ends drive | `config`, `remote-fs`, `transfer`, `terminal` | — | not started |
| `tui` | ratatui front-end → bin `filecargo-tui` | `app-core` | — | not started |
| `gui` | gpui front-end → bin `filecargo` | `app-core` | — | not started |

**Build order:** `config` → `remote-fs` → `transfer`, `terminal` (parallel) → `app-core` → `tui` → `gui`

Each module runs Specify → Plan → Tasks → Implement in order. Contracts between modules live
in the **provider** module's spec. Module ids are stable; never rename them.

## Tech Stack

Versions verified on crates.io on 2026-10-05. Crates marked † are pinned **exactly** (`=x.y.z`) because they ship weekly breaking releases.

| Layer | Crate | Version | Notes |
|---|---|---|---|
| Language | Rust, edition 2024 | MSRV **1.92** | required by gpui-kit; declared as `rust-version` in workspace `Cargo.toml`; toolchain provided by `mise.toml` (`rust = "stable"`) |
| Async runtime | `tokio` | 1.x | runs inside the core; UIs reach it via channels |
| SSH + SFTP | `russh` † + `russh-sftp` | 0.64.1 / 3.0.1 | pure Rust; one connection carries SFTP + PTY shell channels; agent + known_hosts support |
| SSH config | `ssh2-config` | 0.8.1 | read `~/.ssh/config` host aliases/identity files (nice-to-have) |
| FTP / FTPS | `suppaftp` (`tokio-rustls-ring`, + `deprecated` for implicit FTPS) | 12.1.1 | one transfer per control connection → pool per worker |
| Terminal emulation | `vt100` (+ `tui-term` in TUI) | 0.16.2 / 0.3.4 | one parser in `terminal`; TUI renders via `tui-term`, GUI via a custom gpui element over `vt100::Screen` cells |
| Secrets | `keyring-core` + native stores | 1.0 | macOS Keychain, Windows Credential Manager, Linux Secret Service |
| Config | `etcetera` + `toml` + `serde` | 0.11.0 / 1.1.6 / 1 | `directories` is archived; not used |
| Errors / logs | `thiserror` (libs), `anyhow` (bins), `tracing` | latest at scaffold | |
| TUI | `ratatui` + `crossterm` + `tui-tree-widget` | 0.30.2 / 0.29.0 / 0.24.1 | |
| GUI | `gpui-kit` † (re-exports GPUI as `gpui-pre =0.3.8`) | 0.7.1 | **never** also depend on crates.io `gpui` or a zed git checkout (type mismatch) |
| GUI ↔ tokio | gpui_tokio-style bridge (≈80 lines, copied pattern from Zed) | — | tokio runtime as a gpui Global; tasks abort on drop |
| Test-only | `tempfile`, `proptest`, `insta` | latest at scaffold | |

Prior art used as **reference only** (not dependencies): termscp, `remotefs-*` crates (no PTY over the same SSH session; young 1.0 rewrite), Zed's `terminal_view`, `gpui-terminal`.

### Known stack risks
- **suppaftp #93**: the FTPS data connection does not reuse the control connection's TLS session. Servers that require session reuse (vsftpd `require_ssl_reuse=YES`, which is its default; FileZilla Server) reject transfers. **v1 decision:** detect the rejection and fail with a clear, actionable error ("server requires TLS session reuse, not supported yet"). No fork or patch in v1. Main test servers run with `require_ssl_reuse=NO`; one extra server with `YES` asserts the error.
- **gpui / gpui-kit churn**: pre-1.0, weekly releases with breaking changes. Pin exactly, bump deliberately (ask first).
- **GUI platform deps**: macOS 15+ and Xcode CLT; Linux needs Vulkan + wayland/x11 dev packages; Windows needs MSVC + CMake.
- **gpui-kit DataTable** has single selection only. Multi-select of file rows may need a custom list in the GUI pane (decide in `SPEC-gui`).

## Commands

```bash
# one-time
mise install                             # rust stable (≥ 1.92 enforced by rust-version in Cargo.toml)
docker compose -f tests/docker/compose.yml up -d --wait   # FTP/FTPS/SFTP test servers

# everyday (default-members exclude filecargo-gui → no gpui compile)
cargo build
cargo test
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings

# integration tests against the docker servers
cargo it        # alias in .cargo/config.toml → test -p <remote-fs,transfer,terminal> with each crate's `integration` feature

# run
cargo run -p filecargo-tui                       # TUI
cargo run -p filecargo-gui                       # GUI (bin: filecargo)
FILECARGO_CONFIG_DIR=/tmp/fc cargo run -p filecargo-tui   # isolated config

# full workspace incl. GUI
cargo build --workspace && cargo test --workspace
cargo build --release -p filecargo-gui -p filecargo-tui
```

## Project Structure

```
filecargo/
├── Cargo.toml                 # workspace; rust-version = "1.92"; [workspace.dependencies] holds every version; default-members excludes gui
├── mise.toml                  # toolchain: rust stable
├── .cargo/config.toml         # aliases (`cargo it`)
├── .github/workflows/ci.yml   # CI matrix (see Testing Strategy)
├── tasks/                     # plan.md + todo.md for the module in progress (agent-skills convention)
├── SPEC.md                    # this file (project spec + capability map)
├── SPEC-<module-id>.md        # one per module, written in build order
├── crates/
│   ├── filecargo-config/      # config
│   ├── filecargo-remote-fs/   # remote-fs
│   ├── filecargo-transfer/    # transfer
│   ├── filecargo-terminal/    # terminal
│   ├── filecargo-app-core/    # app-core
│   ├── filecargo-tui/         # tui → bin `filecargo-tui`
│   └── filecargo-gui/         # gui → bin `filecargo`
│       each crate: src/, tests/ (integration), tests/fixtures/
└── tests/docker/              # compose.yml, server configs, throwaway TLS certs + SSH keys (test-only)
```
Rule: crate dir = `filecargo-<module id>`. UI crates depend **only** on `filecargo-app-core`, which re-exports the types they need.

## Code Style

`rustfmt` defaults; clippy clean with `-D warnings`. Example of the target style:

```rust
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{path}:{line}: {msg}")]
    Parse { path: PathBuf, line: usize, msg: String },
    #[error("invalid server tree: {0}")]
    Invalid(#[from] ValidationError),
    #[error("i/o error on {path}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
}

impl ConfigStore {
    /// Applies `op`, reloading first if another instance changed the file since we read it.
    #[tracing::instrument(skip(self), fields(op = op.kind()))]
    pub fn apply(&mut self, op: TreeOp) -> Result<NodeId, ConfigError> {
        self.reload_if_changed()?;
        let mut next = self.tree.clone();
        let id = next.apply(op)?;                 // validate before anything touches disk
        write_atomic(&self.paths.servers(), &next.to_toml()?)?;
        self.tree = next;
        Ok(id)
    }
}
```

Conventions:
- **Errors**: one `thiserror` enum per library crate with context (path, host, op); `anyhow` only in the two binaries. No `unwrap`/`expect` outside tests, except documented startup invariants.
- **Paths**: remote paths are a `RemotePath` newtype (UTF-8, always `/`); local paths are `PathBuf`. Never convert one into the other with string formatting (Windows).
- **Async**: core crates are tokio-based. UI code never blocks on I/O: it sends commands to `app-core` and renders from events/snapshots.
- **Secrets**: `secrecy::SecretString`; never in `Debug`, logs, files or error messages.
- **Logging**: `tracing` everywhere; protocol traffic at `debug` (commands/responses, passwords redacted); `app-core` feeds a tracing layer into the Log tab.
- **Naming**: types `PascalCase` nouns (`QueueItem`, `SiteId`), fns `snake_case` verbs; no `utils` modules.
- **Unsafe**: none expected; any `unsafe` needs a `// SAFETY:` comment and approval.

## Testing Strategy

| Level | Where | What |
|---|---|---|
| Unit | `#[cfg(test)]` in each crate | pure logic: tree ops + validation, conflict rules, queue state machine, path handling, listing parsers |
| Property | `proptest` in `config`, `transfer` | random op sequences round-trip; queue invariants hold under random events |
| Backend contract | `filecargo-remote-fs/tests/contract.rs` | **one** generic suite run against every backend (local always; FTP, FTPS-explicit, FTPS-implicit, SFTP with `--features integration`) so all backends behave identically |
| Fault injection | `transfer` tests | fake backend that drops the connection at byte N → item fails → retry resumes → SHA-256 matches |
| Integration | `--features integration` + `tests/docker/` | real vsftpd (plain/explicit/implicit, active + passive) and OpenSSH server (password, key, encrypted key, agent, shell) |
| App | `app-core` tests | drive commands with fake backends, assert emitted events (connect → list → upload → completed) |
| TUI | `ratatui::backend::TestBackend` + `insta` snapshots | layout per state; key handling as unit tests on the input reducer |
| GUI | `gpui` test context for view wiring; manual smoke checklist for rendering | per release, on macOS + Linux + Windows |

**CI (GitHub Actions, `.github/workflows/ci.yml`)**, on push and PR:
- **check** job, matrix `macos-latest` / `ubuntu-latest` / `windows-latest`: `cargo fmt --all --check`, clippy (`-D warnings`), `cargo test` (default members, no GUI)
- **gui** job, same matrix (added when `filecargo-gui` exists): `cargo build -p filecargo-gui` with platform deps installed (Linux: Vulkan + wayland/x11 dev packages)
- **integration** job, `ubuntu-latest` only (needs Docker): compose up → `cargo it` (added when `remote-fs` lands)
- `Swatinem/rust-cache` for build caching; toolchain via `jdx/mise-action` so CI uses the same `mise.toml`

- All tests use temp dirs (`FILECARGO_CONFIG_DIR`) and `MemoryStore` secrets; **never** the real config dir, real keychain, or real servers.
- Coverage goal: ≥ 80 % lines on the five core crates (measured when `cargo-llvm-cov` is available; not a hard gate in v1). UI crates: behavior tests, no number.
- Bugs get a failing test first (Prove-It), then the fix.

## Boundaries

- **Always**
  - Run `cargo fmt --check`, clippy (`-D warnings`) and tests for touched crates before every commit
  - Validate before writing (config ops, conflict decisions, remote deletes)
  - Keep UI crates on `app-core` only
  - Update the module's SPEC when a decision changes, **before** the code
  - Put dependency versions only in `[workspace.dependencies]`
- **Ask first**
  - Adding any dependency not listed in a spec
  - Bumping `gpui-kit` / `russh` / `suppaftp`
  - Changing a cross-module public API
  - Changing any on-disk format (`servers.toml`, `settings.toml`, `queue.json`, `known_hosts`, `trusted_certs.toml`)
  - Installing tools (`cargo-nextest`, `cargo-llvm-cov`, …) or changing the toolchain
  - Adding CI config
  - `unsafe`
- **Never**
  - Store secrets in plaintext or log them
  - Auto-accept unknown/changed SSH host keys or invalid TLS certs
  - Overwrite a config file that failed to parse
  - Run tests against real servers or commit real credentials (test keys/certs in `tests/docker/` are throwaway)
  - Skip, `#[ignore]`, or delete a failing test to get green
  - Mention Claude in commit messages

## Success Criteria (v1 done)

1. **Shared config**: a folder + site created in the GUI shows up in the TUI on next launch (and vice versa). Running both at once loses no edits. Passwords exist only in the OS keychain.
2. **SFTP**: connect with password, key, encrypted key, and ssh-agent. An unknown host key prompts trust-once / always / reject; a **changed** host key blocks the connection.
3. **FTP/FTPS**: plain FTP (passive + active), explicit FTPS and implicit FTPS connect, list and transfer. An invalid cert prompts; it is never silently accepted.
4. **Browse & ops**: navigate local and remote panes; remote rename / delete (with confirmation) / mkdir / chmod; a remote directory with 10,000 entries lists without freezing the UI.
5. **Transfers**: recursive upload and download of a 1,000-file tree; Queue shows live progress, speed and ETA; finished items move to Completed and errors to Failed with a reason; `max_concurrent` is respected.
6. **Resume & conflicts**: killing the connection mid-file → Failed → Retry resumes from the partial offset and the SHA-256 matches. Every conflict rule behaves as specified.
7. **Persistence**: quit with pending/failed items → relaunch → they're restored; interrupted active items come back as pending.
8. **Terminal**: on SFTP sessions the Terminal tab opens a shell over the **same** SSH connection; resize propagates; `htop`/`vim` render correctly in both UIs.
9. **Log**: shows connection and protocol events, with no secrets (verified with a sentinel password).
10. **Import**: FileZilla `sitemanager.xml` imports folders and sites per `SPEC-config`.
11. **Platforms**: both binaries build, pass `cargo test --workspace`, and pass the smoke checklist on macOS, Linux and Windows.

## Resolved Decisions (2026-10-05)

1. **Terminal emulator**: `vt100` for v1 (TUI renders it via `tui-term`; GUI draws its cells in a custom gpui element). `alacritty_terminal` stays a later option if fidelity falls short.
2. **Toolchain**: MSRV declared as `rust-version = "1.92"` in the workspace `Cargo.toml`; toolchain comes from `mise.toml`. No `rust-toolchain.toml`.
3. **FTPS session reuse**: v1 ships a clear error message (see Known stack risks). No fork or patch.
4. **Plan location**: agent-skills convention, `tasks/plan.md` + `tasks/todo.md`.
5. **VCS + CI**: jj colocated with git (`jj git init --colocate`). GitHub Actions matrix on macOS / Linux / Windows from the first commit.

## Open Questions

None.
