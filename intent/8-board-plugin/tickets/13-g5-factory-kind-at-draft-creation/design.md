# Design: G5 — Factory-kind at draft creation

Epic: #8 · Ticket: #13 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

When a card **draft** is created, its **factory-kind** — the factory intent (an ordinary card vs
a "Process with factory" card, and which factory it targets) — must be persisted **atomically with
the draft record**, never as a post-hoc update that a crash could drop. Every later path that
moves or mutates the draft (enqueue, move, retry, restore, recovery) must **preserve or reject**
that kind. And the kind is **inert**: sync never auto-starts work from it; the only thing that
acts on a factory card is the explicit "Process with factory" action.

This closes the bug pattern the repair targets: factory intent was being attached after the fact,
so a crash between "create draft" and "attach intent" silently dropped the intent — leaving either
a factory card that looks ordinary, or an ordinary card that later starts work it shouldn't.

---

## Factory-kind domain

**`FactoryKind`** is a card-level discriminator, exactly two variants today:

| Variant | Serialized value | Meaning |
|---|---|---|
| `Ordinary` | `ordinary` | A plain board card. Sync-display only; never hands to a factory. |
| `FactoryRequest` | `factory-request` | A card that will hand a request to coordinator/planner via the G3 bridge. |

**How it differs from the two fields it is easily confused with:**

- **G2 `CanonicalRequest.factory`** is the **target factory identity** — *which* factory. `factory_kind`
  is the **discriminator** — *whether* this is a factory card at all. They are orthogonal: a
  `FactoryRequest` card carries a `factory` identity (bound at the explicit action, see below); an
  `Ordinary` card has none. The kind is fixed at draft creation; the target factory is resolved at
  request time.
- **G4 `create_intents.factory_kind`** is the **same enum**, snapshotted into the create-intent
  when the card is published back to GitHub. Consolidate: **one** `FactoryKind` definition, one
  canonical serialization (`ordinary` / `factory-request`); G4's column stores that serialized
  value and becomes **non-null** once G5 lands (it was nullable only "until G5 lands").

