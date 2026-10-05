# Spec: remote-fs

> Module id: `remote-fs` · Crate: `crates/filecargo-remote-fs` · Depends on: `config` · Status: **draft, awaiting review**
> Project-wide rules: [SPEC.md](SPEC.md). Plan: [tasks/remote-fs/plan.md](tasks/remote-fs/plan.md).

## Objective

One async filesystem interface over **SFTP**, **FTP**, **explicit FTPS** and **implicit FTPS**,
plus a **local** backend. The module also owns connecting: authentication, SSH host-key checks,
TLS certificate checks, and the SSH shell channel the `terminal` module renders. Everything
above this crate (`transfer`, `terminal`, `app-core`) talks to servers only through this API.

The module never touches the UI. Decisions that need a human (passwords, unknown host keys,
untrusted certificates) go through a `Prompter` trait that `app-core` implements.

## Public API (contract)

### Paths and entries

```rust
/// Absolute, `/`-separated, UTF-8, normalized: no `.`/`..`, no duplicate or trailing `/` (except root).
pub struct RemotePath(String);
impl RemotePath {
    pub fn root() -> Self;
    pub fn parse(s: &str) -> Result<Self, PathError>;   // relative input is rejected
    pub fn join(&self, name: &str) -> Result<Self, PathError>; // name must not contain '/', be '.', '..' or empty
    pub fn parent(&self) -> Option<Self>;
    pub fn file_name(&self) -> Option<&str>;
    pub fn as_str(&self) -> &str;
}

pub enum EntryKind { File, Dir, Symlink { target: Option<String> }, Other }

/// One directory entry. Same type for local and remote listings (no path; callers join).
pub struct Entry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,                      // 0 for dirs
    pub modified: Option<SystemTime>,
    pub permissions: Option<u32>,       // unix mode bits (0o7777 mask) when known
    pub owner: Option<String>,          // usually None: SFTPv3 and most FTP listings carry no names
    pub group: Option<String>,
}
```

### The filesystem trait

Object-safe (`async-trait`), so a backend is chosen at runtime and tests can provide fakes.

```rust
#[async_trait]
pub trait RemoteFs: Send + Sync {
    fn protocol(&self) -> Protocol;
    fn capabilities(&self) -> Capabilities;            // chmod, set_mtime, resume_upload, resume_download
    /// Directory the server starts in (SFTP canonicalize("."), FTP PWD).
    async fn home(&self) -> Result<RemotePath, FsError>;
    /// Entries of `dir`, excluding "." and "..".
    async fn list(&self, dir: &RemotePath) -> Result<Vec<Entry>, FsError>;
    /// `Ok(None)` when nothing exists at `path`.
    async fn stat(&self, path: &RemotePath) -> Result<Option<Entry>, FsError>;
    async fn mkdir(&self, path: &RemotePath) -> Result<(), FsError>;
    async fn rename(&self, from: &RemotePath, to: &RemotePath) -> Result<(), FsError>;
    async fn remove_file(&self, path: &RemotePath) -> Result<(), FsError>;
    async fn remove_dir(&self, path: &RemotePath) -> Result<(), FsError>;      // must be empty
    async fn chmod(&self, path: &RemotePath, mode: u32) -> Result<(), FsError>;
    async fn set_modified(&self, path: &RemotePath, time: SystemTime) -> Result<(), FsError>;

    /// Streams `path` from byte `offset` into `sink`; returns bytes written.
    async fn download(&self, path: &RemotePath, offset: u64,
                      sink: &mut (dyn AsyncWrite + Send + Unpin), progress: &dyn Progress)
                      -> Result<u64, FsError>;
    /// Writes `source` to `path`. `offset == 0` truncates/creates; `offset > 0` resumes
    /// (the remote file must currently be exactly `offset` bytes long). Returns bytes read.
    async fn upload(&self, path: &RemotePath, offset: u64,
                    source: &mut (dyn AsyncRead + Send + Unpin), progress: &dyn Progress)
                    -> Result<u64, FsError>;

    /// Recursive delete built on list/remove (provided method).
    async fn remove_all(&self, path: &RemotePath) -> Result<(), FsError> { /* default impl */ }
    /// Best-effort clean shutdown (FTP QUIT, SSH disconnect).
    async fn close(&self);
}

pub trait Progress: Send + Sync { fn advance(&self, total_bytes_so_far: u64); }
```

