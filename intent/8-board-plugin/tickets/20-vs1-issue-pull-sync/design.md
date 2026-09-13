# Design: VS1 — GitHub issue pull sync

Epic: #8 · Ticket: #20 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

Pull thalixinc GitHub issues into board cards via a **bounded, explicit sync**: fetch issues,
map them to cards, upsert them into the board's SQLite store, and — when a previously-synced issue
changed on GitHub — surface a **visible conflict** ("issue changed; apply?") with a field diff,
never silently overwriting. GitHub is canonical for shared fields; the board's column/stage is
independent; sync never starts work and never runs as a background loop.

This is the first slice that makes the board *show real work*: the card store + the real read
client + the conflict guard. It reuses, and does not reinvent, the store (`Store`, migrations
v1–v4), the identity types (`Identity`, `RepoIdentity`), and the credential-injection pattern
(`TrustRoot::from_env()`).

---

## Card model + schema

**Identity.** Reuse `digest::Identity` (`owner`/`repo` ASCII-lowercased + `number`) as the card's
stable link identity, plus the issue **URL** (derivable, stored for one-click jump). Repo scoping
reuses `create::RepoIdentity` (`owner/repo`, lowercased). Identity is the idempotency key: one
card per issue, enforced by a unique index.

**Schema — migration v5 adds a `cards` table** (following the existing migration chain that added
`requests`, `receipts`, `create_intents`, `drafts`):

