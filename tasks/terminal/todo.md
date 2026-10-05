# Tasks: `terminal`

> Plan: [plan.md](plan.md) · Spec: [SPEC-terminal.md](../../SPEC-terminal.md) · DoD as in [tasks/transfer/todo.md](../transfer/todo.md)

### T1: Key encoding (S)
`Key`, `Mods`, `Modes`, `encode`; Ctrl-letter → C0, Alt → ESC prefix, modified arrows `CSI 1;<m>X`, F1–F12, application cursor mode.
- **Accept:** AC1 table (every variant × {none, Ctrl, Alt, Shift} × both cursor modes).
- **Verify:** `cargo test -p filecargo-terminal keys`
- **Files:** `crates/filecargo-terminal/{Cargo.toml,src/lib.rs,src/keys.rs}`, workspace `Cargo.toml`
- **Deps:** remote-fs Checkpoint B (for the `ShellChannel` types used in T2)

### T2: TerminalHandle (M)
`spawn(channel, size, scrollback)`, `TermCallbacks: vt100::Callbacks` (title, bells), feeder task (output → parser, generation++), input forwarding, `send_key` / `send_text` / `paste`, `resize` coalescing, `scroll`, `selection_text`, `status`, `title`, `close`.
- **Accept:** AC2–AC6 against an in-memory fake `ShellChannel`.
- **Verify:** `cargo test -p filecargo-terminal --test handle` (paused time for coalescing)
- **Files:** `src/handle.rs`, `tests/support/mod.rs`, `tests/handle.rs`
- **Deps:** T1

### T3: Docker integration (S)
Open a shell through `remote_fs::connect` + `ShellOpener`, run `printf 'ok\n'`, resize to 100×30, check `stty size`. Add to the `cargo it` alias.
- **Accept:** AC7 locally and in the CI integration job.
- **Verify:** `cargo it`
- **Files:** `tests/integration.rs`, `.cargo/config.toml`
- **Deps:** T2

### Checkpoint: module complete (7 AC, coverage ≥ 80 %, hand-off notes, spec → done, human review)
