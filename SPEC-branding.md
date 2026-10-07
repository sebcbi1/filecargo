# Spec: branding (app icon on every platform, README logo)

> Increment id: `branding` · Status: **draft, awaiting review** · Last updated: 2026-10-07
> Builds on the v1 modules in [SPEC.md](SPEC.md). Touches the `gui` crate (startup, `build.rs`) and
> the release workflow only. Module ids don't change. The TUI is out of scope.

## Objective

The GUI currently shows a generic icon everywhere: Dock, taskbar, Alt-Tab, Explorer and app
launchers. This increment puts the new logo (`assets/logo.svg`, committed 2026-10-07) everywhere
the OS shows an app icon, and at the top of the README. It is for the developer installing a
release on their own machines. Done means a release archive, unpacked as the README says, shows
the logo on macOS, Windows and Linux (X11 and Wayland) with no extra steps beyond those the README
lists.

How each platform gets its icon (verified against `gpui-pre 0.3.8` sources, 2026-10-07):

| Platform | Where the icon comes from | gpui support |
|---|---|---|
| macOS | `.app` bundle: `Info.plist` `CFBundleIconFile` → `.icns` | none at runtime (setting it would need `unsafe` Objective-C, which `unsafe_code = forbid` rules out) |
| Windows | icon resource **id 1** embedded in `filecargo.exe` | `load_icon()` loads `MAKEINTRESOURCE(1)` for the window class: title bar, taskbar and Alt-Tab |
| Linux X11 | `_NET_WM_ICON` | `WindowOptions.icon: Option<Arc<image::RgbaImage>>`. **Only the X11 backend reads it.** |
| Linux Wayland | `.desktop` file whose name matches the `app_id` (`filecargo`) + hicolor icon theme | `app_id` already set in `main.rs` |

| Feature id | What | Touches |
|---|---|---|
| `icon-assets` | A script that renders every derived icon file from `logo.svg`. Its outputs are committed. | `assets/` |
| `win-icon` | Embed `filecargo.ico` as resource id 1 in `filecargo.exe` | `crates/filecargo-gui/{build.rs,Cargo.toml}` |
| `x11-icon` | Pass the logo as `WindowOptions.icon` on Linux | `crates/filecargo-gui/src/{main,icon}.rs` |
| `linux-desktop` | `filecargo.desktop` and hicolor PNGs in the Linux archives, with install steps in the README | `assets/linux/`, `release.yml`, README |
| `mac-app` | `filecargo.app` (Info.plist + `.icns`, ad-hoc signed) in the macOS archives | `assets/macos/`, `release.yml`, README |
| `readme-logo` | Centered logo header in the README | `README.md` |

### icon-assets

- `assets/icons.sh` regenerates everything under `assets/icons/` from `assets/logo.svg`. It is
  idempotent, runs on macOS, and needs `rsvg-convert`, `magick` (ImageMagick 7) and `iconutil`.
  It stops with a clear message naming any missing tool.
- Outputs (all committed):
  - `assets/icons/png/filecargo-{16,24,32,48,64,128,256,512}.png`: full-bleed tile (the artwork
    as drawn, edge to edge).
  - `assets/icons/filecargo.ico`: full bleed, 16/24/32/48/64/128/256 px, 32-bit RGBA.
  - `assets/icons/filecargo.icns`: **Apple grid**, the tile scaled to 824/1024 and centered with
    a transparent margin. All `iconutil` iconset sizes from 16 to 512@2x.
- The same artwork at every size. There's no simplified small-size variant (see Open Questions).
- `logo.svg` and `logo.png` are not modified.

### win-icon

- `crates/filecargo-gui/build.rs` compiles `crates/filecargo-gui/windows/filecargo.rc`
  (`1 ICON "filecargo.ico"`, with `assets/icons` as an include dir) with
  `embed_resource::compile_for(.., ["filecargo"], ..)`. It does this **only when
  `CARGO_CFG_TARGET_OS == "windows"`**, so macOS and Linux builds never invoke a resource compiler.