| Column | Type / notes |
|---|---|
| `owner`, `repo` | `TEXT` — lowercased, from `Identity` (no leading-zero number) |
| `number` | `INTEGER` — the issue number |
| `url` | `TEXT` — the issue's canonical URL |
| `title`, `body` | `TEXT` — GitHub canonical |
| `state` | `TEXT` — `open` \| `closed` |
| `state_reason` | `TEXT NULL` — `completed` \| `not_planned` (closed reason) |
| `labels` | `TEXT` — serialized list of label names (chips), same serialization convention as `create_intents.labels` |
| `assignee`, `milestone` | `TEXT NULL` — metadata; never a station/agent |
| `column` | `TEXT` — **board-local** stage; never derived from labels/state |
| `factory_kind` | `TEXT` — G5 discriminator, `NOT NULL` default `ordinary` (pulled issues are ordinary until explicitly promoted; inert here) |
| `revision` | `TEXT` — GitHub `updated_at` (RFC3339 UTC, verbatim), the change-detection token (G3's decision) |
| `conflict` | `TEXT` — `none` \| `apply-pending` |
| `synced_at` | `INTEGER` — unix seconds, last fetch time |

Constraints: `UNIQUE (owner, repo, number)` (idempotent upsert); `conflict` and `state` checked.
`column` defaults to the board's configured landing column for a new card (open question: which).

**Relationship to existing tables.** `cards` is the *synced view* of a GitHub issue — distinct
from `drafts` (board-authored, publish path) and `requests` (factory handoffs). A pulled card is
`factory_kind = ordinary`; the "Process with factory" promotion (G5) is a **later slice** and never
runs from sync. VS1 does not touch `requests`/`receipts`/`create_intents`/`drafts`.

---

## Sync flow (bounded, explicit)

**Explicit-only trigger.** A sync is a user-invoked action — a TUI keybinding or a manifest
`[[actions]]` entrypoint ("Sync board"). There is **no timer, no background loop, no scheduler**
(REQUIREMENTS: sync never auto-runs; `autospin=ask`). Each action runs one fetch to completion and
returns.

**Bounded.** One sync = one `list_issues` call for the board's scoped repo (paginated, capped),
optionally followed by `fetch_issue` for any issue whose list payload is stale. It is finite: no
continuous polling, no watch, no reconnect loop.

**Idempotent re-sync (upsert in place).** For each fetched issue:

1. Compute its `Identity`.
2. **No existing card** → insert a new card (default `column`, `factory_kind=ordinary`,
   `conflict=none`, `revision=fetched.updated_at`).
3. **Existing card** → compare `stored.revision` vs `fetched.updated_at`:
   - **Equal** → no change; bump `synced_at` only.
   - **Different** → the issue changed: compute the field diff and mark `conflict=apply-pending`
     (see below). The stored card is **not** overwritten.

Re-syncing the same unchanged input is a no-op; a repeated fetch can never duplicate a card (the
unique identity index).

---

## GitHub client + credentials

**Read surface (new).** The existing `GitHubClient` trait (G4) is publish-scoped
(`create_issue`, `search_issues`, `get_issue` for manual-link existence). VS1 adds a **read trait**
that the real client implements alongside it, without disturbing G4's fake/tests:

- `PullClient::list_issues(&self, repo: &RepoIdentity) -> Vec<IssueFull>` — paginated list.
- `PullClient::fetch_issue(&self, repo: &RepoIdentity, number: u64) -> Option<IssueFull>`.

`IssueFull` carries the full field set (number, title, body, state, state_reason, labels,
assignee, milestone, `updated_at`, url) — richer than G4's `Issue { number, title }`, which stays
as-is for the link existence check.

**The real client.** A single `RealGitHubClient` (HTTP, added in this slice) implements **both**
`GitHubClient` (publish) and `PullClient` (read), constructed once and injected — so publish and
pull share one construction point and one credential.

**Credentials — injected, never persisted.** Mirror the existing `TrustRoot::from_env()` pattern
with a `Credentials::from_env()` (or `from_herdr()`):

- Source order: (1) `HERDR_GITHUB_TOKEN` env; else (2) a token file under
  `HERDR_PLUGIN_CONFIG_DIR` (G1 documented config-dir hygiene). Exact mechanism is an open
  question — herdr 0.9.0 exposes no managed-secrets API (G1), so env + config file is the design.
- The token is held **in memory only**, in the client; it is **never** written to `cards`, any
  table, a card body, or a prompt (constraint). A fake `PullClient` stands in for tests and the
  demo, exactly as the fake `GitHubClient` does today.

---

## Visible-conflict detection

**Trigger.** A card is in conflict when `stored.revision != fetched.updated_at` (G3 fixed
`revision` = GitHub `updated_at`, verbatim). Revision equality is the cheap "did it change" gate;
the diff is computed only when it fires.

**Diff fields** (the shared, GitHub-canonical fields — **never** `column`, which is board-local):

- `title`, `body`, `state`, `state_reason`, `labels`, `assignee`, `milestone`.

**Surface.** The per-field old-vs-new diff is stored (a `card_conflicts` table keyed by card +
field: old value, new value, detected-at), and the card is flagged `conflict=apply-pending`. The
TUI renders the card with a "changed" badge and an **"issue changed; apply?"** prompt showing the
diff — the same visible-conflict, never-silently-win philosophy as G2's `Mismatch`/`Refusal` and
G4's manual link.

**Resolution — two human actions, mirroring G4's link/cancel:**

- **Apply** — accept GitHub's canonical values into the card's shared fields (title/body/state/
  reason/labels/assignee/milestone), update `revision` to the fetched value, clear the conflict.
  `column` and `factory_kind` are **never** touched.
- **Defer** — leave the card as-is and keep the conflict flagged; it re-surfaces on the next sync.
  (There is no "board wins" in a pull-only slice: GitHub is canonical for these fields; a "keep
  board edit + push" option arrives with board-side editing, a later slice.)

The board never auto-applies and never silently overwrites — a re-sync that finds a change *stops*
at the conflict and waits for the human.

---

## Field mapping

Per REQUIREMENTS, the GitHub → card mapping (no label↔column equivalence, no silent upstream
writes):

| GitHub | Card | Authority |
|---|---|---|
| repo + issue identity + number + URL | persistent link identity (`owner/repo#number` + url) | stable identity; number alone insufficient |
| title / body | title + body | GitHub canonical |
| open/closed + reason | `state` badge + `state_reason` | GitHub canonical; separate from `column` |
| labels | chips (serialized list) | GitHub canonical; **no auto label↔column equivalence**; CF hold/stage labels preserved verbatim |
| assignee / milestone | metadata | GitHub canonical; **never** = CF station / card agent |
| column | board placement | board-local; no silent upstream auto-execution |
| (harness/model/etc) | execution settings | board-local for ordinary cards (out of VS1 scope) |

`column` is set only by board-local drag/move, defaulting on first pull; sync never writes it and
never maps labels/state to it.

---

## Boundary

| Board owns | External system owns |
|---|---|
| The `cards` table (migration v5), the sync action, and the upsert/conflict logic. | GitHub: canonical shared fields (title/body, state+reason, labels, assignee/milestone) and its REST/GraphQL read surface. |
| The read client (`PullClient`) and its credential injection. | GitHub: the token/credentials (the board only *holds and injects* what herdr/config provides; it never stores them). |
| Conflict detection (`revision` compare) and the apply/defer resolution. | cf-queue: the **data model** — labels, `Parent epic:`/`Parent initiative:` body lines, the epic/story/task tiers. VS1 pulls issues *as they are* and does **not** interpret the 4-level hierarchy (a later slice). |
| `column` (board stage), `factory_kind` (inert here). | coordinator/planner: the factory pipeline — VS1 never hands off; sync never starts work. |

Credentials stay out of card/prompt/SQLite (constraint): the client reads them from the herdr
surface at construction and holds them in memory only.

---

## Open questions

1. **Exact credential surface.** Confirm the token source: `HERDR_GITHUB_TOKEN` env vs a config
   file under `HERDR_PLUGIN_CONFIG_DIR` vs some herdr credential verb. G1 proved config-dir
   hygiene but not a managed-secrets API; the design assumes env + config file.
2. **Scoped repo.** One board tab per factory ⇒ the sync targets that factory's repo. Confirm the
   repo identity source (manifest/`HERDR_PLUGIN_CONTEXT_JSON` vs explicit config) and whether a
   single sync covers one repo or a configured set.
3. **Default column.** Which landing column a freshly-pulled card gets ("To Do" vs a board
   "Inbox"/"Backlog"), and whether it is board-configurable.
4. **Pagination / rate limits.** The list cap and page size, and how `list_issues` signals
   "more pages" while staying bounded (no auto-loop).
5. **`state_reason` source.** REST exposes closed reason inconsistently; the real client may need
   GraphQL for `completed`/`not_planned`. Confirm which surface VS1 uses for the state badge.
6. **Conflict retention.** How long an `apply-pending` conflict persists before it is considered
   stale (reuse a single staleness constant with G3's `STALENESS_THRESHOLD`, not a new one).