Rules every backend must follow, checked by the shared contract suite:
- Cancellation is **dropping the future**. A backend whose connection can't be trusted after a
  cancelled transfer (FTP mid-`RETR`) marks itself broken: later calls return `FsError::Disconnected`.
- Calls on one `RemoteFs` may come from several tasks. Backends serialize internally when the
  protocol requires it (FTP: one command or transfer at a time per control connection).
- `list` returns entries in server order. Sorting and hiding dotfiles is the caller's job.

### Errors

```rust
pub enum FsError {
    NotFound(String), PermissionDenied(String), AlreadyExists(String),
    NotADirectory(String), DirectoryNotEmpty(String),
    Unsupported(&'static str),                  // e.g. chmod on a server without SITE CHMOD
    /// vsftpd `require_ssl_reuse` / FileZilla Server: the data connection's TLS session must
    /// resume the control session, which the FTP library cannot do yet (suppaftp #93).
    TlsSessionReuseRequired,
    Disconnected(String), Timeout,
    Protocol { code: Option<u16>, message: String },   // unexpected server reply
    LocalIo(String),                                   // the caller's sink/source failed
}
impl FsError { pub fn is_retryable(&self) -> bool; }   // Disconnected | Timeout
```
`TlsSessionReuseRequired` displays as: *"This server requires TLS session reuse on data
connections, which filecargo does not support yet. Use SFTP, or ask the admin to set
`require_ssl_reuse=NO`."*

### Connecting

```rust
pub async fn connect(site: &Site, ctx: &ConnectContext) -> Result<Session, ConnectError>;

pub struct ConnectContext {
    pub paths: Paths,                          // known_hosts, trusted_certs
    pub secrets: Arc<dyn SecretStore>,
    pub prompter: Arc<dyn Prompter>,
    pub trust: Arc<SessionTrust>,              // in-memory, shared by every connection of the app
    pub timeouts: ConnectionSettings,          // connect timeout, keepalive
}

pub struct Session {
    pub fs: Arc<dyn RemoteFs>,
    pub info: SessionInfo,                     // protocol, server banner, TLS summary, home dir
    pub shell: Option<ShellOpener>,            // Some for SFTP only
}

#[async_trait]
pub trait Prompter: Send + Sync {
    /// `None` = user cancelled. `request.remember_allowed` tells the UI whether to show "remember".
    async fn credential(&self, request: CredentialPrompt) -> Option<CredentialAnswer>;
    async fn host_key(&self, request: HostKeyPrompt) -> TrustDecision;
    async fn certificate(&self, request: CertificatePrompt) -> TrustDecision;
}
pub enum CredentialPrompt { Password { site: String, user: String, retry: bool },
                            Passphrase { site: String, key_path: PathBuf, retry: bool },
                            KeyboardInteractive { site: String, name: String, instructions: String,
                                                  prompts: Vec<(String, bool /*echo*/)> } }
pub struct CredentialAnswer { pub values: Vec<SecretString>, pub remember: bool }
pub enum TrustDecision { TrustOnce, TrustAlways, Reject }

pub enum ConnectError {
    Resolve(String), Refused(String), Timeout, Network(String),
    HostKeyRejected, HostKeyChanged { host: String, fingerprint: String, known_in: PathBuf },
    CertificateRejected, Tls(String),
    AuthFailed { methods_tried: Vec<String> }, KeyFile(String),
    Cancelled, Fs(FsError),
}
```

