# Design: Card filtering — assignee first, extensible to the six board dimensions

Epic: #8 · Ticket: #30 · Seat: Brainstorm · Date: 2026-09-13 · Stage: plan (design only)

## Decoded requirement

REQUIREMENTS §Board UX item 3: **filtering is first-class**, with dimensions *label, assignee,
epic, milestone, state, repository*. Ticket #30 names **assignee as the first** dimension to land,
but the architecture must be extensible to the other five without a redesign.

The truth on the ground (read, not assumed): a `Filters` predicate already exists
(`src/ui/model.rs`) and covers *labels* (any-match), *assignee* (exact), *state* (open/closed),
and *title* (substring) — but **`epic`, `milestone`, and `repository` are absent**, and there is
**no interactive input surface at all**: `/` is bound to `ClearFilters`, the toolbar renders only
All/Open/Closed tabs plus passive chips, and `App::set_filter` is never reachable from a keypress
that edits a filter. So "filtering by assignee" today is a model field with no UI to set it.

This slice therefore has two halves: (1) make the filter *model* cover all six dimensions with
assignee fully wired, and (2) add the *interactive* filter input so a human can actually set an
assignee (and later the other dimensions) without code.

---

## Filter model (extend, do not replace)

`Filters` lives in `src/ui/model.rs`; it is a pure, `Eq`, `Default` struct applied in
`BoardModel::from_cards(cards, &filters)` *before* grouping, so filtering never touches the store
(`list_cards` still returns the full repo — the predicate is a pure view). Extend it to the six
dimensions, keeping assignee as the first fully-wired one:

| Dimension | Field (new state) | Match rule |
|---|---|---|
| assignee *(first)* | `assignee: Option<String>` (exists) | `card.fields.assignee == Some(assignee)` (exact) |
| label | `labels: Vec<String>` (exists) | any-match on `card.fields.labels` |
| state | `state: Option<String>` (exists) | `card.fields.state == state` |
| title | `title: Option<String>` (exists) | case-insensitive substring |
| epic | `epic: Option<u64>` **(new)** | card *is* that epic (`is_epic` + `identity.number == epic`) **or** its `parent_of(body) == Some(epic)` |
| milestone | `milestone: Option<String>` **(new)** | `card.fields.milestone == Some(milestone)` (exact) |
| repository | `repository: Option<String>` **(new)** | `card.identity.canonical()` (`owner/repo`) == filter |

- **AND across dimensions** stays (already the case in `Filters::matches`). Any *empty* dimension
  is "no filter".
- `epic` matches by issue number, reusing the existing `is_epic`/`parent_of` helpers — the same
  parser the hierarchy renderer already trusts, so "filter by epic" and "group under epic" cannot
  disagree.
- `repository` matches `card.identity.canonical()` (owner/repo), reusing `Identity::canonical`
  (`src/digest/canonical.rs`). The tab is already scoped to one `RepoIdentity`, so this filter is
  a no-op in the single-repo board **and is deliberately present** for the future multi-repo
  aggregation the VS4 design already named — no extra machinery, just the predicate.
- `milestone`/`assignee` are exact `Option<String>` matches — never styled as a CF station/agent
  (REQUIREMENTS field mapping: "assignee/milestone = metadata; never = CF station/card agent").

This is a **pure predicate extension**: no store query change, no schema change, no data-layer
work. `CanonicalFields` already carries `assignee` and `milestone`; `Identity` already carries
`owner`/`repo`/`number`.

---

## Interactive filter input (the missing half)

The toolbar must let a human *set* a filter, assignee first. Add a filter-input mode to `App`
(`src/ui/app.rs`) — a small state machine, not a second store:

```
enum FilterInput {
    Off,
    Active { dimension: FilterDimension, buffer: String },
}
// FilterDimension = Assignee | Label | State | Epic | Milestone | Repository | Title
```

