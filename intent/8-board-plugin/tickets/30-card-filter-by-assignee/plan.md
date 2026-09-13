# Plan: ticket #30 — Card filtering by assignee (extensible to six board dimensions)
Epic: #8. Date: 2026-09-13. Status: ready. Base: `vs/board-ui-kanban` @ 88766b5.

## Files that change

- `src/ui/model.rs` — extend the `Filters` predicate to all six dimensions. Add three
  fields to `Filters`: `epic: Option<u64>`, `milestone: Option<String>`,
  `repository: Option<String>` (assignee/label/state/title already exist). Extend
  `Filters::matches` for the three new dimensions:
  - `epic` → `card` is the epic itself (`is_epic(card) && card.identity.number == epic`)
    **or** a child (`parent_of(&card.fields.body) == Some(ParentRef { number: epic })`).
  - `milestone` → `card.fields.milestone.as_deref() == Some(milestone)` (exact).
  - `repository` → `card.identity.canonical() == repository` (owner/repo, lowercased).
  Keep AND-across-dimensions and "empty = no filter" (already the case).
- `src/ui/app.rs` — add the interactive filter-input state machine (not a second store):
  - `enum FilterDimension { Assignee, Label, State, Epic, Milestone, Repository, Title }`
  - `enum FilterInput { Off, Active { dimension: FilterDimension, buffer: String } }`
  - a `filter_input: FilterInput` field on `App`, plus pure transition methods:
    `open` (defaults `Assignee`), `cycle_dimension`, `push_char`, `backspace`,
    `apply` (turns the active dimension's buffer into a `Filters` value via the
    existing `App::set_filter`), `clear_dimension`, `close`. `App::set_filter` is
    **unchanged** (it already replaces `filters` and calls `reload()`).
