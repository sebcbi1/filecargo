# Implementation Plan: `remote-fs` module

> Spec: [SPEC-remote-fs.md](../../SPEC-remote-fs.md) · Tasks: [todo.md](todo.md) · Status: **complete; awaiting final human review** · 2026-10-05
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
- [x] T7: `ShellOpener` / `ShellChannel` with a draining pump and resize (S)
### Checkpoint B: AC3, AC4, AC7, AC8 + contract (SFTP) green
### Phase 3: FTP / FTPS
- [x] T8: FTP connection, login, `TYPE I`, FEAT/UTF8, MLSD parser + LIST fallback, metadata ops, serialized control connection (M)
- [x] T9: FTP download/upload with resume (`REST` / `APPE`), cancel ⇒ broken, NOOP keepalive, redacted command log (M)
- [x] T10: FTPS explicit + implicit: pinning verifier, cert prompt and pinning file, data-channel TLS, 522/534 ⇒ `TlsSessionReuseRequired` (M)
- [x] T11: Active mode (Linux CI), whole-run log redaction test, final contract run on all backends, coverage (S)
### Checkpoint C: all 10 AC green, coverage ≥ 80 % (excluding Windows-only agent code), human review — AC and coverage done; human review pending

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

### T7 Shell channel (done)  → Checkpoint B reached
- `ShellOpener::open(term, cols, rows)` (PTY + shell on the **same** `Handle`, waits for the server's pty/shell replies) returns `ShellChannel { input, output }`. `Session` now has `shell: Option<ShellOpener>` (`Some` for SFTP). Types are re-exported at the crate root.
- The pump task drains the channel unconditionally (stdout and stderr both become `ShellOutput::Data`), sends `Exit(status)` then `Closed` last. `ShellInput::Close` (or dropping the input sender) sends EOF + close.
- Verified: exit status 3 propagates; `stty size` follows `Resize`; 4 MB of unread output does not stall SFTP; **one TCP connection** is asserted client-side by counting this process's established sockets to :2222 through `/proc` (own test binary, Linux only), including after a second shell.
- Checkpoint B: AC3, AC4, AC7, AC8 and the SFTP contract suite are green (`cargo it`: 54 integration tests + 29 unit tests); fmt and clippy clean.

### T8 FTP connection, listing, metadata ops (done)
- `ftp::open(site, ctx) -> FtpFs`, wired into `connect()` for plain `Protocol::Ftp` (FTPS arrives with T10). `FtpFs` holds one `AsyncRustlsFtpStream` (the same type FTPS will use) behind a `tokio::sync::Mutex`; every call goes through `begin()` / `Op::finish()`: the connection is marked **broken** when a call starts and un-marked only when it completes without a connection-level error (`Disconnected` / `Timeout` / `TlsSessionReuseRequired`). A cancelled (dropped) call therefore leaves it broken and later calls return `Disconnected` (AC9 mechanism). Each control command also runs under `timeout_secs`.
- Login: anonymous (`anonymous` / `anonymous@`) or password via the shared `credentials` (same lookup/remember/retry-once rules as SFTP); key and agent are `AuthFailed`. After login: `TYPE I`, `FEAT` (keys upper-cased), `OPTS UTF8 ON` when advertised, EPSV when advertised else PASV + NAT workaround. A one-time-per-connect `warn` for plain FTP.
- Listing: MLSD when advertised (own parser `ftp/mlsd.rs`: slink targets, fractional times, `;` in names, case-insensitive facts, cdir/pdir skipped), else `LIST -a <path>` through the library's Unix/DOS parsers with a name-only `Other` entry for anything unparsable. Date arithmetic is in `ftp/time.rs` (no date crate).
- `stat`: `MLST` when advertised, else the parent listing; root is synthesised. `mkdir` / `rmdir` disambiguate ProFTPD's blanket `550` with a follow-up `stat` / `list`; `chmod` = `SITE CHMOD` (500/501/502/504 → `Unsupported`); `set_modified` = `MFMT` when advertised.
- Paths containing CR/LF are refused before they reach the wire (command injection guard).
- New docker service **`ftp-list`** (port 2123, ProFTPD `FactsAdvertise off`) forces the LIST fallback; `docker_ftp(port)` helper in `tests/support`.
- Verified: 23 parser unit tests; 8 integration tests (both listing paths, AlreadyExists / DirectoryNotEmpty / chmod / rename / unicode names, 12 concurrent tasks on one connection, injection guard, retry-once login, refused connect). `download` / `upload` are still `Unsupported` until T9.

### T9 FTP transfers (done)
- `download`: optional `REST` (`resume_transfer`) + `RETR` as a stream; `upload`: offset 0 → `STOR`, offset > 0 → `SIZE == offset` check then `APPE`. Every read/write is bounded by `timeout_secs`; `TransferStream::finish()` always runs on success so the server's verdict is read. A failing **sink/source** drops the stream and returns `LocalIo`; the library defers the 426 reply and the next command consumes it, so the connection stays usable (tested). Anything else mid-transfer (read/write error, timeout, dropped future) leaves the connection broken.
- Keepalive: a task per `FtpFs` (holds a `Weak`, aborted on `close` / drop) sends `NOOP` when the connection has been free for `keepalive_secs`; it skips a tick while a call holds the lock. Proven with a control experiment against `ftp-list`, whose ProFTPD has `TimeoutIdle 5`: with keepalive 1 s the connection survives 8 s idle, with keepalive 3600 s it is dropped.
- Logging on target `filecargo::protocol` at `debug`: `> VERB path` per command (never the PASS argument: login logs `> USER x` and `> PASS ***`), and `< code text` for error replies.
- **Test-server config** (`tests/docker/proftpd/*.conf`): `AllowOverwrite`, `AllowStoreRestart`, `AllowRetrieveRestart on` (ProFTPD refuses overwrite and `APPE`/`REST STOR` by default); passive ranges widened to 50 ports per server (30000-30049, 30050-30099, 30100-30149, 30150-30199): 10 ports ran dry through TIME_WAIT under parallel tests.
- **Contract-suite change:** the 10,000-entry scenario now creates directories with `mkdir` (control channel only); 10,000 uploads exhaust any FTP server's passive ports. And `RemoteFs::remove_all` now tries `remove_dir` before listing a child directory, so deleting 10,000 empty directories no longer opens 10,000 data connections (the first run of this took > 12 min).
- Contract suite is green on **rooted, sftp, ftp, ftp_list** (68 tests, ~23 s). `cargo it`: 46 unit + 104 integration tests.

### T10 FTPS (done)
- `tls/`: `PinningVerifier` (OS trust store through `rustls-platform-verifier`, ring provider, **no aws-lc-rs**) wraps every handshake: a pinned leaf fingerprint (hex SHA-256) is accepted outright, anything else goes to the platform verifier and a failure is **recorded** (reason read off the certificate with `x509-parser`: expired → self-signed → untrusted, `NotValidForName` → name mismatch). `SiteTls` = one `ClientConfig` per connection attempt, shared by the control and all data connections (so rustls' session cache can resume the control session on data connections). `RecordingConnector` is our own `AsyncTlsConnector` (tokio-rustls) that records the negotiated version/cipher and whether each data handshake was a full one or a resumption.
- Flow (`ftp/login.rs::establish`): connect → on a recorded cert failure ask `Prompter::certificate` **once** → *Trust once* = `SessionTrust`, *Trust always* = `<data>/trusted_certs.toml` (`version = 1`, `[[cert]] host/port/sha256`; atomic write; **a file that does not parse is an error and is never overwritten**) → reconnect with that fingerprint accepted, before any password is sent. Reject → `CertificateRejected`. Rejection is not remembered. A changed server cert at a pinned host:port has a different fingerprint and prompts again (tested by planting a stale pin).
- Explicit: library `into_secure` (AUTH TLS, handshake, PBSZ 0, PROT P). Implicit: `connect_secure_implicit` + **our own `PBSZ 0` / `PROT P`** (the library does not send them in implicit mode). `SessionInfo.tls` = `TLS 1.3, TLS13_AES_256_GCM_SHA384` etc.
- **suppaftp ordering bug worked around** (`FtpFs::settle`): the library starts a data connection's TLS handshake *before* reading the server's reply to the command; a refused command (`550` missing file) never gets TLS, so the handshake fails with "tls handshake eof" and the real reply is left unread. After a `SecureError` on a data command we read that pending reply (2 s cap) and report it, so `NotFound` etc. survive on FTPS (found by the contract suite).
- **AC6, measured:** with the default shared `ClientConfig`, the **reuse-required server (ProFTPD default) works**: rustls resumes the control session on data connections, so list / upload / download all succeed (the suite prints `AC6 outcome … list: ok, mkdir: ok, upload: ok`). With resumption off the same server answers `425 Unable to build data connection: Operation not permitted` after a *full* data handshake, which maps to `FsError::TlsSessionReuseRequired` (also: 522 / 534, or a TLS failure before any resumption). Resumption can be turned off with the new pub field **`ConnectContext::tls_session_resumption`** (default `true`); it exists so the failure path is testable and as an escape hatch. The SPEC's "known stack risk" (suppaftp #93) is therefore narrower in practice than feared: servers that accept TLS 1.3 ticket / TLS 1.2 session resumption work; revisit with a real FileZilla Server / vsftpd if one is available.
- Contract suite is green on **rooted, sftp, ftp, ftp_list, ftps_explicit, ftps_implicit** (102 tests). Unit tests: pins (4), verifier (7, using throwaway certs in `tests/docker/certs`: `server.crt` self-signed, `expired.crt`, `leaf.crt` issued by a test CA whose key was discarded).

### T11 Active mode, redaction, final run (done)  → Checkpoint C reached (human review pending)
- **Active mode**: `Site.ftp_mode == Active` → `ftp.active_mode(timeout)` (EPRT/PORT). New docker service **`ftp-active`** (port 2124, ProFTPD `<Limit EPSV PASV> DenyAll`) so a working transfer there can only have been active; `tests/active_mode.rs` (Linux only) connects to the container's bridge IP and carries a **control experiment** (passive against the same server fails).
- **Redaction (AC10)**: `tests/redaction.rs` runs SFTP password, SFTP encrypted key, FTP, explicit and implicit FTPS (each starting with a *wrong* sentinel secret) plus a failed login, with TRACE logging and **every `log` record bridged** (`tracing-log`), then asserts none of 5 secrets appears and every `PASS` line is `PASS ***`. The first run **failed** (as designed): suppaftp traces `CC OUT: PASS <password>` through `log`. Fix: suppaftp is built with its **`no-log`** feature (`log/max_level_off`), which compiles out *all* `log` records of every dependency (russh's included). Our own `tracing` events on `filecargo::protocol` are unaffected. `app-core` must still never bridge `log` below `debug` (defence in depth).
- Final run (`cargo it`): 57 unit + 150 integration tests green; contract suite 102 (6 backends × 17); `cargo fmt --check` and `clippy --workspace --all-targets --features integration -D warnings` clean.
- **Coverage** (`cargo llvm-cov -p filecargo-remote-fs --features integration`): **86.9 % lines** (lowest: `sftp/fs.rs` 62 % error paths, `tls/connector.rs` 45 %). Windows-only agent code is not compiled on Linux, so it is not in the figure.

### Acceptance criteria → tests
| AC | Covered by |
|---|---|
| 1 `RemotePath` | `path::tests` incl. the proptest round trips |
| 2 contract suite | `tests/contract.rs`: 17 scenarios × {rooted, sftp, ftp, ftp_list, ftps_explicit, ftps_implicit} |
| 3 SFTP auth | `tests/sftp_auth.rs` (password, plain key, encrypted key + session trust, agent with a real `ssh-agent`) |
| 4 host keys | `sftp::host_keys::tests` + `tests/sftp_connect.rs` (user `known_hosts` byte-identical) |
| 5 FTPS certs | `tests/ftps.rs` (prompt, trust once / always, reject, changed cert, corrupt pin file) + `tls::*` unit tests |
| 6 reuse-required server | `tests/ftps.rs`: works with the shared session cache; with resumption off → exactly `TlsSessionReuseRequired` |
| 7 retry-once | `tests/sftp_auth.rs`, `tests/ftp_fs.rs`, `credentials::tests` |
| 8 shell | `tests/shell.rs`, `tests/shell_connection.rs` (one TCP connection, via `/proc`) |
| 9 cancel ⇒ broken | `tests/ftp_cancel.rs` |
| 10 no secrets in logs | `tests/redaction.rs` |

### Open points for the human review
- **CI is unverified on GitHub**: the `integration` job (and everything here) has only run locally; Windows-only agent code is compile-checked by CI alone.
- ProFTPD replaced vsftpd as the FTP test server (vsftpd's listener crashes after the first TLS session on this host); spec table updated. A real vsftpd / FileZilla Server has not been tested against, so the TLS-session-reuse behaviour (AC6) is verified against ProFTPD only.
- `transfer` will need an atomic-replace strategy (SFTP `rename` fails if the target exists) and should expect FTP `remove_all` to cost one data connection per non-empty directory.
- New public fields beyond the spec sketch: `ConnectContext::{user_known_hosts, agent_socket, tls_session_resumption}`; `Session` has `shell: Option<ShellOpener>`; `Prompter::certificate`.