`SessionTrust` (memory only, lost on exit) holds credentials typed during this run and host
keys/certs accepted with *Trust once*. Transfer workers open their own connections, and this
shared cache keeps them from prompting the user a second time.

### SSH shell channel (contract consumed by `terminal`)

```rust
pub struct ShellOpener { /* holds the SSH connection handle */ }
impl ShellOpener {
    /// Opens a PTY + shell on the **same** SSH connection as the SFTP session.
    pub async fn open(&self, term: &str, cols: u16, rows: u16) -> Result<ShellChannel, FsError>;
}
pub struct ShellChannel {
    pub input: mpsc::UnboundedSender<ShellInput>,      // Data(Vec<u8>) | Resize { cols, rows } | Close
    pub output: mpsc::UnboundedReceiver<ShellOutput>,  // Data(Vec<u8>) | Exit(Option<u32>) | Closed
}
```

### Local filesystem

```rust
pub mod local {
    /// Listing for the local pane (`Entry` without path; symlinks reported, not followed).
    pub async fn read_dir(dir: &Path) -> Result<Vec<Entry>, FsError>;
    pub async fn stat(path: &Path) -> Result<Option<Entry>, FsError>;
    /// A `RemoteFs` whose root `/` is `root` on disk. Used by contract tests, transfer tests
    /// and app-core tests in place of a server. Never used for real sessions.
    pub struct RootedFs { /* root: PathBuf */ }
}
```

## Behavior

### Authentication (both protocols)
1. `Auth::Anonymous` (FTP): user `anonymous`, password `anonymous@`.
2. `Auth::Password { remember }`: look up the secret in this order: `SessionTrust`, then the
   keychain (if `remember`), then the prompter. On success, a prompted value goes into
   `SessionTrust`, and into the keychain if `remember` and the user ticked remember. A stored
   password that fails is re-prompted **once** (`retry: true`), then the connection fails with
   `AuthFailed`. SSH tries `password`, then `keyboard-interactive`, answering one-prompt
   interactive challenges with the same secret.
3. `Auth::KeyFile { path, remember_passphrase }` (SFTP): expand `~`. Load without a passphrase
   first; if encrypted, get the passphrase with the same lookup order (`Passphrase` prompt). A
   wrong passphrase re-prompts once.
4. `Auth::Agent` (SFTP): unix `SSH_AUTH_SOCK`; Windows OpenSSH agent pipe, then Pageant. Try
   every agent identity. No agent → `AuthFailed { methods_tried: ["agent (not running)"] }`.

