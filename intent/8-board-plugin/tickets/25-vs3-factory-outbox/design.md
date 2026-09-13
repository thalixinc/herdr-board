# Design: VS3 — "Process with factory" outbox → coordinator/planner

Epic: #8 · Ticket: #25 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

The card's explicit "Process with factory" action persists a factory request — repo/issue
identity, target factory, actor, issue revision — and hands it to coordinator/planner through the
G3 receiver/outbox bridge. Sync never starts work; the **only** trigger is this explicit action.
Repeated clicks converge on one active request; a stopped factory leaves a **visible pending**
receipt, never a standalone card agent.

This slice is the *join* of the already-built pieces: it wires G5 (draft promotion) → G2 (digest)
→ G3 (receive/handoff/receipt/reconciliation) into a single board-side action, and defines the
**real** `HandoffTransport`'s shape. It adds no new outbox — it composes what exists.

---

## Explicit entrypoint

The board-side seam is one function, the only caller being the TUI action (a manifest
`[[actions]]` entrypoint). No timer, no loop, no scheduler reaches it; `autospin=ask` applies.

**Signature (shape):**

```
process_with_factory(store, transport, draft_id, card_identity, target_factory) -> Receipt
```

**Orchestration (each step delegates, nothing is re-invented):**

1. **Promote the draft (G5).** `promote_to_factory_request(store, draft_id)` enforces the
   monotonic `ordinary → factory-request` transition (`AlreadyPromoted` on a second click) and
   yields the request `body` + `factory_kind`. This is the one explicit kind transition; it never
   auto-fires a handoff.
2. **Resolve the linked card (VS1).** Read the card by `card_identity` → its `revision` (GitHub
   `updated_at`, verbatim). The card supplies identity + revision; the draft supplies body + kind.
3. **Prove the actor.** `TrustRoot::from_env().prove()` — the actor is set from the trust root
   (herdr workspace identity → OS user), **never** from the card or body. `receive()` re-proves and
   enforces it, so a mismatched/claimed actor is refused (`ActorMismatch`).
4. **Build the `CanonicalRequest`** (see §3) and a fresh `Handoff { handoff_id: HandoffId::new_v4(),
   request }`.
5. **`receive()` (G3)** — the single authority that computes the digest, persists the
   `RequestRecord`, dedups, acquires the active-attempt slot, drives the three-transaction
   write-ahead, hands off, and finalizes. The entrypoint returns the `Receipt`.

The action is **explicit-only**: recording a request is inert until this action runs; sync (VS1)
and push (VS2) never call it.

---

## Request build + digest

**Field sources** (all six `CanonicalRequest` fields, fixed G2 order):

| Field | Source |
|---|---|
| `factory_kind` | G5 promotion → `FactoryRequest` |
| `identity` | the linked card's `Identity` (`owner/repo#number`, lowercased) |
| `revision` | the linked card's `revision` (GitHub `updated_at`, verbatim) |
| `factory` | the action's `target_factory` (which factory the user selected) |
| `actor` | `TrustRoot::from_env().prove().value` |
| `body` | the **draft's** body — the board-authored execution-input payload, *not* the issue body |

**Digest is owned by `receive()`.** The entrypoint does **not** compute or persist a digest: it
hands the `CanonicalRequest` to `receive()`, which calls `compute(request)` (G2), persists the
`RequestRecord` with that digest, and verifies re-deliveries against it. One consequence to note
explicitly: `promote_to_factory_request` returns a `RequestRecord` whose digest is a **placeholder**
computed over an empty identity — VS3 discards that artifact and lets `receive()` derive the real
digest from the fully-populated request. The digest thus covers the actual execution input
("what runs == what the card shows + what was asked"), satisfying G2's contract.

---

## Transport seam → handoff

`HandoffTransport` (G3) is the injectable seam:

```
fn handoff(&self, request: &CanonicalRequest) -> HandoffResult
// Accepted(ExternalResponse) | Refused(ExternalResponse) | Failed(String)
```

It is **sync by contract** — `receive()` calls it between two committed write-ahead transactions,
so the real transport must be blocking (subprocess or blocking HTTP), never async.

**The real transport's shape (defined here; the process integration is a later slice):**

- It is the **single translation point** board-fields → cf-queue contract fields. It takes the six
  `CanonicalRequest` fields and maps them onto cf-queue's acceptance contract (identity, revision,
  target factory, actor, body). It **aligns on the field contract only** and never re-implements
  cf-queue's request encoding (G2 §4 / founder boundary).
- It invokes the coordinator/planner acceptance surface — the concrete mechanism (a `cf`/`herdr`
  subprocess, an HTTP/IPC call) is an open question; the shape is fixed: synchronous, injectable,
  three-way `HandoffResult`.
