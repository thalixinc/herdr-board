# Plan: ticket #27 — VS4 — TUI board integration (native kanban tab)
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change

### dev-1 — board model + rendering (worktree `/Users/chrismckenna/development/herdr-board-d1`, branch `vs/vs4-dev1`)
- `src/ui/mod.rs` (new) — module root + re-exports
- `src/ui/model.rs` (new) — `BoardModel`, `BoardColumn` (known columns + trailing "uncategorized"), `Filters`, `ParentRef`, `is_epic`, `parent_of`, grouping + filtering + 2-level hierarchy; pure functions over `Vec<Card>`
- `src/ui/app.rs` (new) — `App` (holds `Store`, `RepoIdentity`, `model`, `filters`, `focus`, `status`), `App::new`/`reload`/`select_next`/`select_prev`/`focused`/`set_filter`
- `src/ui/render.rs` (new) — `render(frame, &App)`: column layout, card rendering (state badge, label chips, assignee/milestone, factory badge, conflict badge + inline diff on focus), filter bar, status line
- `src/lib.rs` — `pub mod ui;` + re-exports

### dev-2 — actions/keybindings + data-layer wiring (worktree `/Users/chrismckenna/development/herdr-board-d2`, branch `vs/vs4-dev2`)
- `src/ui/actions.rs` (new) — `Action` enum + `KeyMap` (keybinding → `Action`) + `dispatch(&mut App, Action, &Deps)`
- `src/ui/event.rs` (new) — the crossterm event loop (poll → dispatch → `app.reload()` → `render`); no timer, no tick-driven sync
- `src/outbox/cards.rs` — add `set_column(identity, column)` (`UPDATE cards SET column = …`, board-local, no GitHub write)
- `src/outbox/draft.rs` — add `list_drafts() -> Vec<Draft>` (enumerate unpublished drafts for the publish action)
- `src/main.rs` — replace G1's `run`/`draw` placeholder bodies with the `App` + event loop + wiring (`Store::open`, `RealGitHubClient::new(Credentials::from_env())`, `RealHandoffTransport`, scoped `RepoIdentity`); keep `main()` terminal setup/teardown + the `HERDR_*` context read
- `src/lib.rs` — re-export `Action`, `dispatch`, `Deps`

`herdr-plugin.toml` is **unchanged** (pane entry `./target/release/herdr-board`). The TUI is a thin view: it adds no sync/push/factory logic, only the two store primitives below.

## Resolved decisions (design left these open — resolved here, or named assumptions)
- **App + event loop** — `App { store: Store, repo: RepoIdentity, model: BoardModel, filters: Filters, focus: Focus, status: Option<String> }`. `App::reload()` recomputes `BoardModel::from_cards(store.list_cards(owner, repo), &filters)`. The loop polls crossterm events, maps to an `Action`, runs `dispatch`, reloads, renders — **no timer, no tick-driven fetch** (sync never auto-runs). `main()` keeps G1's raw-mode/alternate-screen setup + `herdr-plugin.toml` pane command; `run`/`draw` become the `App` loop.
- **Column model** — `BOARD_COLUMNS = ["to-do", "in-progress", "done"]` (matches `DEFAULT_COLUMN`), a board-local constant, **never** derived from labels/state. Grouping is by `card.column`; an unknown value renders in a trailing "uncategorized" column (never drops a card). Move = `set_column` only — board-local, never writes GitHub, never closes, never starts work.
- **Filtering + hierarchy** — filters are a pure `Filters` predicate (label any-match, assignee, state open/closed, title substring) applied in-memory over the full `list_cards` result (no store query change); AND-combined; session-only (design q6). Hierarchy is 2-level: `is_epic` = `epic` label; `parent_of(body) -> Option<ParentRef>` parses `Parent epic: <N>` (number). Epics render as group/swimlane rows; children indent beneath; ungrouped cards float. Extensible to Initiative/Story by adding `Parent …:` kinds — the renderer is unchanged.
- **Keybindings** — `s` sync, `a`/`d` apply/defer pull conflict, `p` publish focused draft, `Shift+A`/`Shift+D` apply/discard push conflict, `f` process-with-factory, `←`/`→` (`h`/`l`) move column, `j`/`k` select, `/` filter bar, `q`/`esc`/`ctrl+c` quit. All explicit; nothing auto-fires.
- **Two additive store primitives** *(the design named one — `list_drafts` is the second)*: `set_column(identity, column)` and `list_drafts()`, both in dev-2. `list_drafts` is required to render/select the `p` target (the draft store has only `get_draft_by_id` today).
- **Repo source at startup** *(named assumption, carried from VS1 q2)* — the scoped `RepoIdentity` is read from `HERDR_PLUGIN_CONTEXT_JSON` (or `HERDR_BOARD_REPO`), falling back to a dev constant; resolved before the first render.
- **Draft↔card + target-factory UX** *(named assumptions)* — `p` operates on the focused draft from a compact drafts region; `f` resolves the card's draft via the `create_intents` outbox (`issue_number` → intent → draft) and captures `target_factory` via a one-line status-bar prompt (a full chooser is a later slice).

