# Design: Fix label chip truncation

Epic: #8 · Ticket: #31 · Seat: Brainstorm · Date: 2026-09-13 · Stage: plan (design only)

## Decoded requirement

Labels are chips (REQUIREMENTS field mapping: "labels = chips"; preserve CF hold/stage labels
verbatim). The bug: label chips (and the assignee/milestone/factory badges beside them) get
**clipped mid-text** when the card is narrower than the badge row — a long or numerous label set
renders `[high-prior…` or drops chips entirely. The fix must make every chip **legible — no
clipped text** — with the smallest possible change.

## Current behavior (exact location, read from source)

`src/ui/render.rs`, `render_card`:

```rust
let mut badges: Vec<String> = vec![state_badge(card)];
for label in &card.fields.labels { badges.push(format!("[{label}]")); }
if let Some(assignee) = &card.fields.assignee { badges.push(format!("@{assignee}")); }
if let Some(milestone) = &card.fields.milestone { badges.push(format!("m:{milestone}")); }
if card.factory_kind.is_factory_request() { badges.push("⚙".to_owned()); }
let badge_text = truncate(&badges.join(" "), width.saturating_sub(2));
```

and `fn truncate(s, max)` counts **chars** and appends `…` when over `max`. The whole badge row —
state badge + every chip joined by spaces — is treated as **one string** and truncated to
`width - 2`. Two defects:

1. **Mid-chip clipping.** Truncation is applied to the *joined* string, so it cuts inside whatever
   chip happens to straddle `width - 2` — a label becomes `[high-prior…`, an assignee `@foun…`.
2. **Fixed card height ignores overflow.** `render_column` reserves `let height = 4 + diff_rows;`
   (title + one badge row + padding). There is no wrap path; the only "handling" is clipping.

## Target behavior

Badges render as a **wrapping sequence of spans** — each chip stays atomic and whole — and the card
grows by the number of wrapped badge rows. A chip is never split; if the row is too narrow, chips
flow onto the next line (word-wrap at chip boundaries), and the card's reserved height accounts for
every wrapped line.

## Minimal correct fix

Replace the `badges.join(" ")` + `truncate` with a **pure layout helper** and drive card height
from it:

```
fn badge_lines(card: &Card, max_width: usize) -> Vec<String>
```

- Iterate badges (state badge, then `[label]`, `@assignee`, `m:milestone`, `⚙` — same order and
  content as today) and greedily pack them onto lines of at most `max_width` cells, breaking
  **between** chips (a chip wider than `max_width` alone is the only thing elided, and then with a
  trailing `…` — a pathological single-chip case, not the normal one).
- Each line keeps whole chips separated by a single space; no chip text is split. The state badge
  stays first on line 1 (unchanged visual order).
- Pure and side-effect-free so it is directly unit-testable without a terminal.

Then in `render_card`, draw `badge_lines` instead of `badge_text`; in `render_column`, reserve
`height = 1 /* title */ + badge_lines.len() + diff_rows /* conflict rows */ + 1 /* padding */`
(measured with the same width used at render). `render_card` is already called with the slot
`Rect`, so the width is available before the height is finalised — compute `badge_lines` once, use
its `.len()` for the height in `render_column`, and pass the lines into `render_card` (or recompute
deterministically — same pure function, same input).

Files: **`src/ui/render.rs` only** — `badge_lines` helper, `render_card` draw path, `render_column`
height accounting, plus the helper's unit tests. No `src/ui/model.rs`, no store, no data-layer
change. The `truncate` helper remains for the title line (correct there — a title is one atomic
line, not a chip sequence).

Rejected alternatives (so the Dev does not reach for them):
- **Per-chip truncation / drop-trailing-chips with a `+N` marker.** Keeps one line but *hides*
  labels — violates "legible". Not the fix.
- **Shrinking chips to fit.** Hides text inside a chip. Same violation.
- **Fixed-height overflow into a scroll region.** More surface for no legibility gain; wrap is
  the boring correct answer.

## Data flow

```
render_column(area, column, app)
  → for each entry: width = inner.width
     lines = badge_lines(card, width)          // pure, whole chips only
     height = 1 + lines.len() + diff_rows + 1
     render_card(slot, entry, app, focused, diff_rows, &lines)
       → title line (truncate, unchanged)
       → one Line per badge line (whole chips)
       → conflict diff rows (unchanged)
```

## Validation strategy

- **Unit — `src/ui/render.rs` tests** (`badge_lines` is pure): a card with a 30-char label on a
  20-cell width yields **two** lines, the label appears **whole** across them (assert the full
  label text is a substring of the joined lines, and no `…` occurs inside `[ … ]`); a short
  chip set stays on one line; a single chip wider than `max_width` is the only elided case (trailing
  `…`). Run: `cargo test --locked --lib ui::render`.
- **Render — `src/ui/render.rs`** (`ratatui::TestBackend`, narrow terminal e.g. 60 cells): a card
  with a long label renders the label's complete text somewhere in the buffer (no `[high-prior…`
  fragment), and the card block grew to accommodate the second badge line (assert the title and the
  full chip are both present). Run: `cargo test --locked --lib ui::render`.
- **Regression — existing `render_shows_header_columns_badges_and_conflict`** must stay green
  (the badge content and order are unchanged; only wrapping/height changed).

No founder question: wrap-vs-elide is a design decision, and "no clipped text" (the ticket's own
acceptance wording) selects wrap.
