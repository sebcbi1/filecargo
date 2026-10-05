# Implementation Plan: `remote-fs` module

> Spec: [SPEC-remote-fs.md](../../SPEC-remote-fs.md) · Tasks: [todo.md](todo.md) · Status: **awaiting review** · 2026-10-05
> Starts now (depends only on `config`, which is done).

## Overview
`crates/filecargo-remote-fs` in three phases:
1. the trait, `RootedFs` and the shared contract suite, plus the Docker test servers (highest setup risk)
2. SFTP end to end: connect, host keys, every auth method, filesystem, shell channel
3. FTP, then FTPS

Each phase ends with the contract suite green on the new backend. Phase 1 unblocks `transfer`
and phase 2 unblocks `terminal`.

## Architecture Decisions
| Decision | Rationale |
|---|---|
| **`async-trait` object-safe `RemoteFs`** | Backend chosen at runtime; test doubles (`FlakyFs`, fakes) live in other crates. The boxing cost is irrelevant next to network I/O. |
| **Push/pull transfers** (`download(path, offset, sink)`, `upload(path, offset, source)`) instead of returned streams | The backend controls finalization (FTP `finish()`, SFTP `close()`), so callers can't forget it. Cancel = drop. |
| **One generic contract suite**, written once as a macro instantiated per backend | All backends are held to identical semantics. `RootedFs` runs it on every `cargo test`. |
| **`Prompter` + `SessionTrust` injected through `ConnectContext`** | No UI types in this crate; the scripted test prompter makes every prompt path testable. The shared in-memory trust stops worker connections re-prompting. |
| **FTP: one control connection per `RemoteFs`, behind `tokio::sync::Mutex`**; a cancelled transfer flips a `broken` flag | Matches the protocol's one-transfer-at-a-time rule; makes "cancel ⇒ Disconnected" deterministic. |
| **Own MLSD parser, library LIST parsers with name-only fallback** | The library's MLSD parser rejects real-world lines (verified); entries must never vanish. |
| **TLS: `Pinning` verifier wrapping `rustls-platform-verifier`**, a two-pass handshake for prompts | rustls verification is synchronous; record the failure, prompt asynchronously, reconnect with the pin. |
| **known_hosts: russh helpers; filecargo writes only `<data>/known_hosts`** | Never modifies the user's OpenSSH files. Parser limits only cost an extra prompt. |
| **Test servers in Docker** (OpenSSH image + an in-repo Alpine ProFTPD image with 3 configs) | Reproducible locally (OrbStack) and on Linux CI; covers explicit, implicit and reuse-required FTPS. |

### Dependencies (new, pinned in `[workspace.dependencies]`)
`async-trait`, `tokio` (net, io-util, sync, time, rt, macros, fs), `russh =0.64.1` (try `default-features = false` + `ring`),
`russh-sftp 3.0.1`, `suppaftp 12.1.1` (`tokio-rustls-ring`, `deprecated`), `rustls 0.23`,
`rustls-platform-verifier 0.7.1`, `x509-parser 0.18`, `sha2`, `base64`, `tracing`;
dev: `tempfile`, `proptest`.

## Dependency Graph
```
T1 types + trait ── T2 local + RootedFs + contract suite ─┐
T3 docker servers + CI integration job ───────────────────┤   Checkpoint A → transfer may start
                                                          ├─ T4 SFTP connect + host keys ── T5 SFTP auth ── T6 SFTP fs ── T7 shell
                                                          │                                       Checkpoint B → terminal may start
                                                          └─ T8 FTP connection + listing + ops ── T9 FTP transfers/resume/cancel
                                                                                                   └─ T10 FTPS ── T11 active + redaction + final
```

## Task List
### Phase 1: Foundation
- [x] T1: Crate, `RemotePath` (+ proptest), `Entry`, `FsError`, `Capabilities`, `Progress`, `RemoteFs` with `remove_all` (M)
- [ ] T2: `local::read_dir` / `stat`, `RootedFs`, contract suite macro, green on `RootedFs` (M)
- [x] T3: Docker test servers (OpenSSH + ProFTPD image, 3 configs, keys/certs), `cargo it` alias, CI `integration` job (M)
### Checkpoint A: trait and contract suite reviewed; containers healthy locally and in CI
### Phase 2: SFTP
- [x] T4: Connect plumbing (`ConnectContext`, `Prompter`, `SessionTrust`, `Session`, `ConnectError`), SFTP transport + host-key verification (M)
- [x] T5: SFTP auth: password / keyboard-interactive with lookup order, remember and retry-once; key file incl. encrypted; agent (M)
- [x] T6: SFTP `RemoteFs` over russh-sftp; contract suite green on docker `sftp`; keepalive (M)
- [ ] T7: `ShellOpener` / `ShellChannel` with a draining pump and resize (S)
### Checkpoint B: AC3, AC4, AC7, AC8 + contract (SFTP) green
### Phase 3: FTP / FTPS
- [ ] T8: FTP connection, login, `TYPE I`, FEAT/UTF8, MLSD parser + LIST fallback, metadata ops, serialized control connection (M)
- [ ] T9: FTP download/upload with resume (`REST` / `APPE`), cancel ⇒ broken, NOOP keepalive, redacted command log (M)
- [ ] T10: FTPS explicit + implicit: pinning verifier, cert prompt and pinning file, data-channel TLS, 522/534 ⇒ `TlsSessionReuseRequired` (M)
- [ ] T11: Active mode (Linux CI), whole-run log redaction test, final contract run on all backends, coverage (S)
### Checkpoint C: all 10 AC green, coverage ≥ 80 % (excluding Windows-only agent code), human review

