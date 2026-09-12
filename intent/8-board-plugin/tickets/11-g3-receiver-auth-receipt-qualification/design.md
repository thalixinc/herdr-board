# Design: G3 — Receiver/auth/receipt qualification

Epic: #8 · Ticket: #11 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

The board needs **one private, enforced code path** that (1) accepts a "Process with factory"
handoff, (2) proves *who* asked rather than trusting a payload claim, (3) writes a **durable
receipt** of what was handed off and what happened, (4) never double-accepts a replayed handoff,
(5) guarantees **at most one active attempt per request** at a time, and (6) can recover any
uncertain state after a crash — all inside the board. coordinator/planner stays external; the
board never re-implements it; sync never starts work.

---

## The receiver

**What it is.** The receiver is a **board-internal module** (a sealed type + one entry function),
backed by a **durable outbox** in the board's SQLite store. It is the single seam every handoff
flows through; there is no other path from "Process with factory" to coordinator/planner. Its
entry is one function of the G2 contract's shape: given a persisted `CanonicalRequest`, it
verifies the digest, proves the actor, dedups, acquires the active-attempt slot, records a
receipt, performs the one external handoff, and finalizes the receipt — returning a `Receipt` or
a G2 `Refusal`.

**Why "private".** Three concrete meanings, all required:

1. **Not a public verb.** It is *not* a new CF top-level verb and *not* a network or
   addressable surface. It is an in-process seam the board's TUI action calls directly, fitted
   beneath the existing coordinator/planner acceptance surface (REQUIREMENTS non-goal: "No new
   public CF top-level verbs").
2. **Single reachable entrypoint.** The only caller is the explicit "Process with factory"
   action. No background loop, no auto-trigger, no timer can reach it — sync never starts work.
3. **Board-local state.** Its durable state lives in the board's own SQLite (under
   `HERDR_PLUGIN_STATE_DIR`), never in a listening service or shared queue. "Private" = the
   receiver and its outbox are board-owned and board-local, end to end.

**Why an outbox.** Handoff is the one place where a board-internal action crosses into an
external system (coordinator/planner) whose result may be lost or late. The outbox makes that
crossing recoverable: the board commits its *intent* and *outcome* before and after the external
call, so a crash at any point leaves a durable, reconcilable row rather than a silently lost
handoff (see Reconciliation).

---

## Actor provenance

**The rule: the actor is proven, never claimed.** The board never reads an actor value from the
request body, card content, or any persisted field that untrusted content could populate. Those
are data; the actor is *context*.

**Trust root.** The board runs as a herdr pane; herdr launches it and injects process context
(the `HERDR_*` surface G1 proved: plugin id, pane/tab/workspace ids). That injected context is
the trust root: it is set by the parent herdr process, not by any card/prompt/SQLite content,
and cannot be forged by a payload. The actor identity is therefore **derived from the herdr
workspace/operator identity at action time**, not read from the request.

**Provenance binding.** At "Process with factory", the board:

1. Captures the trust-root identity (herdr workspace/operator id) from its own process
   environment — never from the body.
2. Writes it into the `CanonicalRequest.actor` field, which G2 hashes into the digest.
3. Records, on the receipt, both the actor value and its **source channel**
   (`actor_source = herdr-workspace-identity`), so reconciliation can distinguish a
   board-proven actor from anything else.

**Enforcement.** A handoff whose `actor` field is absent, or does not match the board's current
trust-root identity, is refused. Because `actor` is inside the G2 digest, an actor swapped after
persist also fails `verify()` as a `Mismatch` — provenance (trust root) plus integrity (digest)
together make the actor unforgeable end to end.

---

## Receipts

A receipt is the durable record that a specific handoff was received, from whom, and how it
resolved. It records:

