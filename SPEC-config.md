# Spec: config

> Module id: `config` · Crate: `crates/filecargo-config` · Depends on: — · Status: **approved** (2026-10-05)
> Project-wide sections (stack, commands, style, testing, boundaries) live in [SPEC.md](SPEC.md).

## Objective

Single source of truth for everything the user curates, shared by both front-ends:
the **server tree** (folders + sites), **app settings**, **secrets** (OS keychain), and
**FileZilla import**. Pure library: no network, no UI, no async.

Both `filecargo` (GUI) and `filecargo-tui` may run at the same time against the same
files, so a write from one must never silently discard a write from the other.

## Data model

```rust
pub struct SiteId(Uuid);      // stable across renames/moves; keys secrets in the keychain
pub struct FolderId(Uuid);

pub enum Protocol { Sftp, Ftp, FtpsExplicit, FtpsImplicit }   // default ports 22 / 21 / 21 / 990

pub enum Auth {
    Anonymous,                                        // FTP/FTPS only
    Password { remember: bool },                      // also used for SSH keyboard-interactive
    KeyFile { path: PathBuf, remember_passphrase: bool }, // SFTP only
    Agent,                                            // SFTP only
}

pub enum FtpMode { Passive, Active }                  // FTP/FTPS only, default Passive

pub struct Site {
    pub id: SiteId,
    pub name: String,
    pub folder: Option<FolderId>,                     // None = root
    pub protocol: Protocol,
    pub host: String,
    pub port: Option<u16>,                            // None = protocol default
    pub user: String,
    pub auth: Auth,
    pub ftp_mode: FtpMode,
    pub remote_dir: Option<String>,                   // initial remote dir
    pub local_dir: Option<PathBuf>,                   // initial local dir
    pub notes: String,
}

pub struct Folder { pub id: FolderId, pub name: String, pub parent: Option<FolderId> }
```

Mutations go through one enum so they can be re-applied after a reload:

```rust
pub enum TreeOp {
    AddFolder { name: String, parent: Option<FolderId> },
    AddSite(Site),
    UpdateSite(Site),
    Rename { node: NodeId, name: String },
    Move { node: NodeId, parent: Option<FolderId> },
    Duplicate { site: SiteId },                       // new id, name "<name> (copy)", secrets copied
    Delete { node: NodeId },                          // folder delete is recursive
}
```

### Validation rules (enforced on load and on every op)
- Names non-empty after trim; unique among siblings (case-insensitive).
- `host` non-empty; `port` in `1..=65535` when set.
- `Auth`/`Protocol` compatibility: `KeyFile`/`Agent` require `Sftp`; `Anonymous` requires FTP/FTPS.
- A folder cannot be moved into itself or a descendant; no dangling `parent`/`folder` ids.
- Deleting a site (or a folder containing sites) also deletes those sites' keychain entries.
  So does an `UpdateSite` that turns `remember` / `remember_passphrase` off. Secrets are cleaned
  up **after** the file write succeeds; a failed keychain delete is logged, not fatal.
- Display order is alphabetical (case-insensitive) with folders before sites, so the file stores
  no manual ordering.

## On-disk format

Located via `Paths::resolve()` → `etcetera::choose_base_strategy()` + `filecargo/`:
XDG on macOS and Linux (`~/.config/filecargo`, `~/.local/share/filecargo`, same convention
FileZilla uses), Known Folders on Windows (`%APPDATA%\filecargo`). Overridable with
`FILECARGO_CONFIG_DIR` (config + data under one dir; used by tests and portable installs).

| File | Owner | Contents |
|---|---|---|
| `<config>/servers.toml` | config | server tree |
| `<config>/settings.toml` | config | settings |
| `<data>/known_hosts` | remote-fs (path from `Paths`) | SSH host keys trusted via filecargo |
| `<data>/trusted_certs.toml` | remote-fs (path from `Paths`) | FTPS certs the user accepted |
| `<data>/queue.json` | transfer (path from `Paths`) | persisted transfer queue |

`servers.toml` is a **flat** list (hand-editable, diff-friendly):

```toml
version = 1

[[folder]]
id = "0b6f…"
name = "Work"

[[site]]
id = "7c1e…"
name = "prod-web"
folder = "0b6f…"
protocol = "sftp"
host = "web1.example.com"
user = "deploy"
auth = { method = "key_file", path = "~/.ssh/id_ed25519", remember_passphrase = true }
remote_dir = "/var/www"
```

`settings.toml` (all keys optional; missing file = defaults):

```toml
version = 1
[transfers]
max_concurrent = 2            # 1..=10
default_conflict = "ask"      # ask | overwrite | overwrite_if_newer | resume | skip | rename
[connection]
timeout_secs = 20
keepalive_secs = 30
[ui]
show_hidden = false
confirm_delete = true
local_start_dir = "~"
[log]
level = "info"                # error | warn | info | debug | trace
```

### Persistence rules
- **Atomic writes**: temp file in the same dir → fsync → rename. Never a partial file.
- **Reload-before-write**: `ConfigStore` remembers the bytes it loaded; before each
  write, if the file changed on disk it reloads, re-applies the pending `TreeOp`, re-validates,
  then writes. Two instances therefore merge at operation granularity.