### SSH host keys
- Look up `host:port` in `~/.ssh/known_hosts` (read-only, never written) and then in
  `<data>/known_hosts` (filecargo's own file).
- **Match** → continue. **Unknown** → `Prompter::host_key` with algorithm and SHA-256
  fingerprint (`SHA256:base64`, as OpenSSH prints it): *Trust always* appends to filecargo's file;
  *Trust once* records it in `SessionTrust`; *Reject* → `HostKeyRejected`.
- **Changed** (a different key is on file) → no prompt. `HostKeyChanged` with the new
  fingerprint and the file that holds the old key. The user must edit that file themselves.

### TLS certificates (FTPS)
- Verified against the OS trust store, plus the hostname.
- A failure is recorded (reason, subject, issuer, expiry, SHA-256) and the handshake aborts.
  `connect` then calls `Prompter::certificate`. *Trust once/always* reconnects, accepting exactly
  that leaf fingerprint for that `host:port`. *Always* writes it to `<data>/trusted_certs.toml`.
  A pinned cert that later changes prompts again; it is never silently accepted.
- Data connections use the same verifier.

### FTP / FTPS specifics
- Explicit FTPS: `AUTH TLS`, then `PBSZ 0` + `PROT P`. Plain FTP never sends credentials over TLS
  that it didn't negotiate. A **plain** FTP site gets a one-time log warning that credentials
  travel in clear text.
- Passive by default (EPSV when offered, else PASV with the NAT workaround of reusing the
  control IP). `FtpMode::Active` uses EPRT/PORT.
- Listing: `MLSD` when FEAT advertises it, else `LIST -a` parsed as Unix or DOS format.
  filecargo has **its own MLSD parser**: the library's rejects `OS.unix=slink`, fractional
  `modify` times and names containing `;`. LIST lines that neither the Unix nor the DOS
  parser understands still appear as entries (name only, `EntryKind::Other`). Nothing is
  silently dropped. `cdir` / `pdir` entries are filtered out.
- `OPTS UTF8 ON` when advertised. Paths are UTF-8.
- chmod = `SITE CHMOD`; unsupported → `Unsupported("chmod")`. `set_modified` = `MFMT` when
  advertised, else `Unsupported`.
- Upload resume = `APPE` after checking `SIZE == offset`. Download resume = `REST offset` + `RETR`.
- Idle keepalive: `NOOP` every `keepalive_secs`.
- One control connection per `RemoteFs` instance; calls are serialized behind an async mutex.

### SFTP specifics
- One SSH connection carries the SFTP subsystem and, on demand, PTY shells.
- `chmod` / `set_modified` via SFTP setstat; upload resume = open for write at `offset` with
  no truncation.
- Keepalive via SSH keepalive messages every `keepalive_secs`.

### Logging
- `tracing` target `filecargo::protocol`, field `site`. Commands and replies at `debug`, with
  `PASS` arguments and any secret replaced by `***`. Connect, auth method, host-key and TLS
  decisions at `info`. Never log a secret, at any level.
- The FTP library logs raw control traffic, **including `PASS <password>`**, through the `log`
  crate at `trace`. filecargo never bridges `log` records below `debug` for the `suppaftp`
  target. The FTP backend logs commands and replies itself, redacted.

## Test servers (`tests/docker/compose.yml`)

| Service | Image | Purpose | Host ports |
|---|---|---|---|
| `sftp` | `lscr.io/linuxserver/openssh-server` (pinned digest) | password user + key user (plain and encrypted key), bash shell | 2222 |
| `ftp` | `tests/docker/proftpd/Dockerfile` (Alpine ProFTPD 1.3.8 + mod_tls), `explicit.conf` | plain + explicit TLS, `NoSessionReuseRequired` |
| `ftps-implicit` | same image, `implicit.conf` | implicit TLS | 9990, passive 30010–30019 |
| `ftps-reuse` | same image, `reuse.conf` | explicit TLS, session reuse required (ProFTPD default) | 2122, passive 30020–30029 |

- Passive ranges are published 1:1, with `MasqueradeAddress 127.0.0.1`. vsftpd (the first choice) was dropped: its listener process segfaults after the first completed TLS session on the dev host, on both Alpine 3.21 and Debian bookworm builds.
- **Active mode** can't work through published localhost ports (servers refuse a `PORT` from
  behind NAT). Active-mode tests connect to the container IP and run on **Linux CI only**.
- Throwaway keys and self-signed certs are generated into `tests/docker/` once, committed, and
  used nowhere else.

## Implementation notes (verified against crate sources, 2026-10-05)
- **Crates:** `russh =0.64.1` + `russh-sftp 3.0.1`, and `suppaftp 12.1.1` with features
  `tokio-rustls-ring` + `deprecated` (implicit TLS). TLS uses `rustls 0.23` +
  `rustls-platform-verifier 0.7.1` with the **ring** provider passed explicitly, plus
  `x509-parser 0.18` for prompt details. Try russh without default features (`ring`) to
  avoid the aws-lc-rs C build on Windows.
- **SSH host keys:** `Handler::check_server_key` is async, so it awaits the prompt.
  `check_known_hosts_path` returns `Ok(true)` (match), `Ok(false)` (unknown, or a different
  algorithm) or `Err(KeyChanged)`. Its parser doesn't handle tabs, wildcards or
  `@cert-authority`, and hashed `|1|` entries are unverified. A known host it can't match
  therefore shows the unknown-key prompt, which is safe and only costs a click. Hashed-entry
  support is checked in the host-key task.
