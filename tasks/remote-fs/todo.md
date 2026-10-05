# Tasks: `remote-fs`

> Plan: [plan.md](plan.md) · Spec: [SPEC-remote-fs.md](../../SPEC-remote-fs.md) · DoD as in [tasks/transfer/todo.md](../transfer/todo.md).
> Integration tests: `docker compose -f tests/docker/compose.yml up -d --wait && cargo it`. A scripted `TestPrompter` (`tests/support`) answers prompts and records what it was asked.

### T1: Types and trait (M)
`RemotePath` (parse / join / parent / file_name / normalization), `Entry`, `EntryKind`, `FsError` (+ `is_retryable`, user-facing `Display`), `Capabilities`, `Progress`, `RemoteFs` (async-trait) with the default `remove_all`.
- **Accept:** AC1 (rejections + proptest round trip); `remove_all` tested on an in-memory fake.
- **Verify:** `cargo test -p filecargo-remote-fs`
- **Files:** workspace `Cargo.toml`, `crates/filecargo-remote-fs/{Cargo.toml,src/lib.rs,src/path.rs,src/entry.rs,src/error.rs,src/fs.rs}`

### T2: Local + RootedFs + contract suite (M)
`local::read_dir/stat` (symlinks not followed), `RootedFs` (maps `RemotePath` under a root, never escapes it), `tests/contract.rs` macro covering the AC2 scenarios, instantiated for `RootedFs`.
- **Accept:** AC2 for `RootedFs`; escape attempts (`..` via symlink) refused.
- **Verify:** `cargo test -p filecargo-remote-fs --test contract`
- **Files:** `src/local.rs`, `tests/contract.rs`, `tests/support/mod.rs`

### T3: Docker test servers + CI (M)
`tests/docker/compose.yml`, `tests/docker/proftpd/{Dockerfile,explicit.conf,implicit.conf,reuse.conf}`, OpenSSH service (users, password, key, encrypted key), committed throwaway keys/certs + a `tests/docker/README.md` (how to regenerate, `orb start`), `.cargo/config.toml` alias `it`, CI job `integration` on ubuntu (compose up `--wait`, `cargo it`, compose logs on failure).
- **Accept:** all four services healthy locally and in CI; `ssh -p 2222` and `openssl s_client -starttls ftp -connect localhost:2121` work manually.
- **Verify:** `docker compose ... up -d --wait`; `actionlint`; CI run green
- **Files:** `tests/docker/**`, `.cargo/config.toml`, `.github/workflows/ci.yml`

### Checkpoint A: review trait + contract suite; containers healthy → `transfer` may start

### T4: Connect plumbing + SFTP transport + host keys (M)
`ConnectContext`, `Prompter`, `SessionTrust`, `Session`, `SessionInfo`, `ConnectError`, `connect()` dispatch; russh client with timeouts/keepalive; `check_server_key` → known_hosts (`~/.ssh` read-only path injectable for tests, then `<data>/known_hosts`) → prompt; trust always / once / reject; changed ⇒ `HostKeyChanged`. Check whether hashed `|1|` entries match (note the result). Try russh with `ring` only.
- **Accept:** AC4 (user known_hosts byte-identical; prompt counts asserted by `TestPrompter`).
- **Verify:** `cargo it -- sftp_host_keys`
- **Files:** `src/connect.rs`, `src/prompt.rs`, `src/trust.rs`, `src/sftp/{mod.rs,transport.rs,host_keys.rs}`, `tests/sftp_connect.rs`

### T5: SFTP authentication (M)
Password: `SessionTrust` → keychain (if `remember`) → prompt; remember on success; retry once with `retry: true`; keyboard-interactive answering with the same secret / prompting for multi-prompt. Key file: `~` expansion, `KeyIsEncrypted` ⇒ passphrase prompt, wrong passphrase ⇒ retry once. Agent: `connect_env` (unix), named pipe then Pageant (windows, compile-checked in CI).
- **Accept:** AC3, AC7; the agent test runs on Linux CI with a spawned `ssh-agent`.
- **Verify:** `cargo it -- sftp_auth`
- **Files:** `src/sftp/auth.rs`, `src/credentials.rs`, `tests/sftp_auth.rs`