## AC Traceability
| AC | Task | AC | Task |
|---|---|---|---|
| 1 | T1 | 6 | T10 |
| 2 | T2 (RootedFs), T6 (SFTP), T8–T10 (FTP family) | 7 | T5 |
| 3 | T5 | 8 | T7 |
| 4 | T4 | 9 | T9 |
| 5 | T10 | 10 | T11 |

## Risks and Mitigations
| Risk | Impact | Mitigation |
|---|---|---|
| FTPS servers requiring TLS session reuse (vsftpd default) | High | Spec AC6 accepts success **or** the clear error; the shared `ClientConfig` gives resumption a chance; a TLS 1.2-only retry is a follow-up if the test shows reuse failing. |
| aws-lc-rs (russh default) needs CMake/NASM on Windows CI | Med | T4 tries russh with `ring` only; fallback: install the build deps in the CI job. |
| russh/russh-sftp pre-1.0 churn | Med | Exact pins; all russh types are confined to `sftp/` modules behind our own types. |
| Docker on macOS (OrbStack currently stopped), active-mode NAT | Med | Integration tests are opt-in (`cargo it`); active mode is Linux-CI-only by design; T3 documents `orb start`. |
| User's `~/.ssh/known_hosts` uses hashed or wildcard entries russh can't match | Low | Falls back to the unknown-key prompt (safe). T4 checks hashed entries; matching them is a follow-up if needed. |
| Contract test on 10,000 entries is slow over FTP in CI | Low | Entry count is a const, 10,000 locally / 2,000 in CI via env. |

## Open Questions
None. Owner/group names (only via SFTP `longname` parsing) are deferred: `Entry.owner/group` stay `None` in v1.

## Hand-off Notes
_Appended per task during implementation._

### T1 Types and trait (done)
- `RemotePath` normalizes (`//`, `.`, trailing `/`, `a/../b`) and rejects relative paths, NUL and a `..` above the root; `join` takes one component only. `NoProgress` added next to `Progress`. `remove_all` never follows symlinks.

### T2 Local + RootedFs + contract suite (done)
- `RootedFs::new(root)` canonicalizes the root. Every op resolves paths under it; an existing parent chain (and the final component for ops that follow links: list, download, upload, chmod, set_modified) must stay inside the root, else `PermissionDenied`.
- `tests/contract.rs`: scenarios are plain async fns; `contract_suite!(module, setup_fn)` instantiates all 17 for a backend. New backends add one fixture fn + one macro line. `FILECARGO_CONTRACT_ENTRIES` overrides the 10,000-entry listing. `tests/support/mod.rs` holds `sample_bytes` / `sha256_hex`.
- `RootedFs` reports `Protocol::Sftp` (the enum has no local variant); it is a test double only.

### T3 Docker test servers (done)
- **Deviation:** vsftpd was replaced by **ProFTPD** (Alpine 3.21, `proftpd-mod_tls`). vsftpd 3.0.5 (Alpine) and 3.0.3 (Debian bookworm) both lose their listener after the first completed TLS session on this host (exit 139 / dropped connections), however it is configured. Spec table updated.
- ProFTPD: explicit = `TLSRequired off` + `NoSessionReuseRequired`; implicit = `UseImplicitSSL` (the option `UseImplicit` does not exist); reuse server = default (session reuse required). `ftps-reuse` is only *verified healthy*; whether it really rejects suppaftp is checked in T10.
- Healthchecks use `nc -w1 127.0.0.1 <port>` (busybox `nc` has no `-z`, and `localhost` resolves to ::1).
- vsftpd config files must be root-owned: bind-mounting host files makes them exit 2; bake configs into the image.
- `.cargo/config.toml` alias `it`; CI `integration` job added and `actionlint` clean (not yet run on GitHub).

