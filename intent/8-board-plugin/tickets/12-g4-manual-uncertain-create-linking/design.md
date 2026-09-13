# Design: G4 — Manual uncertain-create linking

Epic: #8 · Ticket: #12 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

When the board publishes a card back to GitHub as a new issue, the create is a **non-idempotent
external mutation**: the request may have succeeded while its response was lost (timeout, network
drop, crash), so the board does not know the new issue's number. A naive retry would create a
**duplicate** issue. The board must therefore (a) durably record "create issued, outcome unknown",
(b) **never silently auto-replay** such a create, and (c) surface the pending card so a **human**
links it to the real issue or cancels — the same visible-conflict, explicit-only philosophy as the
rest of the board.

This reuses **G3's outbox/reconciliation pattern** (persist intent → issue call → finalize
outcome; non-terminal vs terminal states; startup sweep; explicit resolve) rather than inventing a
parallel mechanism.

---

## Create path + where uncertainty arises

The board→GitHub publish path:

1. **User drafts a card** on the board (title, body, labels, assignee, and — per G5 — the
   factory-kind intent).
2. **Board persists the create intent** — a durable outbox row recording *exactly what* issue to
   create, and a client-generated idempotency marker — **before** any GitHub call.
3. **Board issues the GitHub create** (the issue-create API).
4. **Outcome is one of three:**
   - **`created`** — GitHub returns the new issue number → board records it and links card↔issue.
   - **`failed`** — GitHub returns a definite error → no issue was created; the intent is terminal
     and safe to re-attempt as a *fresh* intent.
   - **lost response** — timeout / network drop / process crash → the issue **may or may not**
     exist. The board has no number. **This is the uncertainty.**

The uncertainty is irreducible at the transport layer: the board cannot, by retrying, distinguish
"not created" from "created but ack lost." Every guard in this ticket exists because of that.

---

## Uncertain-create durable state

Reuse G3's outbox model, with a **create-specific outcome set**. One row per create intent, in the
same SQLite store (`rusqlite`):

- **intent id** — UUID v4.
- **canonical create payload** — title, body, labels, assignee, and (G5) factory-kind; stored
  verbatim so the human can see *exactly* what would have been / may have been created.
- **idempotency marker** — a client-generated opaque token (UUID), unique per intent (see §6).
- **repo + identity** — the target `owner/repo` the issue was to be created in.
- **timestamp** — UTC, intent creation / issue time / finalize time.
- **outcome state** (non-terminal = still in flight; terminal = decided):

| State | Terminal? | Meaning |
|---|---|---|
| `pending` | no | intent persisted, create not yet sent |
| `issued` | no | create sent to GitHub; **outcome unknown** |
| `created` | yes | GitHub returned the number; card linked |
| `failed` | yes | GitHub returned a definite error (nothing created) |
| `cancelled` | yes | human cancelled |

**Write-ahead order (G3 pattern):** insert `pending` → flip to `issued` in a **committed**
transaction **before** the GitHub call → finalize to `created`/`failed` in a **later** transaction
after the response. A crash between "issued" and "finalize" leaves an `issued` row — a durable,
reconcilable "outcome unknown" rather than a silently lost or silently retried create.

---

## Block-auto-replay rule

**A create intent in `issued` state is never automatically re-issued. Ever.** No background retry,
no startup-sweep replay, no timer. This is the core of the repair.

**Idempotency for creates.** GitHub's issue-create API provides **no client-supplied idempotency
key**, so the board cannot rely on GitHub to dedup a retry. The board's idempotency is therefore
**entirely client-side**:

- The board persists the create intent (with its unique marker) and **refuses to re-issue** any
  intent whose state is `issued` or `created`. A duplicate is impossible *because the board will
  not send a second create for the same intent*.
- Re-issue is permitted **only** from a terminal `failed` state (definitely nothing was created),
  and even then as a **fresh intent** (new intent id, new marker) — never a replay of the old one.
- The marker is not an API idempotency key; it is a **reconciliation key** (below): the board's
  way to *find out* whether the uncertain create actually landed.

**Consequence:** a lost response parks the card in an `issued` state with a visible "awaiting
link" badge. The founder sees it; nothing runs silently.

---

## Manual linking

The entrypoint surfaces every `issued` create-intent as a **pending card** with a visible prompt,
offering exactly two human actions:

1. **Link** — the human identifies the real issue and records its number, either by picking from
   **candidate matches** the board fetched or by pasting an issue number/URL. The board then sets
   `created` with that number and links card↔issue. **The board never re-creates** — it adopts the
   existing issue. A wrong auto-link would bind the card to the wrong issue, which is as bad as a
   duplicate, so linking is strictly human-confirmed.