## Shared contract (frozen first)
Already frozen (in the repo; the TUI only calls these): `sync`, `apply_changes`, `defer_changes` (VS1); `publish_draft`, `push_changes`, `apply_push`, `discard_push` (VS2); `process_with_factory` (VS3); `list_cards`, `get_card`, `get_conflict`, `Card`, `CanonicalFields`, `Conflict`, `CardField`, `CardFieldDiff` (VS1 cards); `FactoryKind`; `Credentials`, `RealGitHubClient`; `HandoffTransport`/`RealHandoffTransport`; `Store::{open, default_path}`, `RepoIdentity`, `DEFAULT_COLUMN`.

New, authored first into both worktree bases:
- **dev-1**: `BoardModel`, `BoardColumn`, `Filters`, `ParentRef`, `is_epic(&Card) -> bool`, `parent_of(&str) -> Option<ParentRef>`, `BoardModel::from_cards(Vec<Card>, &Filters) -> BoardModel`, `App` (`new`, `reload`, `select_next`, `select_prev`, `focused`, `set_filter`), and `render(frame, &App)`.
- **dev-2**: `Action` enum, `KeyMap`, `Deps` (bundles `&dyn PullClient`, `&dyn GitHubClient`, `&dyn HandoffTransport`, `&RepoIdentity`), `dispatch(&mut App, Action, &Deps) -> Result<(), UiError>`, the event loop, and `Store::{set_column, list_drafts}`.