### T4 Connect plumbing + SSH transport + host keys (done)
- `ConnectContext::new(paths, secrets, prompter)` plus the extra pub field **`user_known_hosts: Option<PathBuf>`** (defaults to `~/.ssh/known_hosts`; tests point it at a temp file). Not in the spec's struct sketch; spec updated alongside.
- `sftp::connect_ssh(site, ctx) -> SshConnection` does TCP + handshake + host-key policy, **no auth yet** (T5). `Session` / `SessionInfo` / `ShellOpener` arrive with T6/T7, and `connect()` dispatch with T6.
- russh is built with features `ring`, `rsa`, `flate2` and **no** aws-lc-rs (verified with `cargo tree -i aws-lc-rs`); `rsa` is kept so RSA host/user keys work.
- `timeout_secs` bounds TCP + handshake but pauses while the host-key prompt is open (flag set by the handler).
- Hashed `|1|` known_hosts entries **are** matched by russh (HMAC-SHA1); wildcards / `@cert-authority` markers are not, and an unparsable line only costs a prompt. russh's `learn_known_hosts_path` starts a fresh file with a blank line (harmless).
- Verified: 7 unit tests (policy with a scripted prompter) + 5 integration tests against docker `sftp` (unknown → prompt, trust always persists and the user's file stays byte-identical, once, reject, changed key refused with no prompt, refused port).
- `tests/support` gained `TestPrompter` (scripted decisions/answers, records prompts).

### T5 SFTP authentication (done)
- `sftp::authenticate(&mut SshConnection, &Site, &ConnectContext)`. `credentials.rs` (shared with FTP later): `obtain` (attempt 0: SessionTrust → keychain if the site remembers → prompt; attempt 1: always prompt with `retry: true`), `succeeded` (typed secret goes to SessionTrust, and to the keychain only if the site *and* the user's `remember` allow), `failed` (forget the in-memory copy; the keychain entry is only replaced on success).
- Password: `password`, then `keyboard-interactive` with the same secret **only if** the server lists it as remaining. Multi-prompt / later rounds go to `Prompter::credential(KeyboardInteractive)`; max 5 rounds. A failed round costs 1–2 server-side auth failures; with two rounds that stays under OpenSSH's `MaxAuthTries` of 6.
- Key file: `~` expansion, `KeyIsEncrypted` → passphrase lookup, wrong passphrase retried once, then `ConnectError::KeyFile`. RSA hash via `best_supported_rsa_hash`.
- Agent: unix `connect_uds` / `SSH_AUTH_SOCK`; Windows named pipe `\\.\pipe\openssh-ssh-agent` then Pageant (**compile-checked only in CI**). Every plain-key identity is tried; certificates are skipped.
- **New pub field `ConnectContext::agent_socket: Option<PathBuf>`** so tests can use their own agent (the crate forbids `unsafe`, so `set_var` is out). The agent test spawns `ssh-agent`, loads a 0600 copy of `id_plain`, and skips itself when `ssh-agent` is missing.

### T6 SFTP filesystem (done)
- `sftp::open(site, ctx) -> SftpFs` (connect + auth + subsystem); `connect(site, ctx) -> Session { fs, info }` dispatches on protocol (FTP family returns `Unsupported` until T8). `SessionInfo { protocol, banner, tls, home }`. The `shell` field of `Session` arrives with T7.
- Contract suite instantiated with `contract_suite!(sftp, sftp_fixture)` under `--features integration`: all 17 scenarios green at the full 10,000-entry listing (~19 s). Each test works in `<home>/fc-contract-<pid>-<n>`, removed on success.
- `rename` is plain SFTP `rename` (fails if the destination exists on OpenSSH). `transfer` must delete or use a different strategy when it wants an atomic replace; revisit then (the `posix-rename@openssh.com` extension is not exposed by russh-sftp's high-level API).
- OpenSSH answers "failure" for mkdir-on-existing and rmdir-on-non-empty; the backend disambiguates with a follow-up `stat` / `list` so callers get `AlreadyExists` / `DirectoryNotEmpty`.
- Errors from `File` reads/writes arrive as `io::Error` wrapping the sftp error; `map_io` downcasts it to keep `NotFound` / `PermissionDenied` / `Timeout`.
- `list` order: russh-sftp reverses the READDIR chunk order, so "server order" holds only within a chunk. Owner/group stay `None`.
- Throughput over loopback docker with defaults (16 in-flight requests, 32 KiB writes): ~54 MB/s up, ~37 MB/s down for 20 MB. Not tuned further; revisit if `transfer` shows a bottleneck.
