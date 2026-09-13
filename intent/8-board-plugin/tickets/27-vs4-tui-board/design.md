# Design: VS4 — TUI board integration (native kanban tab)

Epic: #8 · Ticket: #27 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

Replace the G1 proof-of-tab placeholder `src/main.rs` with the real board TUI — a native herdr
tab (ratatui) that renders kanban columns, cards (title / state badge / labels-as-chips /
assignee / milestone), epic→task grouping, first-class filtering, and the explicit actions
(sync · publish · process-with-factory), with conflict states ("issue changed; apply?") as visible
prompts, never silent.

The TUI is a **thin view** over the frozen data layer: it derives a board model from
`list_cards`, renders it, and maps each keybinding to an existing function. It adds **no** new
sync/push/factory logic. Sync never auto-runs.

---

## App structure + render loop

- **`App`** holds: the `Store` (opened at startup under `HERDR_PLUGIN_STATE_DIR`), the scoped
  `RepoIdentity` (one board tab per factory), a `BoardModel` derived from `list_cards`, the filter
  state, selection/focus, and a one-line status/error message.
- **Board model** — a pure, in-memory projection recomputed from `list_cards(owner, repo)` on every
  render (and after every mutation): `Vec<Card>` → grouped by `card.column` → filtered (§5) →
  hierarchy (§4). The store is the single source of truth; the view is derived, never a second
  store.
- **Event loop** (ratatui): poll crossterm events → dispatch keybindings (§6) → each action calls
  the frozen data-layer function → reload the board model → render. No timer, no tick-driven fetch
  (sync never auto-runs).
- **Scaffolding reuse.** Keep G1's `main()` terminal setup/teardown (raw mode, alternate screen,
  `CrosstermBackend`) and the `herdr-plugin.toml` pane entry **unchanged**; replace only the
  `run`/`draw` bodies. The `HERDR_*` context (workspace/pane ids) is still read for identity
  display, not logic.

---

## Column model

- **Board-local values.** `column` is a `String` on the card (`DEFAULT_COLUMN = "to-do"`). A
  board-local **column map** fixes the order and known set — `to-do → in-progress → done` — as a
  constant, extensible, and **never derived from labels or state** (REQUIREMENTS: no
  label↔column equivalence).
- **Grouping.** Cards are grouped by `card.column`; the known columns render left-to-right in map
  order. Any card whose `column` is not in the map renders in a trailing "uncategorized" column, so
  an unexpected value never drops a card from view.
- **Move = board-local only.** Moving a card writes `column` and nothing else — it never writes
  GitHub fields, never closes the issue, never starts work.

---

## Card rendering

Each card renders, top to bottom:

- **Title** — primary line.
- **State badge** — `open` (green) / `closed` (dimmed), with `state_reason` (`completed` /
  `not_planned`) appended for closed cards. Separate from the board column.
- **Labels as chips** — each label a colored span; CF hold/stage labels render verbatim (preserved,
  not reinterpreted).
- **Assignee / milestone** — dim secondary metadata line (never styled as a station/agent).
- **Factory badge** — `factory_kind = factory-request` shows a distinct marker (e.g. `⚙`) so a
  factory card is recognizable at a glance; `ordinary` shows nothing.
- **Conflict badge** — `conflict = apply-pending` renders a highlighted **"issue changed; apply?"**
  marker on the card; when the card is focused, the per-field diff from `get_conflict(identity)`
  (old → new per `CardField`) renders as an inline prompt, never silently resolved.

The issue number/URL renders as a subtle footer (click-to-jump identity).

---

## Hierarchy (epic → task, 2 levels now)

Render the **two-level hierarchy that exists** (cf-queue's `epic` label + `Parent epic:` body
line), not the still-open Initiative/Story tiers (REQUIREMENTS: render what exists, stay
extensible):

- **`is_epic(card)`** = `card.fields.labels` contains `epic`.
- **`parent_of(body) -> Option<ParentRef>`** = parse a `Parent epic: <N>` body line; `ParentRef`
  carries the parent issue number (and later `kind` for Story/Initiative).
- **Rendering** — epics are **group/swimlane rows**; tasks (cards with a `Parent epic:` line) are
  indented beneath their epic, matched by parent number. Cards that are neither epic nor child
  render ungrouped at the top of their column.
