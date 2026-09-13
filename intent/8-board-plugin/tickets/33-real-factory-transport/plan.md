# Plan: ticket #33 — Wire the real factory transport (board → coordinator via herdr-axi send)
Epic: #8. Date: 2026-09-13. Status: ready. Base: `vs/board-ui-kanban` @ 88766b5
(transport files are identical on `main`; #33 does not depend on the render commit).

## Files that change

- `src/receiver/transport.rs` — the concrete `send` closure (replacing the stub in
  `main.rs`'s wiring) + the disposition→`CfSubmission` mapping:
  - Add a `herdr_axi` subprocess helper (mirroring herdr-axi's own
    `send_with_bin`/`abridge::send` seam): serialize the `CfQueueContract` as the
    message body (all five fields verbatim — the board's own form, never re-encoding
    cf-queue's), then invoke `herdr-axi --request-file <envelope>` with
    `operation = send`, `station = (project, coordinator)`, `params.text = body`,
    `message_id = handoff_id`. Synchronous by construction (matches the frozen
    `HandoffTransport::handoff` blocking contract between two committed write-ahead
    transactions).
  - Map the `herdr-axi send` disposition onto `CfSubmission`/`HandoffResult`:
    `submitted` → `Accepted(response)` (receipt finalizes `accepted`; coordinator's
    accept/reject resolves later via `status_query`); `not-submitted` (busy/blocked/
    unverified) → `Failed(reason)` (receipt stays `handed-off`, retryable via
    `reconfirm`); `unknown` (crash/timeout after possible dispatch) → `Failed(reason)`
    (stays `handed-off`, never auto-rerun).
  - Keep `RealHandoffTransport` (in-memory credentials only) with the real closure;
    a new `AxiHandoffTransport` that impls `HandoffTransport` is acceptable if cleaner.
- `src/main.rs` — replace the hardcoded
  `|_contract| CfSubmission::Failed { reason: "cf-queue wire invocation is a later integration" }`
  closure with the real transport constructor (`RealHandoffTransport::new(
  Credentials::from_env(), herdr_axi_send)`), keeping the `HERDR_*` context read for
  project/state-dir resolution (see assumptions).
- `src/lib.rs` — re-export the new constructor/helper (`AxiHandoffTransport` /
  `herdr_axi_send`), so `main.rs` and tests import from `herdr_board::`.

No change to `src/factory/mod.rs` (the entrypoint already takes `transport`),
`src/outbox/`, `src/receiver/{handoff,reconcile,mod}.rs` (the three-way outcome +
receipt states are frozen), or the store.

## Assumptions (sliced here — not re-routed to founder; grounded in REQUIREMENTS.md factory boundary)

- **Delivery = push-to-terminal via `herdr-axi send`.** The board's outbox is the
  durable, reconcilable layer (stopped factory → visible `handed-off` pending,
  `reconfirm` re-dispatches). No durable request-queue surface is required — herdr-axi
  has none, and building one would be a cross-project request.
- **Target-factory resolution = board-side config.** The board reads the herdr-axi
  project name + state dir + role from `HERDR_*` context + a manifest/
  `HERDR_PLUGIN_CONTEXT_JSON` field (dev fallback constant). Resolution is board-side;
  no herdr-axi change.
- **No cross-project request.** herdr-axi already exposes `herdr-axi send`
  (`Operation::Send`) via `--request-file`/`--stdin`; the board **consumes** it and
  never re-implements machine/attach/transport verbs (REQUIREMENTS boundary, verbatim).
- **Target role = the factory's `coordinator`** (`DEFAULT_FACTORY = "coordinator"`,
  already the `f` action's target). The coordinator accepts/rejects via its own queue +
  SDLC state; the board never files the queue item itself.

## Order of work

1. [transport] `herdr_axi` subprocess helper (serialize `CfQueueContract` → envelope,
   invoke `herdr-axi --request-file`, read disposition) + the disposition→`CfSubmission`
   three-way mapping.
2. [transport] Wire the helper into `RealHandoffTransport`/`AxiHandoffTransport`
   (in-memory credentials, `message_id = handoff_id` for idempotent re-dispatch).
3. [main] Replace the stub closure with the real constructor; keep `resolve_repo` +
   add the `HERDR_*` project/state-dir read for station resolution.
4. [lib] Re-export the constructor.
5. [test] Unit (fake `herdr-axi` binary on `PATH`) + integration (`process_with_factory`
   → real transport → stub binary) + demo smoke; record evidence.

## Validation Strategy

- Unit — `cargo test --locked --test transport_reconcile` (extend; fake `herdr-axi`
  binary, same `send_with_bin` seam herdr-axi uses in tests): a canned `submitted`
  envelope → `Accepted`; `not-submitted: busy` → `Failed`; `unknown` → `Failed`; the
  serialized body contains all five `CfQueueContract` fields verbatim (no re-encoding).
- Integration — `cargo test --locked --test factory_transport` (new, or extend
  `transport_reconcile`): drive `process_with_factory` with the real transport wired to
  a stub `herdr-axi` on `PATH`; assert a `submitted` disposition finalizes the receipt
  `accepted`, a `not-submitted` leaves it `handed-off`, and `reconfirm` re-dispatches
  idempotently (same `message_id`).
- Smoke — `cargo run --example factory_outbox_demo` (extend): swap the fake
  `Accepting`/`Stopped` transports for the real one pointed at a stub `herdr-axi`,
  proving the request leaves the board and a disposition returns.

## Proof

```sh
cargo test --locked --test transport_reconcile   # three-way mapping + verbatim body green
cargo test --locked --test factory_transport      # accepted/handed-off + idempotent reconfirm green
cargo run --example factory_outbox_demo           # request leaves the board, disposition returns
```
Expected: exit 0 on all; `submitted`→`Accepted`/`not-submitted`→`Failed`/`unknown`→
`Failed`; the serialized body carries all five `CfQueueContract` fields; `reconfirm`
re-sends the same `message_id`. (Full-suite + clippy/fmt run once at integration, per
crew rule 5.)
Evidence lands in `intent/8-board-plugin/tickets/33-real-factory-transport/evidence/`.

## Risks

- **Disposition mapping (step 1).** `herdr-axi send` returns `submitted` /
  `not-submitted` / `unknown`; the board's three-way outcome is
  `Accepted` / `Refused` / `Failed`. `not-submitted` and `unknown` both map to
  `Failed` with a distinct reason (busy vs outcome-unknown) — never auto-rerun
  `unknown`. Mitigation: unit test each disposition; assert `unknown` does not retry.
- **Idempotent re-dispatch (step 2).** `message_id = handoff_id` must be stable across
  `reconfirm` so herdr-axi dedups; a fresh id per retry would double-deliver.
  Mitigation: integration test asserts `reconfirm` re-sends the same `message_id`.
- **Subprocess contract (step 1).** Blocking `herdr-axi --request-file` must match the
  frozen synchronous `HandoffTransport::handoff` contract; a dev tempted to
  `spawn`+poll breaks the write-ahead ordering. Mitigation: synchronous by
  construction; note in the helper doc-comment.
- **Boundary creep.** The board consumes `herdr-axi send`; it must not re-implement
  station resolution, readiness gating, or the `herdr agent prompt` (REQUIREMENTS:
  transport belongs to herdr-axi). Mitigation: the helper only shells out and maps
  the disposition; no transport logic in the board.