2. **Cancel** — the human cancels; intent → `cancelled`, and the card stays local (or is
   discarded by the human separately). **Cancel never deletes anything on GitHub**: the board
   cannot know whether the issue exists, and deleting a possibly-created issue would be a separate,
   explicit, human-confirmed action (out of scope here). Cancel only means "stop tracking this
   create; I'll handle it manually."

**Candidate matching.** The board pre-populates candidates by searching GitHub (see §7), ordered by
confidence: exact idempotency-marker match first, then title/body/identity matches. Candidates are
shown with title, number, repo, and created-at so the human can pick confidently. The board
**never auto-links** — candidates are suggestions, not decisions.

---

## GitHub reconciliation

How the board resolves "did my create actually succeed" — all read-only, all human-gated:

1. **Primary: search by the client-generated idempotency marker.** The marker is a unique token
   embedded in the created issue (see §7). The board queries GitHub search for that exact marker in
   the target repo. Found ⇒ the create **did** succeed ⇒ that issue is the match; the human links
   it. This is unambiguous because the marker is unique per intent.
2. **Secondary: title + timestamp/author.** If the marker is not found (indexing lag, marker
   stripped, search semantics), fall back to exact-title search scoped to the repo and a time
   window around the `issued` timestamp. These are **heuristic candidates only**, never
   auto-linked.
3. **A search miss never means "safe to re-issue."** GitHub search is eventually consistent and the
   marker may not be indexed/preserved, so a miss only means "still unresolved — keep surfacing for
   manual resolution." The board **never** infers "definitely not created" from a failed search; the
   only terminal "definitely not created" state is a **definite error** returned by the create call
   itself (`failed`), not a later search miss.

Reconciliation is explicit and bounded (G3 pattern): the **startup sweep** surfaces `issued`
intents as pending cards; the board may auto-run the read-only marker-search to *pre-populate
candidates*, but it never auto-links and never auto-re-issues. Work is never started by
reconciliation.

---

## Boundary + credentials

| Board owns | External system owns |
|---|---|
| The create-intent outbox + outcome states (reusing G3). | GitHub: the actual issue creation, the canonical issue fields (title/body, open/closed, labels, assignee — GitHub is canonical per REQUIREMENTS), and its search API semantics. |
| The block-auto-replay rule and client-side idempotency. | GitHub: nothing idempotency-wise — it has no client idempotency key; the board does not pretend otherwise. |
| The manual-link + cancel entrypoint, and the candidate search. | cf-queue: **out of scope here** — this is board→GitHub publishing, not the board→coordinator handoff (G3). cf-queue's only relevance is that the create payload carries the G5 factory-kind, whose own intent-atomicity is G5's ticket. |
| The client-generated idempotency marker (opaque, unique, harmless). | — |

**Credentials handling (constraint):** the GitHub token used for create/search lives **outside**
card/prompt/SQLite — in the board's herdr-provided credential surface. It is never written into an
intent row, a card body, or the marker. The marker itself is **not a credential** — it is a public
opaque UUID with no authority — so embedding it in an issue body leaks nothing.

**Marker placement (design choice):** the marker is embedded in the issue **body** as an HTML
comment (e.g. `<!-- herdr-board:intent:<uuid> -->`), invisible in rendered view but present in the
raw body for search. This keeps it out of labels/title (no UI clutter) and scoped to exactly one
intent. If GitHub strips or fails to index HTML comments, the title+timestamp fallback (§7.2)
carries resolution; this is flagged as an open question, not a blocker.

---

## Open questions

1. **Marker searchability.** Does GitHub's issue search index HTML comments in the body, and are
   HTML comments preserved verbatim by the create API? If not, move the marker to a different
   place (e.g. a literal footer line) — the reconciliation design is unchanged, only the marker's
   location.
2. **Cancel semantics.** Should `cancel` also offer to *delete* a known-created issue (a
   separate, explicit, human-confirmed destructive action), or is "stop tracking; handle manually"
   the only cancel meaning? Current design: cancel never deletes.
3. **Candidate-match confidence.** How many candidates to surface, and the exact
   title-normalization for the secondary search (case/whitespace) — a UI/detail decision, not a
   correctness one.
4. **`issued`-staleness.** Reuse G3's staleness threshold for surfacing `issued` intents as
   "awaiting link" in the startup sweep; confirm it shares one value with G3 rather than diverging.
5. **Fresh-retry after `failed`.** Confirm that a `failed` re-attempt is always a *new* intent
   (new marker) rather than a mutation of the old row, so history stays intact.
