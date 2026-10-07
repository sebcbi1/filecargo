# Tasks: branding

> Plan: [plan.md](plan.md) · Spec: [SPEC-branding.md](../../SPEC-branding.md)

**Every task:**
- Write the failing test or check first, where the task has code.
- Run `cargo fmt --all --check`, `cargo clippy --workspace --exclude filecargo-gui --all-targets -- -D warnings` and `cargo test`.
- For tasks that touch `filecargo-gui`, also run
  `cargo clippy -p filecargo-gui --all-targets -- -D warnings && cargo test -p filecargo-gui`
  (wrapped in `nix-shell --run "…"` on NixOS).
- Make one `jj commit` per task, with no attribution lines.
- Never push or tag without being asked.

---

## Phase 1: Assets and the macOS check (local)

### T1: `icons.sh` and the committed icon set
**Description:** Add `assets/icons.sh`, which renders every derived icon from `assets/logo.svg`:
- full-bleed PNGs at 16/24/32/48/64/128/256/512
- a full-bleed `.ico` (16–256)
- an Apple-grid `.icns` (tile at 824/1024, centered, all iconset sizes)

It checks its tools first and ends with a size check. Run it and commit its outputs.
**Acceptance:**
- [x] With a tool missing (e.g. `PATH` without `iconutil`), it exits non-zero and names the tool.
- [x] A run on a clean checkout produces `assets/icons/png/filecargo-{16..512}.png`,
      `assets/icons/filecargo.ico` and `assets/icons/filecargo.icns`, and the final size check
      passes.
- [x] A second run changes no file (`jj diff` is empty).
**Verify:**
- [x] `assets/icons.sh` exits 0, and `magick identify assets/icons/filecargo.ico` lists 7 sizes.
- [x] `iconutil -c iconset -o /tmp/fc.iconset assets/icons/filecargo.icns` lists 10 images.
- [x] Manual: look at the 16/32/256 PNGs and one icns layer. The icns tile has a transparent
      margin; the PNGs are full bleed.
**Dependencies:** none
**Files:** `assets/icons.sh`, `assets/icons/**` (generated)
**Scope:** S

### T2: `filecargo.app` built locally with `make-app.sh`
**Description:**
- Add `assets/macos/Info.plist` (template, keys from the spec, placeholders `__VERSION__` and
  `__MIN_MACOS__`).
- Add `assets/macos/make-app.sh <binary> <version> <out-dir>`:
  1. Assembles `filecargo.app`.
  2. Fills `LSMinimumSystemVersion` from `vtool -show-build`.
  3. Lints, ad-hoc signs the whole bundle, then verifies.
- First amend `SPEC-branding.md` (mac-app + Project Structure) for the script.
- Build a release GUI locally and launch the bundle.
**Acceptance:**
- [x] `make-app.sh` on `target/release/filecargo` produces a bundle that passes `plutil -lint`
      and `codesign --verify --strict --verbose=2`.
- [x] The bundle's `Info.plist` has the workspace version and the binary's `minos`.
- [ ] `open filecargo.app` starts the GUI. The Dock, ⌘-Tab and Finder show the logo at the size
      of neighbouring apps, with no grey container. Otherwise, add the Apple-mask step to
      `icons.sh`, regenerate, and re-check.
- [x] Bad arguments (missing binary, missing version) exit non-zero with a usage line.
**Verify:**
- [x] `cargo build --release -p filecargo-gui && assets/macos/make-app.sh target/release/filecargo 0.1.0 /tmp/fc-app`
- [x] `/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' /tmp/fc-app/filecargo.app/Contents/Info.plist`
- [ ] Manual: Dock screenshot shared with the user (Checkpoint A).
**Dependencies:** T1
**Files:** `assets/macos/Info.plist`, `assets/macos/make-app.sh`, `SPEC-branding.md` (amendment), `assets/icons.sh` + `assets/icons/filecargo.icns` (only if the mask fallback is needed)
**Scope:** S

