# Plan: ticket #31 — Fix label chip truncation (wrap whole chips, no clipped text)
Epic: #8. Date: 2026-09-13. Status: ready. Base: `vs/board-ui-kanban` @ 88766b5.

## Files that change

`src/ui/render.rs` **only** — the whole fix is a pure layout helper plus the two call
sites, no other file, no model/store/data-layer change.

- Add `fn badge_lines(card: &Card, max_width: usize) -> Vec<String>` — iterate the
  badges in today's order (`state_badge`, then `[label]`, `@assignee`, `m:milestone`,
  `⚙`) and greedily pack **whole chips** onto lines of at most `max_width` cells,
  breaking **between** chips. A single chip wider than `max_width` is the only elided
  case (trailing `…`); no chip text is ever split. Pure and side-effect-free.
- `render_card` — replace `let badge_text = truncate(&badges.join(" "),
  width.saturating_sub(2))` + the single badge `Line` with one `Line` per
  `badge_lines` row (whole chips). Accept the pre-computed lines (or recompute
  deterministically from the same `card`/`width`).
- `render_column` — compute `badge_lines` once (width is available from `inner.width`
  before the slot is finalised) and reserve `height = 1 /* title */ +
  badge_lines.len() + diff_rows /* conflict rows */ + 1 /* padding */`; pass the
  lines into `render_card`. Today it hardcodes `let height = 4 + diff_rows;`.
- Keep `fn truncate` for the title line (unchanged — a title is one atomic line).

Rejected (design, restated so the dev does not reach for them): per-chip truncation /
drop-trailing-chips with `+N` (hides labels — violates "legible"); shrinking chips to
fit (hides text inside a chip); fixed-height overflow into a scroll region (more
surface, no legibility gain). Wrap at chip boundaries is the boring correct answer.

No founder question: "no clipped text" (the ticket's own acceptance wording) selects
wrap over elide.

## Order of work

1. [helper] `badge_lines(card, max_width) -> Vec<String>` (whole-chip greedy wrap,
   single-chip elide only) + its unit tests.
2. [render_card] Draw one `Line` per badge line instead of the single truncated
   `badge_text`.
3. [render_column] Compute the lines once, drive `height` from `.len()`, pass the
   lines through (or recompute — same pure function, same input).
4. [test] Render tests (narrow `TestBackend`) + keep the existing regression green.

## Validation Strategy

- Unit — `cargo test --locked --lib ui::render` (`badge_lines` is pure): a card with a
  30-char label on a 20-cell width yields **two** lines, the full label appears whole
  (assert the full label text is a substring of the joined lines, and no `…` occurs
  inside `[ … ]`); a short chip set stays on one line; a single chip wider than
  `max_width` is the only elided case (trailing `…`).
- Render — `cargo test --locked --lib ui::render` (`ratatui::TestBackend`, ~60-cell
  terminal): a card with a long label renders the label's complete text somewhere in
  the buffer (no `[high-prior…` fragment), and the card block grew to fit the second
  badge line (assert title + full chip both present).
- Regression — existing `render_shows_header_columns_badges_and_conflict`
  (`src/ui/render.rs`) must stay green: badge content and order unchanged, only
  wrapping/height changed.

## Proof

```sh
cargo test --locked --lib ui::render    # badge_lines unit + TestBackend wrap + regression green
```
Expected: exit 0; `badge_lines` returns whole chips only; the narrow-terminal buffer
contains the full label text (never a mid-chip `…`); the existing
`render_shows_header_columns_badges_and_conflict` still passes. (Full-suite +
clippy/fmt run once at integration, per crew rule 5.)
Evidence lands in `intent/8-board-plugin/tickets/31-label-chip-truncation/evidence/`.

## Risks

- **Height/wrap agreement (step 3).** `render_column` reserves height from
  `badge_lines.len()` and `render_card` draws the same lines — if they disagree (one
  recomputes with a different width), the card overflows or under-fills. Mitigation:
  compute once, pass the lines in, and add a render test asserting the card block
  exactly fits (no leftover clip, no blank overflow row).
- **Truncate reuse.** `truncate` stays for the title (correct — one atomic line). A
  dev that also "fixes" the title path risks re-introducing mid-word title clipping
  that is out of scope. Mitigation: title path untouched; regression test asserts the
  title still truncates as before.
- **Single-chip-wide elision.** The only elide case is a chip wider than the card
  alone; that trailing `…` is the pathological case, not the normal one — do not
  generalize per-chip truncation to "normal" labels.
