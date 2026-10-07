# Implementation Plan: branding

> Spec: [SPEC-branding.md](../../SPEC-branding.md) · Tasks: [todo.md](todo.md) · Status: **draft, awaiting review**

## Overview

Put the logo everywhere an OS shows the GUI's icon, and at the top of the README. The work
splits into the six features of the spec:
- `icon-assets` (the derived files)
- three ways the icon reaches the OS: `mac-app`, `win-icon`, `x11-icon` + `linux-desktop`
- the README (`readme-logo`)

The release workflow carries the packaging. Every task leaves the workspace building and CI
green. Plain `cargo build` / `cargo test` never compile the GUI, and nothing here changes that.

The riskiest unknown, how macOS 26+ draws a legacy `.icns`, can be checked on this Mac
(Darwin 27), so the plan goes there first. The Windows and Linux paths can only be checked in CI
from this machine, so they are batched before one push and one `workflow_dispatch` run.

## Dependency graph

```
icons.sh + committed outputs (T1)
    ├── .icns ──────── make-app.sh + Info.plist (T2) ── local Dock check ── [Checkpoint A]
    │                                     │
    ├── .ico ───────── build.rs + .rc (T3)│
    ├── png/256 ────── window_icon() (T4) │
    │                                     ▼
    │                 release.yml: workflow_dispatch + macOS .app archive (T5)
    └── png/* ──────── .desktop + Linux share/ tree (T6, needs T5's dispatch)

readme-logo (T7)                                  standalone
SMOKE Branding section + spec/doc status (T8)     needs T2–T7
                                                  ── [Checkpoint B: push, CI, dispatch run, smoke]
```

T3, T4 and T7 don't depend on each other and can be done in any order (or in parallel) after T1.

## Architecture decisions

### Derived icons: one script, committed outputs (spec `icon-assets`)

- `assets/icons.sh` (bash, `set -euo pipefail`):
  1. Checks that `rsvg-convert`, `magick` and `iconutil` exist, and exits naming the missing one.
  2. Renders the full-bleed PNGs with `rsvg-convert -w N -h N`.
  3. Builds the `.ico` with `magick` from the 16–256 renders.
  4. Builds the Apple-grid `.icns`: renders the tile at 824/1024 of each iconset size, centers it
     on a transparent canvas with `magick -gravity center -extent`, then runs `iconutil -c icns`.
  5. Ends with a size check: `magick identify` on the PNGs/`.ico`, and `iconutil -c iconset` back
     out of the `.icns`.
- rsvg's rendering of our filters was already compared against WebKit's (Quick Look) when the SVG
  was reworked: identical.

### macOS bundle assembly lives in a script, not in YAML (amends the spec)

- `assets/macos/make-app.sh <filecargo binary> <version> <out dir>`:
  1. Builds `filecargo.app` from `assets/macos/Info.plist` (placeholders `__VERSION__`,
     `__MIN_MACOS__`) and the `.icns`.
  2. Reads `LSMinimumSystemVersion` from the binary itself (`vtool -show-build` → `minos`), so it
     always matches what the build really targets (Intel and arm64 differ).
  3. `plutil -lint`, `codesign --force --sign -`, then `codesign --verify --strict`.
- The same script runs locally (T2) and in `release.yml` (T5). This is the only way to look at
  the real Dock icon before CI.
- **Spec amendment (in T2):** add `assets/macos/make-app.sh` to the spec's mac-app section and
  Project Structure.

### Windows resource (spec `win-icon`)

- `build.rs` returns early unless `CARGO_CFG_TARGET_OS == "windows"`. Otherwise it runs
  `embed_resource::compile_for("windows/filecargo.rc", ["filecargo"],
  ParamsIncludeDirs([<abs path of assets/icons>]))` and passes the result to
  `.manifest_optional()?`. Any error fails the build.
- It emits `cargo:rerun-if-changed` for the `.rc` and `.ico`.
- `compile_for` (not `compile`) links the resource into the `filecargo` bin only, not into the
  test binaries.
- The include dir is absolute (`CARGO_MANIFEST_DIR/../../assets/icons`): `rc.exe` resolves
  relative include paths against its own working directory, not the crate.

### X11 icon (spec `x11-icon`)

- New module `filecargo_gui::icon` with `window_icon()`, `cfg(target_os = "linux")`. In
  `main.rs`, `WindowOptions { icon: window_icon(), .. }` sits behind the same `cfg`.
