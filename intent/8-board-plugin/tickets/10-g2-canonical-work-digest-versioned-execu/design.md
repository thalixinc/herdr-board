# Design: G2 — Canonical work digest (versioned execution-input digest)

Epic: #8 · Ticket: #10 · Seat: Brainstorm · Date: 2026-09-12 · Stage: plan (design only)

## Decoded requirement

Guarantee that the *execution input* a card's "Process with factory" action persisted is
byte-for-byte the input that is later dispatched to the coordinator/planner and accepted for
execution. If anything drifts between "what was requested" and "what is about to run", the
drift must surface as a **visible conflict and the factory must refuse to run** — never silently
execute stale or rewritten input.

The digest is a property of the **board's request record**, not of the card's display state and
not of cf-queue's internal encoding.

---

## 1. Canonical field set + serialization order

The digest covers exactly the five fields the board needs to identify and serialize a factory
request (founder boundary). They are hashed **in this fixed order**:

| # | Field | Canonical form | Notes |
|---|---|---|---|
| 1 | `identity` | `owner/repo#number` (owner/repo lowercased, `number` decimal ASCII) | Stable identity; number alone is insufficient (REQUIREMENTS field mapping). URL is derivable, not a separate field. |
| 2 | `revision` | opaque token, stored **verbatim** | The issue revision the request was built against. Exact semantics open (GitHub `updated_at`, cf-queue revision, or content hash) — see §5. |
| 3 | `factory` | target factory identifier, verbatim | Which coordinator/planner pipeline the request targets. |
| 4 | `actor` | trusted actor identity, in the form G3 defines | Provenance of the explicit trigger; ties to the receiver/auth repair. |
| 5 | `body` | the request body as the board persisted it, **verbatim UTF-8 bytes** | The execution-input payload handed to coordinator/planner. Never re-encoded, trimmed, or reordered. |

**Why order matters for determinism.** A digest hashes a byte string. "Same input → same
digest" only holds if that byte string is canonical, and the single most common source of
non-canonical bytes is *serialization order* — map/set iteration order, struct-layout order, or
the order a store happens to return fields. Those are not stable guarantees across processes,
versions, or languages. Fixing an explicit order removes that nondeterminism entirely: any
order works, provided it is **documented, fixed, and versioned**. We fix this order and treat
a reorder as a schema bump (§2).

Two further determinism rules:

- **No delimiter, only length-prefix framing.** Values may contain newlines, Unicode, or any
  delimiter we could choose, so a delimiter-separated encoding is ambiguous. Each field is
  framed as `[byte length][raw canonical bytes]`; the digest input is the concatenation of the
  six frames (version frame + five field frames, in order).
- **No accidental normalization.** A value is canonicalized exactly once at persist time and
  stored; comparison reuses that same stored representation. Bodies are never "prettified" or
  re-serialized, which would change bytes without changing meaning.

---

## 2. Digest algorithm + versioning scheme

- **Algorithm.** SHA-256 over the framed byte string from §1. The full 32-byte digest is the
  source of truth for comparison. A short prefix (first 8 bytes, hex — Git-style) is the human
  "digest id" shown on the request and in conflict diffs; it is display-only and never compared.
- **Versioning.** `schema_version` is the **first frame** of the hashed input (an integer,
  starting at `1`), so it is inside the digest, not alongside it.

  - **A schema bump is required** whenever any of the following changes: the field set (add /
    remove / rename a field), the field order, or the canonicalization rule of any field —
    i.e. any change that could make "same logical input" produce different bytes, or make two
    previously distinct inputs collide. Because the version is hashed, the bump *itself* changes
    the digest even if the five field bytes are identical, so old and new digests can never be
    confused.
  - **Same version + same input ⇒ same digest.** Guaranteed by the fixed order + length-prefix
    framing + verbatim values. No timestamps, process ids, random salts, or store-dependent
    ordering participate.
  - **A digest is never recomputed to "repair" a mismatch.** A mismatch is a conflict to
    surface (§3), not an error to silently fix — recomputing and overwriting would destroy the
    exact guarantee this ticket exists to provide.

---

## 3. Where computed, where compared, and mismatch behavior

**Computed once — at persist.** When "Process with factory" persists the request
(identity, revision, target factory, actor, request body), the board computes the digest over
those five fields and stores it **atomically with the request record**. From that moment the
digest is the request's integrity witness.

