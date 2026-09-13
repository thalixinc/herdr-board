# Design: VS2 — card→issue push

Epic: #8 · Ticket: #23 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

Publish board cards back to GitHub along two paths: (1) a **board-created card becomes a GitHub
issue**, already guarded by G4's `create_intents` outbox (marker, block-auto-replay, manual
link/cancel); and (2) a **board edit to a linked card becomes a pending write** back to GitHub,
checked against the issue's current revision before writing so a concurrent GitHub change surfaces
as a **visible conflict** — GitHub is canonical, the board never silently overwrites, and there is
no conditional PATCH (so the visible-conflict review, not an atomic write, is the guard).

This reuses G4's create path verbatim, and adds only what G4 deliberately left out: the
**update** seam (`update_issue`) and the **pending-write** model on top of VS1's card store.

---

## Publish client

**One client, one token.** VS1's `RealGitHubClient` (blocking `ureq`, `Credentials::from_env()`)
already implements the read `PullClient`. VS2 makes it the **single** publish+read client: it also
implements the G4 publish trait `GitHubClient` (`create_issue`, `search_issues`, `get_issue`) and
gains the new `update_issue`. One construction point — `RealGitHubClient::new(Credentials)` — and
the one token held in memory only, never persisted (constraint).

**The new write method.** Extend the publish seam with an update:

- `fn update_issue(&self, repo: &RepoIdentity, number: u64, patch: &IssuePatch) -> UpdateResult`

`IssuePatch` carries only the fields being changed (`title`/`body`/`labels`/`assignee`/`milestone`
as `Option`s, where a present `Some(None)` clears assignee/milestone; `None` = leave unchanged).
`UpdateResult` mirrors G4's three-way `CreateResult`:

| Variant | Meaning |
|---|---|
| `Updated` | PATCH applied; GitHub returned the new state (and new `updated_at`). |
| `Failed(String)` | Definite error — nothing changed. |
| `Uncertain(String)` | Response lost — may or may not have landed. |

The three-way shape is required because GitHub PATCH is non-idempotent *with respect to concurrent
writers*: re-sending the same patch is safe only if nobody else changed the issue in between —
which is exactly what the revision guard below decides. The existing `GitHubClient` fake gains
`update_issue` mechanically; `PullClient` and the VS1 read path are untouched.

---

## Draft→issue flow (reuse G4 outbox)

This path is **already built in G4**; VS2 wires the draft in and the card out, adding no new
outbox:

1. **Build the intent.** A "Publish draft" action constructs a `CreateIntent` from the draft
   (`drafts` table): `repo` = the board's scoped `RepoIdentity`, `title`, `body`, `labels`
   (JSON array), `assignee`, and `factory_kind` sourced from the draft's G5 column.
2. **Reuse `create::issue`.** `create::issue(store, client, &intent)` runs the existing write-ahead:
   insert `pending` → commit `issued` → `client.create_issue` → finalize `created`/`failed`, or
   leave `issued` on `Uncertain`.
3. **Uncertain → block auto-replay → manual link.** An `issued` intent is never auto-re-issued.
   Reuse `create::candidates` / `create::link` / `create::cancel` verbatim: marker-first, then
   title-fallback candidate search; the human links the real issue (adopt, never re-create) or
   cancels (never deletes on GitHub).
4. **On `created`.** Link the new issue number to a `cards` row (identity = `RepoIdentity` +
   number) and set the draft's `published_at`. From that point the board-created card is a linked
   card and eligible for the edit path below.

No change to `CreateIntent`/`CreateOutcome`/`Marker`/the `create_intents` schema — VS2 consumes
them as-is.

---

## Board-edit pending write + visible conflict

This is the new half of VS2: pushing an edit to a **linked** card.

**Saved base.** The card's synced state is the base: `Card.fields` (the seven `CanonicalFields`)
plus `Card.revision` (GitHub `updated_at`, verbatim). Every board edit is recorded **against** this
base — the edit is a pending write, not an immediate GitHub mutation.