- **Drain every channel.** russh's session loop blocks on a full channel buffer, so an
  unread PTY channel would stall SFTP too. The shell pump task always drains into the
  unbounded `ShellOutput` sender.
- **SFTP:**
  - `set_metadata` sends every `Some` field, so always start from
    `FileAttributes::empty()`. Re-sending fetched metadata would truncate via `size`.
  - Always `close().await` written files; a dropped file loses its errors.
  - Don't `flush` per chunk: it triggers `fsync@openssh.com`.
  - Throughput knobs: russh `window_size`, russh-sftp `max_write_packet_len`.
- **FTP:**
  - Send `TYPE I` after login.
  - Every transfer stream ends with `TransferStream::finish()`.
  - `MFMT` goes through `custom_command`.
  - Reply codes the library doesn't know (522, 534, …) come back as `Status::Unknown`. Read
    the real code from the first 3 bytes of the body. **522/534 on a data connection →
    `TlsSessionReuseRequired`.**
  - Keep one `Arc<ClientConfig>` per site so control and data connections share the TLS
    session cache, which gives resumption a chance.
- **TLS prompt reasons:** macOS reports most failures as `CertificateError::Other(..)`.
  Derive the reason shown in the prompt from the certificate itself: expired if `not_after`
  is in the past, self-signed if issuer == subject, otherwise untrusted.

## Acceptance criteria

1. `RemotePath` rejects relative paths, `..` escapes and names containing `/`; property test: `parse(p.as_str()) == p`.
2. The **contract suite** (`tests/contract.rs`) runs the same scenarios against every backend: list, stat (missing → `None`), mkdir, rename, remove_file, remove_dir (non-empty fails), remove_all, chmod (or `Unsupported`), set_modified, download/upload full, download/upload resumed at an offset (SHA-256 equal), unicode and space-containing names, 10,000-entry directory listing. `RootedFs` always runs; FTP, explicit FTPS, implicit FTPS and SFTP run with `--features integration`.
3. SFTP auth works with a password, a plain key, an encrypted key (passphrase prompted, then taken from `SessionTrust` on the 2nd connect without a prompt), and ssh-agent (Linux CI).
4. Host keys: an unknown key prompts. *Trust always* writes only filecargo's `known_hosts`, and the next connect doesn't prompt. *Reject* fails. A changed key fails with `HostKeyChanged` without prompting. `~/.ssh/known_hosts` is byte-identical after every test (tests point it at a temp file).
5. FTPS: a self-signed cert prompts. *Trust always* pins it and the next connect doesn't prompt. A different cert at the same `host:port` prompts again.
6. Against the `require_ssl_reuse=YES` server, a data transfer either succeeds (session resumption happened to work) or fails with `FsError::TlsSessionReuseRequired` and the message above. It never fails with a raw TLS or I/O error. The test records which case occurred.
7. A stored password that the server rejects prompts once with `retry: true`; a second failure gives `AuthFailed`.
8. `ShellOpener::open` on an SFTP session runs `echo hello` and receives `hello` on the same SSH connection (the server sees one TCP connection); `Resize` changes `stty size` output.
9. Dropping a `download` future mid-transfer on FTP leaves that `RemoteFs` returning `Disconnected`, and a fresh `connect` works.
10. Captured logs of a full test run contain no password, passphrase or `PASS <secret>`.

## Out of scope (v1)
`~/.ssh/config` host aliases, proxies and jump hosts, SSH certificates, FTP over HTTP proxy,
charsets other than UTF-8, server-to-server (FXP) transfers, keyboard-interactive with several
rounds of different prompts beyond what `Prompter::credential` can show, connection pooling
inside this crate (callers open one `Session` per worker).