- **Spec amendment (in T4):** a decode failure is reported with
  `eprintln!("filecargo: cannot decode the window icon: {error}")`, not `tracing::warn!`. The
  gui crate has no `tracing` dependency, and `main.rs` already reports window errors this way.
- `image` is declared once in `[workspace.dependencies]`
  (`version = "0.25", default-features = false, features = ["png"]`) and used only in
  `[target.'cfg(target_os = "linux")'.dependencies]` of `filecargo-gui`. Feature unification
  with gpui-pre's `image 0.25.10` means nothing new compiles.

### Release workflow (spec `mac-app`, `linux-desktop`)

- Adds a `workflow_dispatch` trigger:
  - The tag/version check runs only when `github.ref_type == 'tag'`.
  - The `publish` job gets `if: ${{ !cancelled() && github.ref_type == 'tag' }}`.
  - Archive names use the ref name with `/` replaced by `-`, so a dispatch from `feat/x` still
    produces valid file names.
- macOS legs: `make-app.sh` → archive holds `filecargo.app`, `filecargo-tui`, README and licenses.
- Linux legs: `share/applications/filecargo.desktop` + `share/icons/hicolor/<n>x<n>/apps/filecargo.png`.
  `desktop-file-validate` runs before packaging, with `desktop-file-utils` added to the existing
  apt line.
- Windows legs: unchanged. The icon is inside the exe.

## Task list

### Phase 1: Assets and the macOS check (local)
- [ ] T1: `icons.sh` and the committed icon set
- [ ] T2: `filecargo.app` built locally with `make-app.sh`

### Checkpoint A: macOS icon looks right
- [ ] The Dock / ⌘-Tab / Finder show the logo at neighbour size, with no grey container. If a grey
      container appears, apply the Apple mask in `icons.sh` (spec open question 1) and re-check.
- [ ] The user has looked at the Dock icon.

### Phase 2: Runtime icons (verified in CI)
- [ ] T3: Windows exe icon resource
- [ ] T4: X11 window icon

### Phase 3: Packaging and docs
- [ ] T5: `release.yml` dispatch mode and the macOS `.app` archive (+ README macOS install)
- [ ] T6: Linux desktop entry and icons in the archive (+ README Linux install)
- [ ] T7: README logo header
- [ ] T8: SMOKE Branding section, spec and index status

### Checkpoint B: Complete
- [ ] The user pushes the branch (or asks me to), and CI is green on all jobs (check, integration,
      gui × 3).
- [ ] `gh workflow run release.yml --ref <branch>` produces 6 archives and publishes nothing.
- [ ] The SMOKE Branding section passes on the downloaded archives (macOS here; Windows/Linux as
      available).
- [ ] Spec success criteria 1–7 are met. The spec is set to implemented.

## Risks and Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| macOS 26+ draws our `.icns` inside a grey container (legacy-icon "jail") | Med | Checked first, in T2, on this Mac. The fallback is clipping the macOS render to Apple's squircle in `icons.sh` (same task, no new files). |
| `rc.exe` / `embed-resource` misbehaves on the `windows-11-arm` release runner | Med | The `gui (windows-latest)` CI job covers x86_64. The dispatch run at Checkpoint B covers arm64 before any tag. |
| The `cfg(linux)` code isn't compiled on this Mac, so a type error shows up only in CI | Low | The code is ~15 lines and mirrors the spec snippet. The `gui (ubuntu-latest)` job runs clippy and the unit test, and Checkpoint B requires it green. |
| A downloaded ad-hoc `.app` shows "damaged" instead of the "allow in Privacy & Security" prompt (a broken bundle signature) | Med | Sign the whole bundle **after** it is fully assembled, then `codesign --verify --strict` in the script. At Checkpoint B, test the downloaded archive (quarantined) on this Mac. |
| `LSMinimumSystemVersion` wrong for one arch | Low | Read from each binary with `vtool` at packaging time, never hardcoded. |
| Binary assets grow the repo (~2 MB, mostly the 1024 px icns layer) | Low | Acceptable for an app icon. They are only regenerated when the logo changes. |

## Open Questions

From the spec, still open, none blocking the plan:
1. macOS 26+ icon shape: answered at Checkpoint A.
2. Simplified artwork for 16/24 px: same artwork for now.
3. Embed gpui's Windows manifest (DPI awareness) in the same `.rc`: not in this plan unless you
   say so. It would be a small follow-up task after T3.
