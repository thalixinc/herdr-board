# Intent: board-plugin
Author: manateeit (founder). Status: draft.
Issue: #8. Epic: #8.

## Problem
The factory's Herdr workspace has no in-terminal view of its work. Status lives in
GitHub, so "what is every factory doing" means leaving the terminal. We need a native
Herdr board tab that (1) pulls thalixinc GitHub issues as cards, (2) publishes
board-created cards back as GitHub issues, and (3) lets a card request the full
factory pipeline (coordinator → planner → SDLC) — not just run one agent.

## Proposed outcome
A greenfield Rust plugin (upstream `nelsonPires5/herdr-board` is a DESIGN REFERENCE
ONLY, not a fork) that opens as a native herdr TAB (`herdr plugin pane open
--placement tab`). A per-factory kanban: issues appear as cards grouped by
epic/story/task hierarchy, with filtering and drag-across-columns, readable at a
glance without opening GitHub. A card's "Process with factory" action persists a
request and hands it to coordinator/planner; sync NEVER starts work on its own.

## Affected users and systems
- **Founder** — the primary user: one board tab per factory, at-a-glance status.
- **Crew seats** — coordinator/planner/SDLC receive factory requests (not the board).
- **GitHub** — canonical for shared fields (title/body, open/closed, labels, assignee).
- **Herdr 0.9.0** — plugin/tab/pane mechanism (must be qualified BEFORE schema/sync work).

## Constraints
- GitHub is canonical; board stage is independent (Done ≠ closed issue).
- Visible conflicts: if the linked issue changed, show "issue changed; apply?" — never
  silently win. Column moves never auto-write GitHub fields.
- Explicit-only trigger: sync never starts work; `autospin=ask`.
- GitHub has NO conditional PATCH → no zero-race-overwrite promise; visible-conflict
  review is the guard, not an atomic write.
- Credentials never in card/prompt/SQLite content.
- No full issue replication (comments/attachments link to GitHub initially).
- No new public CF top-level verbs (fit beneath an existing surface).
- Cross-COF message routing out of scope.

## Open questions
- What does herdr 0.9.0 actually support for plugin/tab/pane? (critic repair G1 — the
  first gate; do not assume.)
- TUI framework (ratatui?) and store (SQLite vs simpler) — design stage, after G1.
- The five critic repairs (canonical digest, receiver/auth/receipt, uncertain-create,
  factory-kind, herdr-compat) are the mandatory planning gate — sliced as the first
  tickets before any vertical slice.