### T6: SFTP filesystem (M)
`SftpFs: RemoteFs` (list via `read_dir`, `stat`/`lstat`, mkdir, rename, remove, `set_metadata` from `FileAttributes::empty()` for chmod/mtime, ranged download/upload with explicit `close`, capabilities). Raise window / packet sizes; log throughput of a 100 MB transfer.
- **Accept:** contract suite (AC2) green on docker `sftp`.
- **Verify:** `cargo it -- contract::sftp`
- **Files:** `src/sftp/fs.rs`, `tests/contract.rs`

### T7: Shell channel (S)
`ShellOpener::open(term, cols, rows)` → PTY + shell on the same `Handle`; pump task draining `ChannelMsg` into `ShellOutput` (Data, ExtendedData, ExitStatus, Eof/Close); `ShellInput::Resize` ⇒ `window_change`.
- **Accept:** AC8 (single TCP connection asserted via server-side `ss`/`who` or connection count).
- **Verify:** `cargo it -- shell`
- **Files:** `src/sftp/shell.rs`, `tests/shell.rs`

### Checkpoint B: SFTP complete → `terminal` may start

### T8: FTP connection, listing, metadata ops (M)
`FtpFs` over `AsyncRustlsFtpStream` (plain for now), login (anonymous / password via the same credential lookup), `TYPE I`, FEAT, `OPTS UTF8 ON`, `pwd`, own MLSD parser (unit-tested on real-world lines), LIST fallback with name-only entries, `stat` (MLST or parent listing), mkdir / rename / rm / rmdir / `SITE CHMOD` / `MFMT`, async-mutex serialization.
- **Accept:** contract scenarios except transfers green on docker `ftp` (plain); parser unit tests cover slink, fractional times, `;` in names, cdir/pdir.
- **Verify:** `cargo test -p filecargo-remote-fs ftp::`, `cargo it -- contract::ftp`
- **Files:** `src/ftp/{mod.rs,fs.rs,mlsd.rs,list.rs}`, `tests/contract.rs`

### T9: FTP transfers (M)
download (`REST` + `RETR` stream, `finish()`), upload (`STOR` / `APPE` after `SIZE == offset`), progress, cancel ⇒ `broken` ⇒ `Disconnected`, NOOP keepalive task, redacted command/reply logging on `filecargo::protocol`.
- **Accept:** full contract suite green on plain `ftp`; AC9.
- **Verify:** `cargo it -- contract::ftp ftp_cancel`
- **Files:** `src/ftp/transfer.rs`, `src/ftp/keepalive.rs`, `tests/ftp_cancel.rs`

### T10: FTPS (M)
`Pinning` verifier (platform verifier + ring provider, `x509-parser` details), two-pass prompt flow, `<data>/trusted_certs.toml` (host:port → SHA-256), explicit (`into_secure`) and implicit (`connect_secure_implicit`), one `Arc<ClientConfig>` per site, 522/534 ⇒ `TlsSessionReuseRequired`.
- **Accept:** AC5, AC6; contract suite green on `ftp` (explicit) and `ftps-implicit`.
- **Verify:** `cargo it -- ftps contract::ftps`
- **Files:** `src/tls/{mod.rs,verifier.rs,pins.rs}`, `src/ftp/mod.rs`, `tests/ftps.rs`

### T11: Active mode, redaction, final run (S)
Active mode via container IP (test `#[cfg(target_os = "linux")]` + env gate); a whole-run log-capture test asserting no secret (AC10); all contract instances green; coverage report.
- **Accept:** AC10; Checkpoint C items.
- **Verify:** `cargo it`; `cargo llvm-cov -p filecargo-remote-fs --features integration`
- **Files:** `tests/active_mode.rs`, `tests/redaction.rs`

### Checkpoint C: module complete (10 AC, CI green incl. integration job, hand-off notes, spec → done, human review)