- `src/ui/actions.rs` — new `Action` variants + keymap arms + dispatch:
  - `Action::FilterOpen`, `Action::FilterChar(char)`, `Action::FilterApply`,
    `Action::FilterBackspace`, `Action::FilterNext`, `Action::FilterClearDimension`.
  - `KeyMap::resolve` change: `/` now maps to `Action::FilterOpen` (repurposed from
    today's `Action::ClearFilters`); `Tab`/`←`/`→` → `FilterNext`; `Enter` →
    `FilterApply`; `Backspace` → `FilterBackspace`; `Esc` → close-without-apply;
    `Char(c)` while the bar is `Active` → `FilterChar(c)`; `x` while the bar is open
    → `Action::ClearFilters` (clear everything, the old `/` behavior).
  - `run_action` arms drive `app.filter_input` (typing appends to `buffer`, no reload;
    `FilterApply` produces the `Filters` value then `app.set_filter`).
- `src/ui/event.rs` — **no change** (already generic: `KeyMap::resolve(key.code,
  key.modifiers)` returns the new actions; `KeyCode::Char(c)` flows through).
- `src/ui/render.rs` — `render_toolbar` gains a prompt line when
  `app.filter_input` is `Active` (`assignee > founder_` style, buffer echoed), plus
  the active-filter chips gain `[epic:#N]`, `[m:milestone]`, `[repo:o/r]` (the
  existing chips already render `[@{assignee}]` / `[label:x]` / `[“title”]`). The
  top-level `render()` vertical layout reserves a second toolbar row while the bar is
  active (toolbar is `Constraint::Length(1)` today).
- `src/ui/mod.rs` / `src/lib.rs` — re-export `FilterDimension`, `FilterInput` (only
  if tests/`main` reference them by path; a one-line `pub use` addition).

No `src/outbox`, `src/sync`, `src/factory`, `src/receiver`, or store change:
filtering is a session-local predicate over the full `list_cards` result.

## Assumptions (design defaults, sliced here — not re-routed to founder; all grounded in REQUIREMENTS.md)

- **Persistence = session-only** (VS4 default). Filters are cleared on restart; the
  store is not touched. No per-factory persistence.
- **Input = free-text + chips** (the design's free-text-with-chips shape). Extensible
  to a values chooser later; no values index now.
- **Assignee match = exact** on `card.fields.assignee` (GitHub login), matching the
  existing `Filters::matches` semantics — not display-name substring.
- `epic`/`milestone`/`repository` are **never** styled as a CF station/card agent
  (REQUIREMENTS field mapping: assignee/milestone = metadata).

## Order of work

1. [model] Add the three `Filters` fields + `matches` arms; extend the
   `filters_and_combine` unit test (exact match, epic self + child, milestone,
   repository, AND-exclusion, empty = no filter).
2. [app] Add `FilterDimension` + `FilterInput` + `App` field + transition methods
   (open/cycle/char/backspace/apply/clear/close) over the unchanged `set_filter`.
3. [actions] Add the six `Action` variants, rework `KeyMap::resolve` (`/` opens,
   `x` clears-all, `Enter` applies, `Esc` closes, `Tab`/`←`/`→` cycle, `Char` types),
   and the `run_action` arms.
4. [render] `render_toolbar` prompt line + new chips + the two-row toolbar layout;
   re-export `FilterDimension`/`FilterInput`.
5. [test] App-state + render tests (below); run the scoped commands; record evidence.

## Validation Strategy

- Unit — `cargo test --locked --lib ui::model`: extended `filters_and_combine`
  covers assignee exact + non-match; `epic` matches the epic itself **and** its
  `Parent epic: N` child, and excludes a non-child; `milestone` exact;
  `repository` `owner/repo` match; AND-combination (one non-matching dimension
  excludes); empty dimension = no filter.
- App state — `cargo test --locked --test ui_actions`: typing into the buffer then
  `FilterApply` sets `filters.assignee` and reloads a filtered model; `Esc` closes
  without applying; empty-Enter clears the dimension; `Tab` cycles dimension.
- Render — `cargo test --locked --lib ui::render` (`ratatui::TestBackend`): an active
  assignee filter shows `assignee > founder` in the toolbar and only matching cards
  in the buffer (assert a non-matching card's title is absent); chips render
  `[epic:#N]`/`[m:m]`/`[repo:o/r]`.

## Proof

```sh
cargo test --locked --lib ui::model        # extended filters_and_combine green
cargo test --locked --test ui_actions      # filter-input state machine green
cargo test --locked --lib ui::render       # filter bar + chips + filtered columns green
```
Expected: all three exit 0; `filters_and_combine` covers the six dimensions; the
`TestBackend` buffer shows the active `assignee > …` prompt and omits non-matching
cards. (Full-suite + clippy/fmt run once at integration, per crew rule 5.)
Evidence lands in `intent/8-board-plugin/tickets/30-card-filter-by-assignee/evidence/`.

## Risks

- **`/` repurpose (actions step 3).** `/` is bound to `ClearFilters` today; the
  design moves clear-everything to `x` while the bar is open. A dev that leaves a
  stray `/ → ClearFilters` arm silently breaks the new bar. Mitigation: a
  `ui_actions` test asserts `/` opens the bar and does **not** clear, and `x` (bar
  open) clears all.
- **Toolbar layout (render step 4).** The toolbar is one `Length(1)` row; adding a
  prompt line must grow the layout, not overflow into the columns region.
  Mitigation: a `TestBackend` render asserts both the prompt line and the tabs/chips
  render without clipping.
- **`epic` matching reuses the hierarchy parser.** `parent_of` is the same parser the
  hierarchy renderer trusts, so filter-by-epic and group-under-epic cannot disagree —
  but a child card whose `Parent epic: N` is absent/malformed will not match. That is
  correct (the hierarchy renderer floats it too); no change.
- **Rejected and why:** a second store/query for filtering (the predicate is a pure
  view over `list_cards`); a cycling hotkey selector over existing assignees (needs a
  values index — free-text + chips is the VS4 shape); persisting filters (session-only
  per VS4).