**Target-factory binding (design decision):** the *which-factory* identity stays G2's `factory`
field, captured at the explicit "Process with factory" action, **not** at draft creation. Draft
creation fixes only the *kind*; a factory card may be drafted before its specific target factory
is chosen. (If the founder wants target-factory bound at draft too, it folds into the same atomic
write as an optional early-bound `factory` value — orthogonal to this ticket's correctness.)

---

## Atomic persistence at draft creation

- **Where the draft lives.** The draft is a row in the board's card store — the same SQLite store
  under `HERDR_PLUGIN_STATE_DIR` that G3/G4 use (`rusqlite`). This is the board's own card table,
  upstream of both `create_intents` (G4, publish) and the request outbox (G3, handoff).
- **`factory_kind` is `NOT NULL`, default `Ordinary`, written in the same `INSERT` transaction as
  the draft row.** There is no code path that creates a draft and then sets its kind in a second
  statement. The kind is a first-class column of the draft record from the instant the row exists.
- **The one permitted transition is also atomic.** `Ordinary → FactoryRequest` happens only via the
  explicit "Process with factory" action, and in a **single transaction**: set
  `factory_kind = FactoryRequest` **and** persist the `CanonicalRequest` (G2) with its digest. If
  the transaction rolls back, both roll back — there is never a factory card without a request, nor
  a request on an ordinary card.
- **No "kind unset" state exists** at any point. A draft, a published card, an in-flight request,
  and a reconciled request all have a definite kind.

---

## Enforce-across-paths

**Invariant:** `factory_kind` is set at creation, monotonic (`Ordinary → FactoryRequest` only, via
the explicit action), and read-only everywhere else. Each path upholds it as follows:

| Path | Rule |
|---|---|
| **Enqueue** | Only a `FactoryRequest` card can be enqueued to coordinator/planner. Enqueue reads and preserves the kind; an `Ordinary` card has **no** enqueue path. |
| **Move** | Column moves (To Do → In Progress → Done) carry the kind through unchanged. A move never demotes a `FactoryRequest` card, never clears its kind, and never starts work (sync-never-starts-work). |
| **Retry** | Retrying a refused/failed factory request reuses the same kind and the same request identity; it never reclassifies. |
| **Restore** | Kind is **part of** the restored record — restored atomically with the card, not a separate field that a partial restore could drop. A restore cannot yield a card with an absent kind. |
| **Recovery** | G3/G4 reconciliation (startup sweep, uncertain-create, handoff reconciliation) reads the kind as authoritative and never mutates it; a recovered request carries its kind through to whatever terminal outcome is resolved. |

The invariant is enforced at the store level where it matters (kind column `NOT NULL`, transitions
only through the one atomic action path), so a buggy caller cannot leave a kind-less card.

---

## Sync-never-starts-work

`factory_kind` is **pure metadata, inert by construction**. Persisting it — even as
`FactoryRequest` — never dispatches anything. Consequences:

- The only thing that acts on a `FactoryRequest` card is the **explicit "Process with factory"**
  action, which calls the G3 receiver/bridge. Nothing else reads the kind and runs.
- Sync (the board's GitHub ↔ board display loop) may render the kind as a **badge** (e.g. "factory
  request → coordinator/planner") — display only. It never enqueues, never dispatches, never
  flips the kind.
- `autospin=ask` (REQUIREMENTS) applies: even the explicit action asks rather than auto-spinning.
  A stopped factory leaves a **visible pending** card, not a background worker.

---

## G2 schema-bump impact

Per G2's explicit flag, `factory-kind` joins the canonical field set → this is a **field-set
change**, so `SCHEMA_VERSION` bumps **1 → 2**.

- **Field position.** `factory_kind` is inserted as the **new first data field** (a discriminator
  reads first). Canonical order becomes: `factory_kind`, `identity`, `revision`, `factory`,
  `actor`, `body` — after the version frame. Any fixed position works; we fix this one and bump.
- **Digest impact.** Because the version frame is hashed, **every** digest changes: a version-1
  digest can never equal a version-2 digest. The canonical bytes for the same logical input differ,
  so old and new digests are unambiguously distinguishable.
- **Verification refinement (small G2 clarification).** To verify a digest, the verifier must know
  *which* field layout to canonicalize. The `schema_version` is therefore both (a) hashed as the
  first frame (integrity — tampering with it changes the digest) and (b) recorded as metadata on
  the request record (layout selection during `verify()`). `verify()` reads the record's version,
  reconstructs that version's frames, hashes, and compares.
- **Migration.** The board is greenfield — G1 proved the tab; no shipped request data exists. So
  the migration is a pure version increment with **no data rewrite**. Any stray version-1 request
  (dev artifacts only) fails `verify()` under v2 and surfaces as a `Mismatch` → the human
  re-confirms it under the new schema. From here on, the version-on-record mechanism makes every
  future bump a clean "old digests fail verify, human re-confirms" path.
- **Honest nuance.** For the current two-variant domain, a persisted `CanonicalRequest` can only
  come from a `FactoryRequest` card, so `factory_kind` is effectively constant in every digest
  today. It is included nonetheless, per G2's flag: it makes the *kind* part of the execution-input
  identity and future-proofs the schema for additional kinds (e.g. a future `Subagent` or `Manual`
  kind) without another bump.

---

## Boundary + open questions

| Board owns | External system owns |
|---|---|
| The `FactoryKind` enum and its canonical serialization. | GitHub: canonical card fields (title/body, open/closed, labels, assignee) — the kind is board-local intent, **not** a GitHub field. |
| Atomic draft creation with `factory_kind`, and the monotonic-transition rule. | coordinator/planner (external): accepting/rejecting requests — the board never re-implements it; the kind is only ever a *label on the request the board hands over*. |
| The enforce-across-paths invariant and the inertness rule. | cf-queue: the board aligns on the field contract only; `factory_kind` is the board's own discriminator, mapped to cf-queue's factory intent *if and only if* the contract has such a field. |
| The G2 schema bump (v1→v2) and its migration mechanism. | — |
| Credentials: still held outside card/prompt/SQLite; the kind carries none. | — |

**Open questions:**

1. **Digest membership confirmation.** G2 flagged factory-kind *into* the canonical set; confirm it
   belongs there (recommended) vs. enforcing kind purely via draft-atomicity + immutability and
   leaving the digest at five fields. Current design: in the set, v2.
2. **Target-factory binding timing.** Confirm *which factory* binds at the explicit action (current
   design), not at draft creation.
3. **Demotion.** Confirm `FactoryRequest → Ordinary` is never permitted and that "cancel" is a
   terminal *outcome* on the request, not a kind change.
4. **G4 column backfill.** Pre-G5 `create_intents` rows have `NULL factory_kind`. Backfill to
   `ordinary`, or leave NULL and treat NULL as ordinary at read time? (Recommendation: backfill to
   `ordinary`, then make the column `NOT NULL`.)
5. **Future kinds.** If new kinds (e.g. `Subagent`, `Manual`) are added later, each is another
   field-set change → another schema bump; confirm the version-on-record mechanism is acceptable as
   the standing migration path.