- **receipt id** — UUID v4, unique per receipt.
- **request id** — the board's request-record id this receipt is about.
- **digest** — the full G2 digest, plus the short **digest id** (G2's display prefix) for humans.
- **actor** — the proven actor, plus its `actor_source` channel.
- **identity + revision** — the `owner/repo#number` and the revision token, for human
  reconciliation display.
- **target factory** — which coordinator/planner pipeline was targeted.
- **timestamp** — wall-clock UTC of receipt creation/finalization.
- **outcome** — the handoff's state (see states below).
- **external response** — the coordinator/planner ack/refusal, recorded verbatim but treated as
  **untrusted external data** (it never overrides the board's own committed state).

**Outcome states** (non-terminal = "active attempt"; terminal = "decided"):

| State | Terminal? | Meaning |
|---|---|---|
| `pending` | no | accepted by the receiver; handoff not yet attempted |
| `handed-off` | no | dispatched to coordinator/planner; outcome not yet known |
| `accepted` | yes | coordinator/planner accepted |
| `refused` | yes | mismatch (G2) or coordinator/planner refused |
| `cancelled` | yes | human cancelled |

**Survival across restart.** The receipt is committed to SQLite in the same transaction as the
request-state transition. The write-ahead order is: (a) insert a `pending` receipt and acquire
the active-attempt slot; (b) flip to `handed-off` in its own committed transaction **before** the
external call; (c) finalize to `accepted`/`refused` in a **third** transaction after the external
result. A crash between (b) and (c) leaves `handed-off` — a durable, reconcilable "outcome
unknown" rather than a lost handoff. Receipts never contain credentials (constraint).

---

## Dedup

**A re-delivered or replayed handoff must never double-accept.**

**Idempotency key = the full G2 digest.** The digest uniquely and deterministically identifies
the exact execution input (identity + revision + factory + actor + body). Therefore:

- A re-delivered handoff of the *same* input computes the *same* digest → recognized as a
  duplicate of an already-receipted handoff → the receiver returns the **existing receipt**
  unchanged and does **not** re-dispatch or re-accept.
- A *different* input (revision bumped, body edited, actor or factory changed) computes a
  *different* digest → correctly treated as a **new** request, not a duplicate. (This is exactly
  the "issue changed → visible conflict" path, not dedup.)

Each handoff also carries a **`handoff_id`** (UUID) so the receiver can distinguish "the same
attempt re-delivered" (idempotent no-op) from "a new attempt of the same request" (routed to the
active-attempt rule below). Dedup is enforced by a uniqueness constraint on the digest inside
the same SQLite transaction as receipt insertion, so it is race-safe and restart-safe.

---

## Active-attempt uniqueness

**Exactly one active (non-terminal) attempt per request at a time.**

Enforced by a **store-level uniqueness constraint, not in-memory state**: at most one receipt
row per `request_id` whose `outcome` is non-terminal (`pending` or `handed-off`). Attempting to
insert a second active attempt for the same request violates that constraint, the transaction
aborts, and the receiver refuses with "an active attempt already exists" — returning the existing
receipt instead of dispatching a second handoff.

- **Concurrency.** SQLite (WAL) serializes writers; the constraint makes "insert active attempt"
  race-safe — two simultaneous clicks for one request converge on the single winner, matching
  REQUIREMENTS "repeated clicks converge on one active request."
- **Restart.** The active-attempt marker is a committed row with a non-terminal outcome, so it
  survives restart: an in-flight request stays in-flight (`handed-off`) and the next handoff is
  resumed/deduped, never duplicated.
- **Convergence rule (decides G2 open q5).** If a request already has a non-terminal receipt,
  a re-click returns that receipt (no new attempt). If its receipts are all terminal, a re-click
  is a **fresh request**: re-persist under the *current* input → new digest → new attempt. A
  terminal receipt never blocks a genuinely new attempt, because the constraint keys on
  non-terminal outcomes only.

---

## Reconciliation

Reconciliation is the set of board-internal entrypoints that convert "uncertain" back to
"decided" without ever starting work. The outbox guarantees every handoff leaves a durable
outcome; reconciliation is what *reads and resolves* the non-terminal/ambiguous rows.

1. **Startup sweep.** On pane open, scan receipts whose outcome is non-terminal. For each
   `handed-off` past a staleness threshold (or with no external ack), surface it as a **visible
   "pending, outcome unknown"** card state — the founder sees it, never a silent background
   re-dispatch. (REQUIREMENTS: "A stopped factory → visible pending.")
2. **Explicit re-confirm / re-dispatch.** A human action that takes a `handed-off`/unknown
   receipt and either re-dispatches (same digest + same `handoff_id` → idempotent, no double
   accept) or cancels. This is the only thing that moves an unknown state toward a terminal one,
   and it is always human-gated.
3. **Status query against coordinator/planner.** The board may ask the external
   coordinator/planner for the current state of a handed-off request (aligning on its existing
   status surface) to resolve `unknown` → `accepted`/`refused` **without re-running**. The
   external answer is untrusted data: it is recorded, but final state authority stays with the
   board's own receipt and the digest it verified.
4. **Idempotent replay.** Re-delivery of an already-terminal receipt returns the stored receipt
   as a no-op, so any retry or re-delivery is safe.

Reconciliation is bounded by the explicit-only trigger: the sweep *surfaces*, the re-confirm and
status query are *actions*, and none of them auto-dispatches work.

---

## cf-queue / coordinator boundary

coordinator/planner is external. The board owns the board-side of the bridge and only *aligns on*
the external side's contract.

| Board owns | Board aligns on (does NOT own) |
|---|---|
| The receiver, its single entrypoint, and its privacy guarantees. | The acceptance surface coordinator/planner exposes — the field/verb contract the board must satisfy to hand a request over. |
| The outbox and receipt store, and the receipt fields/outcome states. | coordinator/planner's queue and SDLC state — the board reads status only, never mutates it. |
| Actor provenance (trust root = herdr workspace identity). | coordinator/planner's own authn/authz, if any — separate; the board never re-implements it. |
| Dedup (digest key) and active-attempt uniqueness (constraint). | coordinator/planner's own dedup/idempotency, if any — out of scope. |
| The G2 digest verification and `Mismatch`/`Refusal` mapping. | cf-queue's internal request encoding — the board never re-owns it (G2 §4). |

Consequence, made explicit: the board guarantees **"exactly one proven, deduplicated, receipted
handoff per digest"** and then hands the request over, recording the external response verbatim
but treating it as untrusted. The board enforces nothing *inside* coordinator/planner; its
invariant ends at the bridge. The board-side handoff fits beneath the existing acceptance
surface — no new public verb.

---

## Open questions

1. **Exact trust-root field.** G1 proved herdr injects `HERDR_*` context (plugin, pane/tab,
   workspace ids), but the precise workspace/operator-identity field to use as the actor trust
   root must be confirmed against herdr 0.9.0's documented environment. Fallback if herdr
   exposes no operator id: the OS user the pane runs as. (Blocking for §3, not for the outbox.)
2. **Handoff mechanism.** Is the handoff a direct in-process call (board and coordinator/planner
   co-resident in the herdr process) or a subprocess/CLI invocation? Changes whether the receiver
   is a function call vs a spawned process, but not the outbox/receipt design.
3. **Staleness threshold.** How long before a `handed-off` receipt is surfaced as "outcome
   unknown" in the startup sweep, and confirmation that re-dispatch is always explicit (never
   auto) — expected yes, matching sync-never-starts-work.
4. **cf-queue acceptance verb + status surface.** The exact external acceptance contract and the
   status-query surface to align on for reconciliation (carried from G2 §5 q4).

### Decisions taken here (resolving G2 open items)

- **Revision semantics (G2 q1/q2):** `revision` = the GitHub issue `updated_at`, RFC3339 UTC,
  stored verbatim. It is the board's "which issue version was this request built against" marker
  for display and conflict diffs — not the content-integrity witness. Content integrity is
  carried by the G2 digest itself (the body is hashed), so a coarse/coinciding `updated_at` does
  not weaken integrity. Consequence: any GitHub edit (metadata or content) surfaces as a visible
  conflict, and the human applies or discards — consistent with the authority model.
- **Repeated-click convergence (G2 q5):** resolved above (§Active-attempt uniqueness).