- It maps each external outcome: `accepted` → `Accepted`, `refused` (a normal decision) →
  `Refused`, unreachable/timeout → `Failed` (receipt stays `handed-off`, outcome unknown).
- It is constructed with credentials from the same herdr surface (in-memory, never persisted), and
  a **fake** stands in for tests and the demo — exactly as the VS1/VS2 clients do.

The external response is recorded verbatim but treated as **untrusted**: it never overrides the
board's own receipt/digest state.

---

## Reconciliation / stopped-factory

A handoff whose outcome is unknown (transport `Failed`, or a crash between `handed-off` and
finalize) leaves the receipt in `HandedOff`. This is the "stopped factory" case, surfaced — never
re-run — via G3's existing reconciliation:

- **`startup_sweep(store, now)`** — `handed-off` receipts older than `STALENESS_THRESHOLD` (5 min)
  are listed; the board renders them as a **visible "pending, outcome unknown"** badge on the card.
  **Not a standalone card agent**: the pending state lives on the card's own receipt, no new card,
  no spawned agent.
- **`reconfirm(store, receipt, transport)`** — re-dispatch the same digest + same handoff,
  idempotent (re-verifies the digest against the persisted record first; a drift refuses rather
  than re-dispatching).
- **`status_query(store, receipt, accepted, response)`** — record an out-of-band external answer;
  the board stays the authority (never rewrites the digest).
- **`replay(store, receipt)`** — an idempotent no-op (returns the receipt, never re-runs).
- **Cancel** — a human-gated transition of the receipt to `Cancelled` (terminal); today reached via
  the store transition (open question: promote it to a first-class entrypoint alongside
  `reconfirm`).

Every one is human-gated; none auto-starts work.

---

## Convergence

Repeated clicks converge on one active request, enforced by G3's schema-level invariants (which
`receive()`/`insert_pending` trigger):

- **Identical input** (same identity/revision/factory/actor/body → same digest): `receive()` hits
  the active-digest dedup (`get_active_by_digest`) or the receipt insert's uniqueness constraint →
  `ActiveAttemptExists` → the entrypoint surfaces the **existing active receipt**, never a second
  attempt. (`insert_pending` also upserts the request row by digest, so a re-click reuses the same
  `request_id`.)
- **Re-delivery** (same `handoff_id`): `get_by_handoff_id` → verify → return the existing receipt
  unchanged (idempotent).
- **Terminal receipt** (accepted/refused/cancelled): no active slot is held, so a fresh click on
  the same card produces a **new** request (new digest if the input changed; a terminal never
  blocks a legitimate re-request).
- **Changed input** (revision drifted between clicks): a new digest → a new request row. This is
  correct per-request (the execution input differs), but it means the *previous* active request and
  the *new* one can momentarily coexist — see open questions.

---

## Boundary + open questions

| Board owns | External system owns |
|---|---|
| The explicit entrypoint, the request record, the G2 digest, the receipt, and the bridge. | coordinator/planner: the queue, SDLC state, and acceptance decision — the board reads the response verbatim, never mutates queue/state. |
| The `HandoffTransport` translation point (board-fields → contract fields) and its credential injection. | cf-queue: its request encoding and the exact acceptance verb — the board **aligns on the field contract, never re-owns it**. |
| Reconciliation, dedup, active-attempt uniqueness, and the visible-pending policy. | — (the board's guarantees end at the bridge). |

Credentials never in card/prompt/SQLite: the transport holds them in memory, injected at
construction.

**Open questions:**

1. **Draft↔card linkage.** How the entrypoint knows which `draft_id` corresponds to which
   `card_identity` (both are passed today). Is there a persisted draft→card link (`published_at` +
   issue number), or is it resolved by matching identity? Confirm the lookup before wiring.
2. **One active request per *issue* vs per *digest*.** Today's active-attempt uniqueness is keyed
   on `request_id` (per digest), so a changed-revision re-click can create a second active request
   for the *same* issue. Confirm whether "one active request" should be scoped to identity
   (`owner/repo#number`) so a drifted re-click refuses/converges instead of double-running.
3. **Real transport mechanism.** The concrete invocation — a `cf`/`herdr` subprocess vs an
   HTTP/IPC call — and the exact cf-queue acceptance verb to align on. The shape is fixed; the
   mechanism is the later integration.
4. **Cancel entrypoint.** Promote `Cancelled` from a raw store transition to a first-class,
   human-gated function beside `reconfirm`/`status_query`.
5. **Target-factory source.** Confirm `target_factory` is chosen per-action (user selection) vs
   bound at draft creation (G5 left "which factory" at the action); the entrypoint takes it as a
   parameter either way.
6. **Body authority.** Confirm the request `body` is the draft's board-authored payload (current
   design), not the card's synced issue body — so a GitHub-side body edit does not silently change
   the request the factory runs.