- A failed resource compile, or a missing resource compiler, fails the build (`manifest_required`). It is not ignored.
- Result: Explorer shows the exe icon, and gpui's window class picks it up for the title bar,
  taskbar and Alt-Tab with no runtime code.

### x11-icon

- `filecargo_gui::icon::window_icon() -> Option<Arc<image::RgbaImage>>` decodes
  `include_bytes!` of `assets/icons/png/filecargo-256.png`. `main` puts it in
  `WindowOptions.icon`. It is `cfg(target_os = "linux")`, so macOS and Windows binaries don't
  embed or decode it.
- If decoding fails, it logs `tracing::warn!` and the window opens without an icon. Startup never
  fails because of the icon.

### linux-desktop

- `assets/linux/filecargo.desktop`: `Type=Application`, `Name=filecargo`,
  `GenericName=FTP/SFTP Client`, `Exec=filecargo`, `Icon=filecargo`, `Terminal=false`,
  `Categories=Network;FileTransfer;`, `StartupWMClass=filecargo`.
- The Linux release archives gain `share/applications/filecargo.desktop` and
  `share/icons/hicolor/<n>x<n>/apps/filecargo.png` for every PNG size.
- The README's Install section says to copy `share/` into `~/.local/share/` and the binaries into a
  directory on `PATH` (e.g. `~/.local/bin`). That is what Wayland needs to show the icon.

### mac-app

- `assets/macos/Info.plist` is a template:
  - `CFBundleName` / `CFBundleDisplayName` = `filecargo`
  - `CFBundleIdentifier` = `io.github.sebcbi1.filecargo`
  - `CFBundleExecutable` = `filecargo`, `CFBundleIconFile` = `filecargo`,
    `CFBundlePackageType` = `APPL`
  - `CFBundleShortVersionString` / `CFBundleVersion` = the workspace version, substituted at
    packaging time
  - `NSHighResolutionCapable` = true
  - `LSMinimumSystemVersion` = the deployment target the binary is actually built for (read with
    `vtool -show-build` during implementation, not guessed)
- `assets/macos/make-app.sh <binary> <version> <out dir>` assembles `filecargo.app/Contents/{Info.plist,
  MacOS/filecargo, Resources/filecargo.icns}`, fills `LSMinimumSystemVersion` from the binary's
  `minos` (`vtool -show-build`), runs `plutil -lint`, ad-hoc signs the finished bundle with
  `codesign --force --sign -`, then verifies with `codesign --verify --strict`. The two macOS
  release legs call it, and it can be run locally to look at the real Dock icon.
- macOS archive contents: `filecargo.app`, `filecargo-tui`, README and licenses. There is no bare
  `filecargo` binary next to the app. Windows and Linux archives keep their current layout, plus
  the `share/` tree on Linux.
- README: unpack, move `filecargo.app` to `/Applications`, and allow it on first launch in
  *System Settings → Privacy & Security* (it is still not notarized).
- `release.yml` gains a `workflow_dispatch` trigger that builds and uploads the archives as
  workflow artifacts **without publishing**. The tag/version check and the `publish` job run on
  tag pushes only. This tests packaging before a tag.

### readme-logo

- At the top of `README.md`, above the title:
  `<p align="center"><img src="assets/logo.svg" width="128" alt="filecargo logo"></p>`.

## Tech Stack

The v1 stack in [SPEC.md](SPEC.md). New dependencies, both declared in `[workspace.dependencies]`
and **approved with this spec**:

| Crate | Version | Where | Why |
|---|---|---|---|
| `image` | 0.25 (`default-features = false`, `features = ["png"]`) | `filecargo-gui`, `[target.'cfg(target_os = "linux")'.dependencies]` | `WindowOptions.icon` takes an `image::RgbaImage`, and gpui-kit doesn't re-export `image`. Already compiled through `gpui-pre` (0.25.10, with `png`), so this adds no new code to the build. |
| `embed-resource` | 3.0.11 | `filecargo-gui` `[build-dependencies]` | Compiles the `.rc` with the MSVC `rc.exe` / `llvm-rc`. Already in the lockfile (3.0.11) through `gpui-pre`, so the version matches it. |