**Pending-write record (new table, migration after `cards`).** One row per card with a pending
edit — enforced at most one per card (a new edit merges into the existing pending write, so
repeated edits converge, mirroring G3's active-attempt convergence):

| Field | Meaning |
|---|---|
| `write_id` | UUID |
| `owner`, `repo`, `number` | linked issue identity |
| `base_revision` | the card's `revision` the edit was made against |
| `title`, `body`, `labels`, `assignee`, `milestone` | the **new** values (`NULL` = unchanged) |
| `outcome` | `pending` \| `written` \| `failed` \| `uncertain` \| `discarded` |
| `created_at`, `finalized_at` | timestamps |

Recording an edit is **inert**: it writes the pending row only. Nothing touches GitHub until an
explicit **"Push changes"** action (explicit-only trigger; sync never auto-writes).

**Push, with the revision gate.** The explicit push:

1. `fetch_issue(repo, number)` → current `updated_at`.
2. **Compare `current` vs `base_revision`:**
   - **Equal** → the base is still current → `client.update_issue(…)` with the pending values →
     on `Updated`, write the new values into `Card.fields`, set `revision` to the new `updated_at`
     (re-fetch or use the PATCH response), and finalize `written`.
   - **Different** → GitHub changed since the base → **visible conflict**: surface "issue changed;
     apply?" with the field diff between the board's pending values and GitHub's current values
     (reusing `sync::conflict::field_diffs`' field set and ordering, but in the push direction).

**Conflict resolution — two human actions:**

- **Apply (push anyway)** → PATCH the board's values over GitHub's (an explicit, human-confirmed
  board-wins; this is the race window REQUIREMENTS accepts, because there is no conditional PATCH).
  Update base revision + fields on success.
- **Discard** → drop the pending write and re-sync the card to GitHub's current values (back into
  VS1's pull `apply`). The board never auto-pushes and never silently overwrites.

**Relationship to VS1's pull conflict.** VS1's `Conflict::ApplyPending` (GitHub drifted, await pull
apply) and this push-side conflict share the same root signal (revision drift) but are distinct
states. A card already in `ApplyPending` has a stale base, so its next push will surface the
push-side conflict — the two mechanisms agree on the same `revision` token.

---

## Field mapping / boundary

**Pushed (GitHub-canonical, board-editable) vs board-local:**

| Field | Push? | Notes |
|---|---|---|
| `title`, `body` | **yes** | GitHub canonical; board edits = pending writes |
| `labels` | **yes** | pushed as the edited set; **no label↔column equivalence**, and CF hold/stage labels are preserved (not silently stripped) |
| `assignee`, `milestone` | **yes** | GitHub canonical metadata; never = CF station / card agent |
| `state` / `state_reason` | **no (VS2)** | open/closed is GitHub canonical but pushed only via an explicit close/reopen action — column moves **never** close an issue |
| `column` | **never** | board-local; column moves never auto-write GitHub |
| `factory_kind` | **never** | board-local intent (G5); inert, never pushed |

**Boundary:**

| Board owns | External system owns |
|---|---|
| The pending-write table, the base-revision gate, and the apply/discard resolution. | GitHub: canonical shared fields + the PATCH surface; its `updated_at` is the revision token. |
| The publish client (`GitHubClient` + `update_issue`) and its credential injection. | GitHub: the token — the board only holds/injects it, never stores it. |
| Draft→intent wiring (reuses G4's outbox). | cf-queue: the data model (labels, `Parent:` lines, tiers) — the board pushes labels verbatim, never rewrites the data model. |
| `column` / `factory_kind` / the visible-conflict policy. | coordinator/planner: out of scope — publishing is board→GitHub, not the G3 handoff. |

Credentials never in card/prompt/SQLite: the client reads the token at construction, in memory only.

---

## Idempotency / retry

The two write paths have **different** idempotency, and this is the crux:

- **Create (board-created card → new issue).** Non-idempotent: each create makes a *new* issue.
  Idempotency is G4's — the client idempotency **marker** + **block-auto-replay** (an `issued`
  intent is never re-issued). A `failed` create re-attempts as a **fresh intent (new marker)**,
  never a replay of the old row. VS2 changes nothing here.
- **Update (board edit → linked issue).** *Value-idempotent*: a PATCH of specific fields to
  specific values converges to the same final state no matter how many times it lands. There is
  **no create-marker** (the issue number is already known) and **no block-auto-replay**. A
  re-delivered update is simply idempotent; a `failed`/`uncertain` update is re-attempted with the
  **same values**, but **only after re-running the revision gate** — if the issue changed since
  `base_revision`, it surfaces the conflict instead of silently overwriting.

So: create dedup = marker; update dedup = value-idempotency + revision guard. Both refuse to
silently overwrite; both stay human-gated.

---

## Open questions

1. **`state` push.** Confirm open/close is out of VS2's board-edit path (explicit close/reopen as
   a distinct action — column moves never close). If closing is wanted now, it becomes a third
   pending-write field with the same revision gate.
2. **Label preservation.** Should the board *refuse* to remove CF hold/stage labels in a push
   (guard), or only warn? "Preserve" is stated but the enforcement strength is open.
3. **Pending-write merge.** When a second edit arrives before a push, merge field-by-field (last
   wins) vs replace the whole pending row — confirm merge semantics and whether `base_revision`
   stays pinned to the original base or advances.
4. **Uncertain-update retry.** Is re-attempting an `uncertain` update automatic on the next push,
   or always explicit? (Recommendation: always explicit, matching sync-never-auto-writes.)
5. **Post-PATCH revision.** Trust the PATCH response's `updated_at` vs re-`fetch_issue` to set the
   new base revision — a determinism/race detail, not a correctness one.
6. **Convergence with VS1 conflict.** Whether a card in `ApplyPending` should first force a pull
   apply before any push is allowed (recommended), so edits never build on an already-stale base.
