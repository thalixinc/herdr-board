# herdr-board — Requirements (authoritative)

Greenfield rebuild. `nelsonPires5/herdr-board` is a DESIGN REFERENCE ONLY, not a fork.

## Problem

The factory's Herdr workspace needs a board tab that (1) pulls thalixinc GitHub issues as cards,
(2) publishes board-created cards back as GitHub issues, and (3) lets a card request the full
factory pipeline (coordinator → planner → SDLC), not just run one agent.

## Authority model (founder-ruled, Q1 = A)

- **GitHub is canonical** for shared fields (title/body, open/closed, labels, assignee).
- **Board stage is independent** — moving a card to Done never closes the issue.
- **Visible conflicts**: if the linked issue changed, show "this card's issue changed; apply?" with a
  diff. Never silently win. Column moves never auto-write GitHub fields.

## Trigger (founder-ruled, Q2 = A)

- **Explicit-only**: sync NEVER starts work; no auto-triggers; `autospin=ask` applies.
- A card's "Process with factory" is an explicit action that persists a request (repo/issue identity,
  target factory, actor, issue revision) and hands to coordinator/planner, which accepts/rejects via
  current queue + SDLC state.
- Repeated clicks converge on one active request. A stopped factory → visible pending, not a
  standalone card agent.

## Field mapping

| GitHub | Board | Authority |
|---|---|---|
| repo + issue identity + number + URL | persistent link | stable identity; number alone insufficient |
| title / body | title + body | GitHub canonical; board edits = pending writes on a saved base |
| open/closed + reason | state badge | separate from board stage |
| labels | chips | no auto label↔column equivalence; preserve CF hold/stage labels |
| assignee/milestone | metadata | never = CF station / card agent |
| column | board placement | board-local; no silent upstream auto-execution |
| harness/model/etc | execution settings | board-local for ordinary cards; factory cards use team routing |

## Board UX (founder's primary use case — the "Jira/Trello" experience)

1. **One board tab per factory** — every factory's herdr workspace has its OWN board tab, scoped to
   that factory's GitHub project/repo. No global board; each crew sees its own work.
2. **4-LEVEL nested hierarchy (GitHub Projects full model)** — Initiative → Epic → Story/Task →
   Sub-issue, tasks/stories indented under their parent, one group row per parent. The board must
   render this nested roadmap/table view. Mapping to the factory's encoding:
   - **Initiative** — NEW top tier (does NOT exist in cf today). Encodes as a label (`initiative`)
     + `Parent initiative:` body line on its child epics (mirroring the `Parent epic:` convention).
   - **Epic** = issue with `epic` label (+ `sdlc:<stage>` label).
   - **Story / Task** = issue with `task` (or `story`/`bug`) label + `Parent epic: <N>` body line.
   - **Sub-issue** = GitHub native sub-issue (the `sub-issue` link `cf sdlc ticket` already writes).
   IMPORTANT: today cf only encodes **2 levels (epic → task)**. The Initiative and Story tiers are
   DATA-MODEL additions that must be added to `cf-queue` / `cf board` (labels + `Parent …:` body
   lines), NOT invented by the board alone. These are separate planning decisions, listed as open.
3. **Filtering** — by label, assignee, epic, milestone, state, repository. This is first-class, not
   an afterthought.
4. **Columns = workflow stages** — drag cards across columns (e.g. To Do → In Progress → Done),
   reflecting the SDLC/board stages; visual state is readable at a glance WITHOUT opening GitHub.
5. **Visual at-a-glance** — the founder can see what every factory is doing from the board tab, no
   GitHub visit required.

This UX layer is a HARD requirement, not a nice-to-have: the point of the board is to REPLACE
"open GitHub to see status" with a native in-terminal kanban per factory.

## Factory boundary (founder rule — MUST NOT violate)

herdr-board owns ONLY the board: the tab, the nested view, filtering, sync DISPLAY, and the
card→factory trigger BRIDGE (handing a request to the existing coordinator/planner). Any change to
another product belongs in THAT product's own factory — herdr-board NEVER re-implements it:

- cf / cf-queue / cf board (data model, labels, `Parent ...:` lines, initiative tier) → codefactory factory (#426).
- herdr-axi (machine/attach verbs, transport) → herdr-axi factory.
- The board CONSUMES what those factories produce; if the board needs something that doesn't exist
  yet, file it in the owning factory and treat it as a dependency, not build it here.

## Open data-model decisions (block the full 4-level view — decide before board build)

1. **Initiative tier** — add `initiative` label + `Parent initiative:` body line to cf-queue / cf
   board, so epics can be grouped under initiatives. (Founder wants this — GitHub Projects' top level.)
2. **Story tier** — add `story` label + `Parent story:` body line for the epic → story → task
   middle tier, OR treat story ≡ task (2-level under the epic). Founder to confirm 3 vs 4 levels of
   task granularity.
3. **Sub-issue** — cf already writes the sub-issue link; confirm the board reads it (Group by
   sub-issue) rather than only `Parent epic:`.

Until these are decided, the board can render the hierarchy that *exists* (epic → task, 2 levels),
and the nested roadmap view simply gains levels as the data model grows.

## Non-goals

- No full issue replication (comments/attachments link to GitHub initially).
- No new public CF top-level verbs (fit any bridge beneath an existing surface).
- Cross-COF message routing is out of scope (separate effort).

## Constraints

- Credentials never in card/prompt/SQLite content.
- Reuse the native herdr plugin/tab mechanism (`herdr plugin pane open --placement tab`).
- Correctness boundary: GitHub has NO conditional PATCH → there is NO zero-race-overwrite promise.
  Visible-conflict review is the guard, not an atomic write.

## Mandatory planning gate (five critic repairs, in order)

1. **Canonical work digest** — versioned execution-input digest, compared at acceptance and dispatch.
2. **Receiver/auth/receipt qualification** — real private receiver + reconciliation entrypoints,
   trusted actor provenance, durable receipts, dedup, atomic active-attempt uniqueness.
3. **Manual uncertain-create linking** — ambiguous GitHub create → block auto-replay, manual linking.
4. **Factory-kind at draft creation** — persist factory intent atomically before publication; enforce
   across every enqueue/move/retry/restore/recovery path. Sync never starts work.
5. **Herdr compatibility FIRST** — qualify/pin the target herdr + prove the native board tab before
   schema/sync work.

## Source

Ported from the design decision record (codefactory brainstorm `herdr-board-github-sync/`):
clarification.md (requirements + authority model), recommendation.md (architecture), decisions.md
(founder amendment: greenfield rebuild, not fork; five repairs).