Tools (not in `mise.toml`; only `assets/icons.sh` needs them): `rsvg-convert` ≥ 2.5x,
ImageMagick 7, `iconutil` (macOS). CI only copies committed files.

## Commands

```
Icons:         assets/icons.sh                       # regenerate assets/icons/* (macOS)
Format:        cargo fmt --all --check
Lint (GUI):    cargo clippy -p filecargo-gui --all-targets -- -D warnings
Test (GUI):    cargo test -p filecargo-gui
Run GUI:       cargo run -p filecargo-gui
               (on NixOS, wrap the GUI commands in nix-shell --run "…")
Packaging:     gh workflow run release.yml --ref <branch>   # builds archives, publishes nothing
Inspect:       gh run download <run-id> -D /tmp/fc-release
```

## Project Structure

```
assets/
├── logo.svg, logo.png            # source artwork (unchanged)
├── icons.sh                      # icon-assets generator
├── icons/
│   ├── filecargo.ico             # win-icon
│   ├── filecargo.icns            # mac-app (Apple grid)
│   └── png/filecargo-<n>.png     # x11-icon (256), linux-desktop (all)
├── linux/filecargo.desktop       # linux-desktop
└── macos/{Info.plist,make-app.sh} # mac-app template and bundle assembly
crates/filecargo-gui/
├── build.rs                      # win-icon (Windows targets only)
├── windows/filecargo.rc          # win-icon
└── src/icon.rs                   # x11-icon (cfg linux)
.github/workflows/release.yml     # mac-app, linux-desktop, workflow_dispatch
tasks/branding/{plan,todo}.md     # plan and task list for this increment
```

## Code Style

Same as v1: edition 2024, `unsafe_code = forbid`, clippy `-D warnings`, comments only for the
non-obvious. Platform code is gated with `cfg` at the item, not with runtime checks:

```rust
/// The window icon for X11 (`_NET_WM_ICON`); the other platforms take it from the bundle,
/// the exe resources or the desktop file.
#[cfg(target_os = "linux")]
pub fn window_icon() -> Option<Arc<image::RgbaImage>> {
    const PNG: &[u8] = include_bytes!("../../../assets/icons/png/filecargo-256.png");
    match image::load_from_memory_with_format(PNG, image::ImageFormat::Png) {
        Ok(decoded) => Some(Arc::new(decoded.into_rgba8())),
        Err(error) => {
            tracing::warn!(%error, "cannot decode the window icon");
            None
        }
    }
}
```

Shell (`icons.sh`, workflow steps): `set -euo pipefail`, one tool invocation per output, no
silent fallbacks.

## Testing Strategy

Most of this increment is assets and packaging, so automated checks guard the parts that can
silently break. The rest is a manual smoke list.

- **Unit (`filecargo-gui`, `cfg(target_os = "linux")`, runs in the `gui (ubuntu-latest)` CI
  job):** `window_icon()` returns `Some` 256×256 image. The `include_bytes!` path is checked at
  compile time.
- **Build:** the `gui (windows-latest)` CI job builds `filecargo.exe` and so compiles the
  resource. A missing `.ico` or a broken `.rc` fails it.
- **Release workflow checks** (in the packaging steps, failing the job):
  - macOS: `plutil -lint`, `codesign --verify --strict`, and `test -f` on the icns and executable
    inside the bundle.
  - Linux: `desktop-file-validate assets/linux/filecargo.desktop`. This adds `desktop-file-utils`
    to the existing `apt-get install` line.
- **Assets:** `icons.sh` ends by checking with `magick identify` / `iconutil` that every output
  exists at its expected sizes. It exits non-zero otherwise.