- **Cross-process lock**: reload + write run under an exclusive `std::fs::File::lock` on
  `<config>/.lock`, so two instances can't interleave between the check and the rename.
- **Corrupt or invalid file**: `load()` returns `ConfigError::Parse { path, line, msg }` (or
  `Invalid { … }`); the file is **never** overwritten automatically. Front-ends show the error;
  an explicit `reset()` moves it to `servers.toml.bak-<timestamp>` first.
- `version` > supported → error, no write (protects files from a newer filecargo).

## Secrets

```rust
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &SecretKey) -> Result<Option<SecretString>, SecretError>;
    fn set(&self, key: &SecretKey, value: &SecretString) -> Result<(), SecretError>;
    fn delete(&self, key: &SecretKey) -> Result<(), SecretError>;
}
pub enum SecretKey { Password(SiteId), Passphrase(SiteId) }  // account = "site:<uuid>:password"
```
- `KeyringStore` (service `filecargo`) on `keyring-core` + per-OS native stores (macOS Keychain,
  Windows Credential Manager, Linux Secret Service) for real use; `MemoryStore` for tests.
- Secrets are **never** written to any file, log line, or `Debug` output (`secrecy::SecretString`).
- Keychain unavailable (e.g. headless Linux without Secret Service): `remember` degrades to
  "prompt every time" with a one-time warning. No plaintext fallback.

## FileZilla import

`import_filezilla(path, ImportOptions { import_passwords, folder_name }) -> Result<ImportReport, ImportError>`
reads FileZilla 3 `sitemanager.xml`.
- Default lookup: `~/.config/filezilla/sitemanager.xml` (macOS/Linux), `%APPDATA%\FileZilla\sitemanager.xml` (Windows).
- Imported under a new root folder named by the caller (app-core passes
  `Imported from FileZilla (YYYY-MM-DD)`, which keeps config free of a date crate), preserving
  nesting. If that name already exists, ` (2)`, ` (3)`, … is appended.
- Mapping: protocol `0`/`6`→`Ftp`, `1`→`Sftp`, `3`→`FtpsImplicit`, `4`→`FtpsExplicit`;
  logon type anonymous→`Anonymous`, normal→`Password{remember:true}`, ask/interactive→
  `Password{remember:false}`, key file→`KeyFile`; `PasvMode` `MODE_ACTIVE`→`Active` else `Passive`.
  Decode FileZilla's segmented `RemoteDir` encoding to a plain path.
  *(Codes are verified against FileZilla 3.x source (`ServerProtocol`, `LogonType` enums, site
  manager XML writer) before implementation. Fixtures are hand-written from that source with fake
  credentials. The user's real `sitemanager.xml` is never read by tests or tooling.)*
- `<Pass encoding="base64">` → stored in keychain only if the caller passes `import_passwords: true`.
  `encoding="crypt"` (FileZilla master password) → site imported, password skipped, noted in report.
- Unsupported protocols (S3, WebDAV, Storj, …) → skipped, listed in `ImportReport.skipped` with reason.
- Import is one atomic write; a failure imports nothing.

## Public API (sketch)

```rust
pub struct ConfigStore { /* paths, tree, settings, loaded hashes */ }
impl ConfigStore {
    pub fn open(paths: Paths, secrets: Arc<dyn SecretStore>) -> Result<Self, ConfigError>;
    pub fn tree(&self) -> &ServerTree;              // read-only view: children(parent), site(id), path_of(node)
    pub fn apply(&mut self, op: TreeOp) -> Result<NodeId, ConfigError>;
    pub fn settings(&self) -> &Settings;
    pub fn update_settings(&mut self, f: impl FnOnce(&mut Settings)) -> Result<(), ConfigError>;
    pub fn reload(&mut self) -> Result<(), ConfigError>;
    pub fn secrets(&self) -> &dyn SecretStore;
    pub fn import_filezilla(&mut self, path: &Path, opts: ImportOptions) -> Result<ImportReport, ImportError>;
}
```

## Acceptance criteria

1. Fresh config dir → `open()` succeeds with an empty tree and default settings; no files written until the first mutation.
2. Every `TreeOp` round-trips: apply → reopen store from disk → identical tree (property test over random op sequences).
3. Each validation rule above has a test that the op is rejected and **nothing** is written.
4. Two `ConfigStore`s on the same dir: A adds site X, B (stale) adds site Y → file contains X and Y.
5. Corrupt `servers.toml` → `ConfigError::Parse` with line number; file bytes unchanged afterwards.
6. Deleting a folder with 3 sites removes 3 sites and their secrets from the `SecretStore`.
7. No secret value appears in `servers.toml`, `settings.toml`, `Debug` output, or `tracing` output (test with a sentinel password).
8. FileZilla fixture (nested folders, SFTP+key, FTPS explicit, anonymous FTP, base64 password, crypt password, S3 site) imports with the exact expected tree and report.
9. `FILECARGO_CONFIG_DIR` override is honored; all tests run against temp dirs, never the real config dir or real keychain.

## Out of scope (v1)
Live file watching (other instance's edits appear on next write or explicit `reload()`),
per-site charset, proxy settings, export to FileZilla format, settings UI layout (front-end modules).