- **Extensibility** — grouping is a pure `parent_of` function over the body; the Initiative/Story
  tiers later add more `Parent …:` line kinds + label kinds **without changing the renderer**, only
  the parser and the group header. The board never invents the data model — it consumes cf-queue's
  (G2 §4 / founder boundary).

---

## Filtering

First-class filters, applied **before render** over the in-memory `BoardModel` (the store still
returns the full repo via `list_cards`; filtering is a pure view predicate — no store query
changes):

- **By label** (any-match on chips), **by assignee**, **by state** (open/closed), and **title
  substring** search.
- **By repo** — the tab is already scoped to one `RepoIdentity`, but the filter bar exposes it for
  clarity (and future multi-repo aggregation).
- Filter state lives in `App` (session-local); combining is AND across filter kinds. A visible
  filter bar shows active filters and a clear binding.

---

## Actions + keybindings

Explicit-only: every action is a keybinding; nothing auto-fires (sync never auto-runs; `autospin=ask`).

| Key | Action | Frozen function |
|---|---|---|
| `s` | Sync (pull) | `sync(store, client, repo)` |
| `a` / `d` | Apply / defer a pull conflict (focused `apply-pending` card) | `apply_changes` / `defer_changes` |
| `p` | Publish the focused draft → issue | `publish_draft(...)` |
| `Shift+A` / `Shift+D` | Apply / discard a **push** conflict | `apply_push` / `discard_push` |
| `f` | Process with factory (focused card) | `process_with_factory(store, transport, draft_id, card_identity, target_factory)` |
| `←`/`→` (or `h`/`l`) | Move card to adjacent column (board-local) | `set_column(identity, next)` (§7) |
| `j`/`k` | Select next/previous card | — (focus only) |
| `/` | Focus the filter bar | — |
| `q` / `esc` / `ctrl+c` | Quit | — |

Sync is invoked only by `s`; the loop contains no timer and never calls `sync` on a tick.

---

## Data-layer seam

The TUI is a **pure view**; it calls only the frozen functions and re-derives the model:

- **Pull** — `sync`, `apply_changes`, `defer_changes` (VS1).
- **Push** — `publish_draft`, `push_changes`, `apply_push`, `discard_push` (VS2).
- **Trigger** — `process_with_factory` (VS3).
- **Read** — `list_cards`, `get_card`, `get_conflict`.

**One additive store primitive.** The only store op the board needs and does not yet have is a
board-local **`set_column(identity, column)`** — a single `UPDATE cards SET column = …` (no GitHub
write). This is a data-layer primitive, not view logic; it is the minimum needed for column moves.
Everything else is wired, not reimplemented.

**Wiring in `main`.** Construct `Store` + `RealGitHubClient::new(Credentials::from_env())` + a real
`HandoffTransport` once, inject them into `App`, and keep G1's `herdr-plugin.toml` pane entry and
the `HERDR_*` context read unchanged. Tests inject fakes (the same seams as VS1/VS2/VS3), so the
render loop is testable without network or a live herdr.

---

## Open questions

1. **Column-move store op.** `set_column` is the one additive primitive. Confirm it's in VS4 scope
   (vs deferring drag/move to a later slice and rendering columns read-only for now).
2. **Epic detection.** Is an epic the `epic` label, "no `Parent epic:` line + has children", or
   both? Confirm the rule cf-queue actually writes, so the group header matches reality.
3. **`Parent epic:` parsing.** The exact body-line format (`Parent epic: <N>` vs other spacing /
   `<owner>/<repo>#<N>`) — confirm against cf-queue before fixing the parser.
4. **Draft↔card selection UX.** How the user selects a draft to publish and a card to
   process-with-factory (carries VS3's draft↔card linkage question into the UI).
5. **Target-factory selection UX.** How the user picks `target_factory` in the `f` action
   (a chooser over known factories? a prompt?).
6. **Filter persistence.** Session-only filters (current) vs persisted across restarts in the
   store — a UX decision, not correctness.
7. **Repo source at startup.** The tab's `RepoIdentity` comes from manifest/`HERDR_PLUGIN_CONTEXT_JSON`
   (carried from VS1 open q2); VS4 needs it resolved before the first render.
