# Plan: ticket #38 — Scope a repo at open, surface action failures, quit always works
Epic: #8. Date: 2026-09-13. Status: ready. Base: `main` @ 82581fd.

## Files that change

- `src/main.rs` — repo scoping: `resolve_repo` now returns a `RepoScope { repo, explicit }`
  so the board can tell an *explicit* scope (env / `$HERDR_PLUGIN_CONFIG_DIR/repo` /
  `HERDR_PLUGIN_CONTEXT_JSON.repo`) from the dev fallback; on the fallback it seeds a
  visible "not scoped" status. The pure decision is factored into `resolve_repo_from`
  (unit-testable) with a `#[cfg(test)] mod tests` at the end of the file.
- `src/ui/render.rs` — add a status line row to the layout and `render_status`: a set
  `app.status` renders the action outcome or why a key could not act; otherwise the
  line shows the scoped repo + focused card (or "no card focused"). The status is
  always a visible row — a dead key is never silent.

Rejected: hiding the status in the command bar (no room; a dedicated line is legible);
making `sync` auto-run at startup to "guarantee a focused card" (violates the
explicit-only trigger rule); scoping the repo from the workspace id alone (a workspace
is not a repo).

## Order of work

1. [main] `RepoScope` + `resolve_repo_from(env, config, context)` pure decision, with
   priority env → config file → context JSON → dev fallback.
2. [main] `resolve_repo` reads the three sources from env/fs; `run` seeds a
   "not scoped …" status when the fallback fires.
3. [render] Add the status `Constraint::Length(1)` row + `render_status`.
4. [test] Unit tests for the scoping priority/fallback; render tests for the status
   line (set status shown; empty board shows "no card focused").

## Validation Strategy

- Unit — `resolve_repo_from` priority + fallback (`cargo test --locked --bin herdr-board`).
- Render — `TestBackend` asserts the status line surfaces a set status and the
  "no card focused" hint on an empty board.
- Full suite + clippy/fmt — crew rule 5.

## Proof

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
```
Evidence in `intent/8-board-plugin/tickets/38-scope-repo-and-surface-failures/evidence/`.

## Risks

- **Status-line row steals a column row.** The board area loses one row to the status
  line. Acceptable: legibility over density, and the row doubles as the scoped-repo
  hint when idle. No scroll region added.
- **Config-file source ambiguity.** `$HERDR_PLUGIN_CONFIG_DIR/repo` is a plain
  `owner/repo` file; a malformed value falls through to the next source rather than
  erroring. Documented in the status text when the fallback fires.
- **Quit already worked** (keymap + `run_loop` intercept `Quit` before dispatch). No
  code change; existing `keymap_resolves_bindings` covers `q`/`Esc`/`Ctrl-C`.