- **Manual smoke:** a new *Branding* section in `crates/filecargo-gui/SMOKE.md`, run on archives
  from a `workflow_dispatch` run:
  - macOS: Dock, ⌘-Tab and Finder icons on the `.app`, same size as neighbouring Dock icons.
  - Windows: Explorer exe icon, title bar, taskbar and Alt-Tab.
  - Linux X11: window and taskbar icon with no desktop file installed.
  - Linux Wayland: icon in the dock or launcher after the README install steps.
  - README: renders on GitHub in light and dark themes.

## Boundaries

- **Always:**
  - Regenerate derived icons with `assets/icons.sh`. Never hand-edit them.
  - Commit the script outputs together with any `logo.svg` change.
  - Run fmt, clippy and the GUI tests before each commit.
  - Make one jj commit per task, with no AI attribution lines.
  - Keep platform code behind `cfg`, so non-Linux builds don't link `image` for the icon.
- **Ask first:**
  - Changing the artwork itself (`logo.svg` / `logo.png`).
  - Adding any dependency beyond `image` and `embed-resource`.
  - Changing the bundle identifier or `app_id`.
  - Changing CI beyond what this spec lists (`workflow_dispatch`, packaging steps,
    `desktop-file-utils`).
  - Signing or notarization with a real certificate.
  - Installers (`.dmg`, MSI, `.deb`, AppImage).
- **Never:**
  - `unsafe` (including Objective-C calls for the Dock icon).
  - Fail GUI startup because of an icon.
  - Publish a release or push a tag without being asked.
  - Ship the bare `filecargo` binary next to `filecargo.app` in the macOS archive.

## Success Criteria

1. `assets/icons.sh` on a clean checkout reproduces every file under `assets/icons/`, and the
   size check passes.
2. `filecargo.exe` (built in CI) shows the logo in Explorer, and the running window shows it in
   the title bar, taskbar and Alt-Tab.
3. On Linux X11 the window shows the logo with nothing installed. On Wayland it shows after the
   README's install steps. `desktop-file-validate` passes.
4. The macOS archive contains a `filecargo.app` that passes `plutil -lint` and
   `codesign --verify`. Once allowed in Privacy & Security, it launches the GUI, and the Dock
   shows the logo at the same visual size as other apps.
5. A `workflow_dispatch` run of `release.yml` produces all six archives and publishes nothing. A
   tag push still publishes as before.
6. The README shows the centered logo above the title on GitHub, in light and dark themes.
7. `cargo build` / `cargo test` (default members) are unaffected and still don't compile gpui.
   CI is green on all jobs.

## Open Questions

Decided with the user on 2026-10-07:
- macOS ships a `.app` inside the existing tar.gz (no `.dmg`).
- The `.icns` uses the Apple grid; Windows and Linux use the full-bleed tile.
- Linux gets the desktop file and icons in the archive plus the X11 runtime icon (no install
  script).
- Derived icons are generated by a script and committed.

Still open (none blocking):
1. **macOS 26+ icon shape.** Since macOS 26, the Dock draws legacy icons whose shape doesn't match
   Apple's squircle inside a grey container. Our tile's corners are close to Apple's mask but not
   identical. If the smoke test shows the grey container, `icons.sh` clips the macOS render to
   Apple's mask. Adopting Icon Composer's `.icon` format is out of scope.
2. **Small sizes.** At 16 and 24 px the drop shadow and the document fold may turn to mush. Is the
   same artwork acceptable for v1, or do you want a simplified variant later?
3. **Windows manifest.** gpui-pre's own manifest (Per-Monitor-V2 DPI awareness, Common Controls
   v6) is linked with `rustc-link-arg-bins`, so it never reaches downstream binaries. Today
   `filecargo.exe` probably has no manifest. Should the same `.rc` also embed one? That would be
   a separate change, but the file would already be there.

Assumptions to confirm at review:
- Bundle identifier `io.github.sebcbi1.filecargo`; the `app_id` stays `filecargo`.
- The macOS bundle is ad-hoc signed only, so the README keeps the "allow in Privacy & Security"
  note.
- The README header uses the SVG (8.6 KB) rather than the 463 KB PNG.