dev-1 is self-contained (pure model + render, no network — testable with `ratatui`'s `TestBackend` + in-memory cards). dev-2 depends only on dev-1's `App`/`BoardModel`/`render` + the frozen data layer; it touches disjoint files (`src/ui/actions|event.rs`, `src/outbox/{cards,draft}.rs`, `src/main.rs`). **Merge order:** dev-1 merges first; dev-2's worktree is cut from a base containing `src/ui/` and rebases onto dev-1.

## Order of work
1. [contract] Commit the frozen `App`/`BoardModel`/`render` + `Action`/`dispatch` signatures to both worktree bases.
2. [dev-1] `src/ui/model.rs`: `BoardModel`/`BoardColumn`/`Filters`/`ParentRef`/`is_epic`/`parent_of` + grouping/filtering/hierarchy.
3. [dev-1] `src/ui/app.rs`: `App` + `reload` + selection/filter state.
4. [dev-1] `src/ui/render.rs`: column layout + card rendering (badges/chips/diff) + filter bar + status line; `pub mod ui;` + re-exports in `src/lib.rs`; write unit + `TestBackend` render tests; gate in `herdr-board-d1`.
5. [dev-2] `src/outbox/cards.rs` + `src/outbox/draft.rs`: `set_column` + `list_drafts`.
6. [dev-2] `src/ui/actions.rs`: `Action` + `KeyMap` + `dispatch` (call frozen fns + reload).
7. [dev-2] `src/ui/event.rs`: the crossterm event loop (no timer).
8. [dev-2] `src/main.rs`: replace `run`/`draw` with the `App` + loop + wiring; re-export in `src/lib.rs`; write integration + full-flow tests; gate in `herdr-board-d2`.
9. [coordinator] Merge dev-1 + dev-2; run the full-suite gate + the `herdr plugin pane open` smoke on the merged tree.

## Validation Strategy
- Unit — `cargo test --locked --lib`: `BoardModel::from_cards` grouping (column order + uncategorized fallback); `Filters` AND-combination across label/assignee/state/title; `is_epic`; `parent_of` parsing (spacing variants, `<owner>/<repo>#<N>`, trailing whitespace).
- Render — `ratatui` `TestBackend`: render a fixed `App` and assert the buffer contains the title, an `open`/`closed` state badge, a label chip, a `⚙` factory badge, and the "issue changed; apply?" conflict marker; assert the focused conflict card surfaces the per-field diff.
- Integration — `cargo test --locked --test ui_actions` (dev-2): drive the event loop with fakes + `TestBackend`; `s` → `sync` called + model reloaded; `a`/`d` → `apply_changes`/`defer_changes`; `p` → `publish_draft`; `f` → `process_with_factory`; `←`/`→` → `set_column` (and no GitHub write); `q` exits.
- Edge cases (named fixtures): card with an unknown `column` → uncategorized; closed card + `state_reason`; CF hold/stage label rendered verbatim; empty board renders; `apply-pending` focused → diff shown, unfocused → badge only; filters cleared restore the full board.
- E2E / smoke — `cargo test --locked --test ui_flow` (full sync→render→move→render cycle against a fake client + tempdir store), plus the human gate: `cargo build --release && herdr plugin pane open --plugin thalixinc.herdr-board --entrypoint board --placement tab` renders the live board.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/ui/`.

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --test ui_actions        # keybinding -> data-layer dispatch green
cargo test --locked --test ui_flow           # full sync->render->move->render cycle green
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report >= 90% line coverage on src/ui/
cargo build --release                        # binary exists at target/release/herdr-board
herdr plugin pane open --plugin thalixinc.herdr-board --entrypoint board --placement tab   # live kanban tab renders
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% threshold; the `TestBackend` buffers show the board columns, card badges (state/labels/factory/conflict), and the "issue changed; apply?" prompt; the live tab opens as a native Herdr pane and renders the board (no placeholder). Evidence artifacts land in `intent/8-board-plugin/tickets/27-vs4-tui-board/evidence/`.

## Risks
- **Riskiest step — the thin-view boundary (dev-2 step 8 + `main.rs`).** The board's authority model depends on the TUI *never* reimplementing sync/push/factory and *never* deriving `column` from labels/state. If `dispatch` or `render` sneaks in its own sync logic, or the renderer reads `column` from a label, the board silently violates "no label↔column equivalence" / "sync never auto-runs". Mitigation: `render` is pure over `BoardModel`; `dispatch` calls only the frozen functions + `set_column`; a test asserts `←`/`→` writes only `column` (no GitHub call) and no timer path invokes `sync`.
- **`Parent epic:` parsing (dev-1 step 2).** A parser that doesn't match cf-queue's real body-line format mis-groups the hierarchy. Mitigation: a tolerant parser + unit tests over the actual formats (spacing/`#N`), and a named assumption to confirm the format against cf-queue before freezing the parser (design q3).
- **Rejected and why:** a second store for the board model (the `Store` is the single source of truth — the model is derived); deriving `column` from labels/state (REQUIREMENTS: no label↔column equivalence); a tick/timer-driven sync loop (explicit-only, `autospin=ask`); reimplementing sync/push/factory in the UI (thin view over frozen functions); persisting filters across restarts (session-only); rendering the full 4-level hierarchy now (render the 2-level epic→task that exists, stay extensible).
