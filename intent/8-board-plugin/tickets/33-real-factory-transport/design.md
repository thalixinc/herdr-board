# Design: Wire the real factory transport (board → coordinator/planner)

Epic: #8 · Ticket: #33 · Seat: Brainstorm · Date: 2026-09-13 · Stage: plan (design only)

## Decoded requirement

The outbox and `process_with_factory` are fully built and tested, but the transport that actually
reaches coordinator/planner is a stub: `src/main.rs` wires
`RealHandoffTransport::new(credentials, |_contract| CfSubmission::Failed { reason: "cf-queue wire
invocation is a later integration" })`. So a "Process with factory" click persists a request and a
receipt, then always records `Failed` — the request **never reaches coordinator/planner**. This
ticket replaces that stub with the real transport, consuming the surface that owns it.

REQUIREMENTS factory boundary is explicit: the board owns *only* the board + the trigger BRIDGE;
**transport belongs to herdr-axi** ("herdr-axi (machine/attach verbs, transport) → herdr-axi
factory"; "the board CONSUMES what those factories produce; if the board needs something that
doesn't exist yet, file it in the owning factory"). So the decisive question is: does herdr-axi
already expose the transport verb, or must a dependency be filed?

## Transport answer (decisive)

**herdr-axi already exposes the verb the board must consume — no cross-project request is
required for the transport.** The board consumes **`herdr-axi send`** (the `Operation::Send`
envelope).

Evidence, read from the herdr-axi checkout (`/Users/chrismckenna/development/herdr-axi`) and the
Chief-of-Staff registry:

1. **`herdr-axi send <project> <role> <text> [--message-id <id>]`** is implemented and verified —
   `src/facade.rs` (`send` arm → `Operation::Send`), `src/ops.rs::Ops::send` (persist intent →
   readiness gate → native `herdr agent prompt` → returns `submitted` / `not-submitted` /
   `unknown`), and the README walkthrough exercises a correlated `send`→`read` exchange. Exit
   codes 0/1/2; `--json` returns the raw `EnvelopeResponse`.
2. **The machine entry point** is `herdr-axi --request-file <envelope.json>` / `--stdin`
   (`src/main.rs`, "the adapter beneath cf team") — the same dispatch over a structured
   `EnvelopeRequest` (`src/identity.rs`), with bounded body bytes. This is the path the board
   consumes; the facade is the thin human surface over it.
3. **`herdr-axi read`/`observe`** (`Operation::Observe`) is the reconciliation surface for
   `status_query` (read the coordinator's recent output). Already implemented.
4. **`herdr-axi report`** (`Operation::Report`, `intent/43-report-return-channel`) is the
   *"option-13 / herdr-axi return channel"* the founder referenced: a **seat → Chief-of-Staff**
   completion/blocker channel into the Abridge passive inbox (`recipient = abridge::COF`). It is
   the producer half of option-13 and is **not** the board's trigger — its recipient is CoS, not
   coordinator/planner, and it requires a provisioned station binding (the board is a plugin pane,
   not a crew seat). Disambiguation matters: the board does **not** consume `report` for this
   ticket.

## The board-side wire-up

`RealHandoffTransport<F>` (`src/receiver/transport.rs`) already owns the board-side translation
(`CanonicalRequest` → `CfQueueContract` { identity, revision, factory, actor, body }) and the
three-way `HandoffResult` mapping. The only missing piece is the concrete `send` closure. Replace
the stub with a real closure that shells out to herdr-axi:

1. **Serialize.** Encode `CfQueueContract` as the message body (a stable text/JSON form carrying
   all five fields — the board's own format; the board owns its representation, it never re-owns
   cf-queue's encoding — G2 §4 boundary). Pass `--message-id <handoff_id>` so a re-dispatch is
   idempotent at the transport (`herdr-axi send` dedups on `message_id`).
2. **Invoke.** Blocking subprocess: `herdr-axi --request-file <envelope>` with
   `operation = send`, `station = (project, coordinator)`, `params.text = body`, `message_id =
   handoff_id` — **synchronous by construction**, matching the frozen
   `HandoffTransport::handoff` contract (blocking between two committed write-ahead transactions).
   The target role is the factory's **coordinator** (`DEFAULT_FACTORY = "coordinator"`, already
   the `f` action's target); the coordinator accepts/rejects via its own queue + SDLC state — the
   board never files the queue item itself.
3. **Map dispositions** (the board's three-way outcome):

   | `herdr-axi send` disposition | `CfSubmission` / `HandoffResult` | Receipt |
   |---|---|---|
   | `submitted` | `Accepted(response)` | finalize `accepted` (delivery accepted; coordinator's accept/reject resolves later via `status_query`) |
   | `not-submitted` (busy / blocked / unverified target) | `Failed(reason)` | stays `handed-off` (retryable via `reconfirm`) |
   | `unknown` (crash/timeout after possible dispatch) | `Failed(reason)` | stays `handed-off` (outcome unknown, never auto-rerun) |

   This is the same three-way contract the G3/VS3 designs already froze; `herdr-axi send`'s own
   disposition enum maps onto it one-to-one with no new surface.

Concrete files:
- **`src/receiver/transport.rs`** — add a `herdr_axi` subprocess helper (mirroring herdr-axi's own
  `send_with_bin`/`abridge::send` pattern) + the disposition→`CfSubmission` mapping; keep
  `RealHandoffTransport` (or a new `AxiHandoffTransport` that impls `HandoffTransport`) with
  in-memory credentials only.
- **`src/main.rs`** — replace the hardcoded `Failed` closure with the real transport constructor.
- **`src/lib.rs`** — re-export the new constructor.
- No change to `src/factory/mod.rs` (the entrypoint already takes `transport`), `src/outbox/`, or
  the store.

## Boundary (founder rule, restated)

| Board owns | herdr-axi owns |
|---|---|
| `CfQueueContract` serialization (the board's own request form), the `send` invocation, the three-way outcome mapping, the receipt/digest state, reconciliation. | The transport itself: station resolution, the readiness gate, the native `herdr agent prompt`, the `submitted`/`not-submitted`/`unknown` disposition, the `send`/`read`/`report` verbs. |
| The board's outbox as the durable, reconcilable layer ("stopped factory" → visible pending, re-confirm). | Delivery to the role; the board reads the disposition verbatim and never mutates coordinator/planner queue/SDLC state. |

The board **consumes** `herdr-axi send`; it never re-implements machine/attach/transport verbs
(REQUIREMENTS boundary, verbatim).

## Validation strategy

- **Unit — `src/receiver/transport.rs`** (fake `herdr-axi` binary, same `send_with_bin` seam
  herdr-axi itself uses in tests): a canned `submitted` envelope → `Accepted`; `not-submitted:
  busy` → `Failed`; `unknown` → `Failed`; and the serialized body contains all five
  `CfQueueContract` fields verbatim (no re-encoding). Run: `cargo test --locked --test
  transport_reconcile`.
- **Integration — `tests/transport_reconcile.rs`** (or a new `tests/factory_transport.rs`): drive
  `process_with_factory` with the real transport wired to a stub herdr-axi on `PATH`; assert a
  `submitted` disposition finalizes the receipt `accepted`, a `not-submitted` leaves it
  `handed-off`, and `reconfirm` re-dispatches idempotently (same `message_id`). Run:
  `cargo test --locked --test factory_transport`.
- **Smoke — `examples/factory_outbox_demo.rs`** (extend): swap the fake `Accepting`/`Stopped`
  transports for the real one pointed at a stub `herdr-axi`, proving the request actually leaves
  the board and a disposition comes back. Run: `cargo run --example factory_outbox_demo`.

## Open questions / founder questions

1. **Delivery semantics (founder — the one real policy call).** This design delivers to the
   coordinator's **terminal** via `herdr-axi send` (push-to-terminal, retryable through the
   board's outbox when the coordinator is busy/blocked). The alternative — a **durable
   request-queue surface** the coordinator drains on its own schedule — does **not** exist in
   herdr-axi today (the only durable channel is `report` → CoS Abridge inbox, wrong recipient)
   and would be a cross-project request. Confirm push-to-terminal is intended; REQUIREMENTS
   "hands to coordinator/planner, which accepts/rejects via current queue + SDLC state" reads as
   push-to-terminal (the board's outbox is the durability layer), but it is the founder's call.
2. **Target-factory resolution (founder/wiring).** How does the board learn *which* coordinator to
   target for the scoped factory — the herdr-axi **project name + state dir + role**? Resolution
   is board-side config (read `HERDR_*` context + a manifest/`HERDR_PLUGIN_CONTEXT_JSON` field or a
   chooser), not a herdr-axi change, but the contract ("factory name ↔ herdr-axi project/role") is
   a decision to pin before the Dev wires it.

Neither blocks writing the transport mapping; #1 is the acceptance-semantics confirmation, #2 is
the config contract.