### Checkpoint A: macOS icon looks right
- [ ] T1–T2 acceptance met. `cargo test` and the GUI clippy/test are green.
- [ ] The user has seen the Dock icon and approved it. Spec open question 1 is recorded as
      resolved.

---

## Phase 2: Runtime icons (verified in CI)

### T3: Windows exe icon resource
**Description:**
- Add `crates/filecargo-gui/windows/filecargo.rc` (`1 ICON "filecargo.ico"`).
- Add a `build.rs` that, only for Windows targets, compiles it with
  `embed_resource::compile_for(.., ["filecargo"], ParamsIncludeDirs([<abs assets/icons>]))` and
  fails the build on error. It emits `rerun-if-changed` for the `.rc` and `.ico`.
- Add `embed-resource = "3.0.11"` (the version already in the lockfile; `manifest_required`) to `[workspace.dependencies]` and to the gui's
  `[build-dependencies]`.
**Acceptance:**
- [x] On macOS and Linux, `build.rs` is a no-op: it doesn't invoke a resource compiler, and the
      GUI builds and tests as before.
- [ ] (pending CI) On Windows, `filecargo.exe` contains an `RT_GROUP_ICON` with id 1 (gpui's `load_icon()`
      picks it up). A missing `.ico` fails the build.
- [ ] (pending CI) Test binaries don't get the resource (`compile_for` targets the `filecargo` bin only).
**Verify:**
- [x] Local (macOS): `cargo clippy -p filecargo-gui --all-targets -- -D warnings && cargo test -p filecargo-gui`
- [ ] CI: `gui (windows-latest)` green (Checkpoint B).
- [ ] Manual (Windows, Checkpoint B): Explorer shows the exe icon; title bar, taskbar and Alt-Tab
      show the logo.
**Dependencies:** T1
**Files:** `crates/filecargo-gui/build.rs`, `crates/filecargo-gui/windows/filecargo.rc`, `crates/filecargo-gui/Cargo.toml`, `Cargo.toml`, `Cargo.lock`
**Scope:** S

### T4: X11 window icon
**Description:**
- Add `filecargo_gui::icon::window_icon() -> Option<Arc<image::RgbaImage>>`
  (`cfg(target_os = "linux")`), which decodes the embedded 256 px PNG.
- Use it for `WindowOptions.icon` in `main.rs` behind the same `cfg`. A decode failure prints
  `filecargo: cannot decode the window icon: …` and the window opens without an icon.
- Add `image` to `[workspace.dependencies]` (0.25, `default-features = false`, `png`) and to the
  gui's Linux-only dependencies.
- First amend `SPEC-branding.md` (`eprintln!` instead of `tracing::warn!`).
**Acceptance:**
- [x] Unit test (Linux; also run on macOS with the cfg temporarily widened): `window_icon()` returns `Some` with a 256×256 image.
- [x] macOS and Windows builds neither depend on `image` through filecargo-gui nor embed the
      PNG (`cargo tree -p filecargo-gui -e normal --target aarch64-apple-darwin -i image` shows
      it only via gpui-pre).