**Compared twice — dispatch and acceptance.**

| Point | Who | What is compared |
|---|---|---|
| Dispatch | the bridge handing the request to coordinator/planner | Recompute over the dispatch-time input; compare to the stored digest. |
| Acceptance | coordinator/planner, when it accepts/rejects against current queue + SDLC state | Recompute over the acceptance-time input; compare to the stored digest. |

Acceptance is a second, later gate because the issue can change between dispatch and acceptance
(revision bump, body edit), or a request can be retried/restored onto a different input. It is
the **last gate before work starts**; the invariant is "what runs == what was persisted".

**Mismatch behavior — refuse, visibly, never silently.**

- The factory **refuses to run**. No stale or rewritten input executes.
- The board shows a **visible conflict** with a field-level diff: which field changed, old vs
  new value (revision, body, factory, actor, identity), plus the two digest ids.
- **Resolution is human.** The founder either (a) re-confirms on the *current* input, which
  re-persists the request and produces a **new** digest, or (b) cancels the request. The board
  never auto-wins, never auto-recomputes, and never leaves the card silently "pending" as if
  the request were valid.
- This is the same visible-conflict authority model as the rest of the board
  (REQUIREMENTS.md: "if the linked issue changed, show …; never silently win").

---

## 4. Field-contract boundary with cf-queue

cf-queue is an **external dependency**. The board aligns on its *field contract* only; it never
re-owns or re-implements cf-queue's request encoding.

| Board owns | Board aligns on (does NOT own) |
|---|---|
| The canonical field set, its order, and each field's canonical form (§1). | The **set of fields** cf-queue requires to accept a request — their names and semantics — so the board populates them correctly. |
| The framing, the digest algorithm, and the versioning scheme (§2). | cf-queue's **internal serialization / wire encoding** of a request (how coordinator/planner/SDLC represent and transmit it). |
| Where the digest is computed and compared, and the mismatch policy (§3). | cf-queue's own integrity/validation, if any — separate and out of scope. |
| The five fields as the **board** persists them. | Anything cf-queue does *after* the board hands the request over. |

Consequences, made explicit:

- The digest is computed over the **board's own canonical representation** of the five fields,
  never over cf-queue's serialized bytes. It is the board's guarantee that "what I persisted ==
  what I dispatched / you accepted" — it does **not** attempt to predict or validate cf-queue's
  post-translation encoding.
- The translation board-fields → cf-queue-fields is the **bridge** (G3 receiver/auth), not a
  digest concern. The digest does not cover cf-queue's encoding of the translated request.
- If cf-queue's field contract changes (new required field, rename), the board updates its field
  mapping — and per §2 that is a **schema bump**, because the canonical field set changed.

---

## 5. Open questions (carried forward)

1. **Revision semantics.** Is `revision` GitHub's `updated_at`, a cf-queue monotonic revision,
   or a content hash? This changes what "the issue changed" means, not determinism. The board
   stores the token verbatim either way.
2. **Content vs metadata revision.** GitHub bumps `updated_at` on any edit (label, assignee,
   body). If a metadata-only edit should *not* invalidate a request, `revision` may need to
   split into a content-affecting signal vs a metadata signal — or the body field alone carries
   content and `revision` becomes advisory. Decide before G3 wires the bridge.
3. **G5 field inclusion.** G5 ("factory-kind at draft creation") adds a field (factory intent)
   persisted atomically at draft time. It belongs in this canonical set; confirm it lands here
   as a **schema bump** (field-set change), keeping G2's scheme authoritative and G5 the additive
   consumer.
4. **Who recomputes at acceptance.** Does coordinator/planner recompute the digest itself, or
   does the bridge verify and hand over a verified receipt? Depends on G3's receiver boundary;
   the digest id should appear on G3's durable receipt regardless.
5. **Repeated-click convergence.** "Repeated clicks converge on one active request" — does a
   re-click re-persist (new digest) or reuse the existing active request's digest? Specifies
   whether the digest is per-click or per-active-request.
6. **Digest id length.** Is the 8-byte (16-hex) display prefix sufficient for humans to spot a
   drift at a glance, or is a shorter/longer prefix wanted? (Full 32-byte comparison is
   unaffected.)