- **`/` opens the filter bar** (repurposed from today's `ClearFilters`): defaults to the
  **assignee** dimension (ticket order — assignee first), `Tab`/`←`/`→` cycle the active
  dimension, typing appends to `buffer`, `Enter` applies (turns `buffer` into the active
  `Filters` field via `App::set_filter`, which already reloads), `Esc` closes without applying,
  and an empty `Enter` clears that dimension. `ClearFilters` (clear everything) moves to a
  modifier or a dedicated key (`x` while the bar is open) rather than `/`.
- **`App::set_filter(filters)` is unchanged** — it already replaces `filters` and calls `reload()`.
  The new code only *produces* the `Filters` value from the input state.
- **Char entry.** The event loop already has `KeyCode::Char(c)` and `KeyModifiers`; text entry is
  the existing `KeyMap::resolve` path extended with a filter-input arm (`Action::FilterChar(char)`,
  `Action::FilterApply`, `Action::FilterBackspace`, `Action::FilterNext`, `Action::FilterClear`).
  No new event source, no timer.
- **Rendering.** `render_toolbar` (`src/ui/render.rs`) gains a prompt line when the bar is active:
  `assignee > founder_` plus the existing active-filter chips. The chips already render
  `[@{assignee}]` / `[label:x]` / `[“title”]`; add `[epic:#N]`, `[m:milestone]`, `[repo:o/r]`.

Concrete files: `src/ui/model.rs` (predicate), `src/ui/app.rs` (`FilterInput` state),
`src/ui/actions.rs` (actions + keymap), `src/ui/event.rs` (pass-through — already generic),
`src/ui/render.rs` (bar + chips). No `src/outbox`, `src/sync`, or store changes.

---

## Data flow

```
keypress (/ then type) → Action::FilterChar(c) → dispatch(app, ..)
  → app.filter_input.buffer += c   (no reload yet)
Enter → Action::FilterApply → app.filters.assignee = Some(buffer) → app.set_filter(filters)
  → app.reload() → BoardModel::from_cards(list_cards(owner, repo), &filters)
  → render (filtered columns + active-filter chips)
Esc/empty Enter → clear the dimension (assignee → None) → reload
```

Sync never runs here; filtering is display-only. The store is still the single source of truth;
the filter is a session-local predicate (VS4 already decided session-only; persistence is an open
question below, not correctness).

---

## Validation strategy

- **Unit — `src/ui/model.rs`** (extend the existing `filters_and_combine` test): `assignee` exact
  match + non-match; `epic` matches the epic itself *and* its `Parent epic: N` children (and
  excludes a non-child); `milestone` exact; `repository` `owner/repo` match; AND-combination where
  one non-matching dimension excludes; empty dimension = no filter. Run: `cargo test --locked --lib
  ui::model`.
- **App state — `src/ui/app.rs`** (or `tests/ui_actions.rs`): typing into the filter buffer then
  applying sets `filters.assignee` and reloads a filtered model; `Esc`/empty-enter clears it;
  `Tab` cycles dimension. Run: `cargo test --locked --test ui_actions`.
- **Render — `src/ui/render.rs`** (`ratatui::TestBackend`): an active assignee filter shows
  `assignee > founder` in the toolbar and only the matching cards in the buffer (assert a
  non-matching card's title is absent). Run: `cargo test --locked --lib ui::render`.

---

## Open questions / founder questions

1. **Filter persistence (founder).** Session-only (cleared on restart) — the VS4 default — or
   persisted per-factory in the store so the board reopens with the same filters? UX decision; no
   correctness impact. Default recommendation: session-only for now.
2. **Input affordance (founder).** Free-text-with-chips (this design) vs a cycling hotkey selector
   over the *existing* values (assignees seen on the board). The former is more general and
   already the VS4 shape; the latter is less typing but needs a values index. Recommendation:
   free-text + chips, extensible to a chooser later.
3. **Assignee match semantics.** Exact vs substring (login vs display name). GitHub's assignee
   field is a login (exact is correct today); flag only if display-name matching is wanted.

None of these block the predicate; #1 and #2 are UX calls to confirm before the Dev writes the
input surface.