- [x] `Cargo.lock` gains no new package (image 0.25.10 is already there).
**Verify:**
- [x] Local (macOS): GUI clippy and tests green (the Linux code isn't compiled here).
- [ ] (pending CI) CI: `gui (ubuntu-latest)` clippy and test green, with the icon test run (Checkpoint B).
- [ ] Manual (Linux X11, Checkpoint B): window and taskbar show the logo with no desktop file
      installed.
**Dependencies:** T1
**Files:** `crates/filecargo-gui/src/icon.rs` (new), `crates/filecargo-gui/src/lib.rs`, `crates/filecargo-gui/src/main.rs`, `crates/filecargo-gui/Cargo.toml`, `Cargo.toml`, `SPEC-branding.md` (amendment)
**Scope:** M

---

## Phase 3: Packaging and docs

### T5: `release.yml` dispatch mode and the macOS `.app` archive
**Description:**
- Add `workflow_dispatch` to `release.yml`:
  - The tag/version check runs only on tags.
  - `publish` runs only on tags.
  - Archive names replace `/` in the ref name with `-`.
- On the macOS legs, package with `assets/macos/make-app.sh`. The archive holds `filecargo.app`,
  `filecargo-tui`, README and licenses, and no bare `filecargo`.
- Update the README Install section for macOS: move the app to `/Applications`, then allow it in
  Privacy & Security on first launch.
**Acceptance:**
- [ ] (pending CI) A dispatch run on a branch builds 6 archives as artifacts, and no release is created.
- [x] A tag push behaves as before (the check runs, and publish runs).
- [x] macOS archives unpack (simulated locally: tar, extract, `codesign --verify --strict`) to a `filecargo.app` that passes `codesign --verify --strict`.
**Verify:**
- [x] `actionlint .github/workflows/release.yml` if installed (installing it needs approval).
      Otherwise `ruby -ryaml -e 'YAML.load_file(ARGV[0])' .github/workflows/release.yml`.
- [ ] (pending CI) Dispatch run at Checkpoint B: `gh run download`, then `tar tzf` shows the bundle layout.
**Dependencies:** T2
**Files:** `.github/workflows/release.yml`, `README.md`
**Scope:** S

### T6: Linux desktop entry and icons in the archive
**Description:**
- Add `assets/linux/filecargo.desktop` (keys from the spec, `StartupWMClass=filecargo`).
- On the Linux legs: add `desktop-file-utils` to the apt line, run `desktop-file-validate`, and
  put `share/applications/filecargo.desktop` and `share/icons/hicolor/<n>x<n>/apps/filecargo.png`
  (every PNG size) into the archive.
- Update the README Install section for Linux: copy `share/` to `~/.local/share/` and the binaries
  into a directory on `PATH`.
**Acceptance:**
- [ ] (pending CI) `desktop-file-validate` passes in the release job, and a failure stops packaging.
- [ ] (pending CI) Linux archives contain the `share/` tree with 8 icon sizes.
- [ ] (pending manual) Following the README on Wayland shows the logo in the dock or launcher.
**Verify:**
- [ ] (pending CI) Dispatch run at Checkpoint B: `tar tzf` on a Linux archive lists the `share/` tree.
- [ ] (pending manual) Manual (Linux Wayland, Checkpoint B), if available.
**Dependencies:** T1, T5 (dispatch mode to verify)
**Files:** `assets/linux/filecargo.desktop`, `.github/workflows/release.yml`, `README.md`
**Scope:** S

### T7: README logo header
**Description:** Put `<p align="center"><img src="assets/logo.svg" width="128" alt="filecargo logo"></p>`
above the title in `README.md`.
**Acceptance:**
- [ ] (pending GitHub) The logo is centered above `# filecargo` on GitHub, in light and dark themes.
**Verify:**
- [ ] (pending GitHub) Manual: GitHub rendering after push (Checkpoint B).
**Dependencies:** none
**Files:** `README.md`
**Scope:** XS

### T8: SMOKE Branding section, spec and index status
**Description:**
- Add a *Branding* section to `crates/filecargo-gui/SMOKE.md` (the checks from the spec's
  Testing Strategy, per OS).
- Set `SPEC-branding.md` and this plan to implemented once Checkpoint B passes.
- Update the `SPEC.md` index line.
**Acceptance:**
- [x] SMOKE lists the macOS, Windows, Linux X11, Linux Wayland and README checks, each with the
      archive it applies to.
- [x] Spec, plan and index statuses match reality (built, awaiting Checkpoints A and B).
**Verify:**
- [x] Manual: read-through.
**Dependencies:** T2–T7
**Files:** `crates/filecargo-gui/SMOKE.md`, `SPEC-branding.md`, `SPEC.md`, `tasks/branding/plan.md`
**Scope:** XS

### Checkpoint B: Complete
- [ ] The user pushes the branch (or asks me to). CI is green: check × 3, integration, gui × 3.
- [ ] `gh workflow run release.yml --ref <branch>` produces 6 archives, and no release is created.
- [ ] The SMOKE Branding checks pass on the downloaded archives (macOS here; Windows/Linux as
      available).
- [ ] Spec success criteria 1–7 are met.
