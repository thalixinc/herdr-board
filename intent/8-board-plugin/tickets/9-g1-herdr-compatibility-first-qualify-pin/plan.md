# Plan: ticket #9 — G1 — Herdr compatibility FIRST: qualify/pin herdr + prove native board tab
Epic: #8. Date: 2026-09-12. Status: draft.

## Files that change
- `herdr-plugin.toml` (new) — plugin manifest, `min_herdr_version = "0.9.0"`, `[[panes]]` board entrypoint
- `Cargo.toml` — add `ratatui` + `crossterm` (TUI stack for the pane command)
- `src/main.rs` — replace print stub with a minimal TUI that renders plugin identity + injected Herdr context and stays alive
- `src/lib.rs` — keep `PLUGIN_NAME` / `version()`; no change needed
- `docs/herdr-compat.md` (new) — committed compatibility note: what herdr 0.9.0 plugin/tab/pane supports, and what it does not
- `intent/8-board-plugin/tickets/9-g1-herdr-compatibility-first-qualify-pin/evidence/` — proof artifacts

## Order of work
1. Read the herdr 0.9.0 plugins doc + CLI `--help` to qualify the plugin/tab/pane API (no assumptions).
2. Add `ratatui`/`crossterm`; write the minimal TUI `main.rs`.
3. Write `herdr-plugin.toml` (pin 0.9.0; `[[panes]] id="board" placement="tab"`).
4. `cargo build --release` + `cargo test` + `clippy`/`fmt` green.
5. `herdr plugin link .` then `herdr plugin pane open --plugin thalixinc.herdr-board --entrypoint board --placement tab`.
6. Capture evidence: `herdr tab list`, `herdr pane list`, `herdr pane read --source visible`, `herdr plugin list`.
7. Write `docs/herdr-compat.md` from observed behavior; commit; open PR.

## Validation Strategy
- Unit: `cargo test --locked` (existing `src/lib.rs` tests still pass).
- Lint: `cargo clippy --locked --all-targets -- -D warnings`; `cargo fmt --all -- --check`.
- E2E (the real gate): the linked plugin's `board` pane opens as a **native tab** and the TUI renders the injected `HERDR_*` context — verified via `herdr tab list` / `herdr pane list` / `herdr pane read --source visible`.

## Proof
The definition of done is the live tab, not the compile:
```sh
cargo build --release --locked            # binary exists at target/release/herdr-board
herdr plugin link "$PWD"                   # manifest validated + plugin registered
herdr plugin list                          # shows thalixinc.herdr-board
herdr plugin pane open --plugin thalixinc.herdr-board --entrypoint board --placement tab
herdr tab list                             # new tab present
herdr pane list                            # pane runs target/release/herdr-board
herdr pane read <pane-id> --source visible # rendered TUI with HERDR_PLUGIN_ID + pane/tab/workspace ids
```
Expected: every command exits 0 and the read shows the board title + injected context. Evidence files land in `evidence/`.

## Risks
- **Manifest placement `tab` may not be a valid manifest value** (the open request lists `overlay|split|tab|zoomed`; the manifest examples only show `overlay`/`popup`). Mitigation: if `link` rejects `placement = "tab"`, set manifest `overlay` and prove `--placement tab` at open time (the founder's stated command).
- **TUI on alternate screen may not appear in `pane read` scrollback.** Mitigation: read `--source visible` (the rendered viewport), and keep the proof on the screen.
- **`cargo build --release` is required before `pane open`** because `plugin link` does not run build commands (build commands run only on GitHub `install`). The binary must exist at `target/release/herdr-board` first.
- **herdr 0.9.0 may not support the assumed tab mechanism.** If link/open reveals no native-tab support, STOP and report honestly (no workaround).
