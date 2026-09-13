# Plan: ticket #25 — VS3 — "Process with factory" outbox → coordinator/planner
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change

### dev-1 — explicit entrypoint + convergence (worktree `/Users/chrismckenna/development/herdr-board-d1`, branch `vs/vs3-dev1`)
- `src/factory/mod.rs` (new) — `process_with_factory(store, transport, draft_id, card_identity, target_factory) -> Result<Receipt, ProcessError>` + `ProcessError`; the promote → resolve-card → prove-actor → build-`CanonicalRequest` → `receive` composition
- `src/outbox/mod.rs` — **migration v7**: identity-scoped active-attempt partial unique index (`receipts(identity) WHERE outcome IN ('pending','handed-off')`) so "one active request per *issue*" holds
- `src/outbox/receipt.rs` — `classify_insert_constraint` maps the new index to `StoreError::ActiveAttemptExists`
- `src/lib.rs` — `pub mod factory;` + re-export `process_with_factory`, `ProcessError`
- `tests/process_lifecycle.rs` (new) — promote→build→receive→receipt; convergence (identical re-click, drifted re-click, terminal→fresh)

### dev-2 — real transport shape + reconciliation/stopped-factory (worktree `/Users/chrismckenna/development/herdr-board-d2`, branch `vs/vs3-dev2`)
- `src/receiver/transport.rs` (new) — `RealHandoffTransport` (the board→cf-queue translation point + VS1 `Credentials` injection) + the documented `CfQueueContract` field alignment + `impl HandoffTransport`
- `src/receiver/reconcile.rs` — real behavior: `startup_sweep(store, now)` (threshold filter), `reconfirm(store, receipt_id, transport)` (re-verify + re-dispatch), `status_query(store, receipt_id, accepted, response)`, `cancel_receipt(store, receipt_id)`, `replay` (kept idempotent)
- `src/receiver/mod.rs` — `mod transport;` + re-exports
- `src/lib.rs` — re-export `RealHandoffTransport`, `CfQueueContract`, `cancel_receipt`
- `tests/transport_reconcile.rs` (new) — sweep threshold; reconfirm idempotent re-dispatch; status_query records; cancel terminal; translation mapping
- `examples/factory_outbox_demo.rs` (new) — runnable smoke of the full flow incl. a stopped-factory sweep

`src/main.rs` is **unchanged**; the "pending, outcome unknown" badge renders with the TUI vertical slice. G2 `CanonicalRequest`/`compute`, G3 `receive`/`HandoffTransport`/receipts/`reconstruct_request`, G5 `promote_to_factory_request`, and VS1 `Store::get_card`/`Credentials` are consumed as-is — VS3 adds no outbox, it composes them.

## Resolved decisions (design left these open — resolved here, or named assumptions)
- **Entrypoint** — `process_with_factory(store, transport, draft_id, card_identity, target_factory) -> Receipt`. It composes: `promote_to_factory_request(store, draft_id)` (body + `FactoryRequest`, monotonic) → `store.get_card(card_identity)` (identity + revision) → `TrustRoot::from_env().prove()` (actor) → build `CanonicalRequest` (the six fields) → `Handoff { handoff_id: new_v4(), request }` → `receive(store, &handoff, transport)`. It **discards** the placeholder `RequestRecord`/digest `promote_to_factory_request` returns (that digest is over an empty identity) and lets `receive()` derive the real digest from the fully-populated request.
- **Request field sources** — `factory_kind = FactoryRequest` (G5), `identity`/`revision` from the linked card, `factory = target_factory` (per-action parameter, design q5), `actor` from the trust root (never the card/body), `body` = the **draft's** board-authored payload (design q6 — never the synced issue body).
- **One active request per *issue*** (design q2) — scope active-attempt uniqueness to identity, not digest: migration v7 adds `idx_receipts_active_identity ON receipts(identity) WHERE non-terminal`. A drifted re-click on an in-flight issue now converges (`ActiveAttemptExists`) instead of double-running; a terminal receipt never blocks a fresh re-request (the constraint keys on non-terminal only). The existing digest/request-scoped indexes stay (the digest one still enforces in-flight dedup).
- **Real transport shape** *(named assumption)* — `HandoffTransport` (already frozen) stays the seam. `RealHandoffTransport` is the single translation point board-fields → cf-queue's **field contract** (identity, revision, factory, actor, body); it never re-owns cf-queue's encoding. It maps external outcomes to the three-way `HandoffResult` (`accepted`→`Accepted`, `refused`→`Refused`, unreachable/timeout→`Failed`, receipt stays `handed-off`). The concrete wire invocation (cf subprocess vs HTTP/IPC) is a **later integration** — VS3 delivers the translation + the injectable seam, fake-tested; credentials from VS1 `Credentials`, in-memory only.
- **Reconciliation is real now** (G3 left these no-ops): `startup_sweep(store, now)` filters `handed-off` older than `STALENESS_THRESHOLD` (visible pending, never a spawned agent); `reconfirm` re-verifies the digest against the persisted record then re-dispatches (idempotent); `status_query` records the out-of-band answer and transitions the receipt (board stays authority); `cancel_receipt` is a first-class human-gated terminal transition (design q4).
- **Draft↔card linkage** *(named assumption)* — the entrypoint takes both `draft_id` and `card_identity` explicitly (the TUI resolves them; the persisted draft→card link is VS2's later card-link, not re-added here).

## Shared contract (frozen first)
Already frozen (in the repo; both halves consume, neither modifies): `CanonicalRequest` (6 fields), `compute`, `receive`, `Handoff`, `HandoffTransport::handoff -> HandoffResult`, `ExternalResponse`, `Receipt`, `ReceiptId`/`HandoffId`, `TrustRoot::from_env()`, `reconstruct_request` (pub(crate)), `STALENESS_THRESHOLD`, `promote_to_factory_request`, `FactoryKind`, `Store::{get_card, insert_pending, transition_to, get_by_handoff_id, get_active_by_digest, list_non_terminal, receipt_by_id}`.

New, authored first into both worktree bases:
- **dev-1**: `process_with_factory(store: &Store, transport: &impl HandoffTransport, draft_id: &str, card_identity: &Identity, target_factory: &str) -> Result<Receipt, ProcessError>`; `ProcessError` (wraps `CardError`/`ReceiverError`/card-not-found).
- **dev-2**: `startup_sweep(store, now) -> Vec<Receipt>`; `reconfirm(store, receipt_id: &ReceiptId, transport: &impl HandoffTransport) -> Result<Receipt, …>`; `status_query(store, receipt_id, accepted: bool, response: &str) -> Result<Receipt, …>`; `cancel_receipt(store, receipt_id) -> Result<Receipt, …>`; `RealHandoffTransport` + `CfQueueContract`.

dev-1 and dev-2 do **not** call each other — both build on the frozen G3 surface, and they touch disjoint files (`src/factory/` + `src/outbox/` vs `src/receiver/`), so each worktree compiles and tests independently. The coordinator merges either order; `src/lib.rs` re-exports are the only shared edit and are additive.

## Order of work
1. [contract] Commit the frozen signatures (entrypoint + reconcile) to both worktree bases.
2. [dev-1] `src/outbox/mod.rs`: migration v7 (identity-scoped active-attempt index); `src/outbox/receipt.rs`: classify the new constraint.
3. [dev-1] `src/factory/mod.rs`: `process_with_factory` + `ProcessError` composition.
4. [dev-1] `pub mod factory;` + re-exports in `src/lib.rs`; write `tests/process_lifecycle.rs`; gate in `herdr-board-d1`.
5. [dev-2] `src/receiver/transport.rs`: `CfQueueContract` translation + `RealHandoffTransport` (credentials + three-way mapping).
6. [dev-2] `src/receiver/reconcile.rs`: real `startup_sweep`/`reconfirm`/`status_query`/`cancel_receipt`/`replay`.
7. [dev-2] `mod transport;` + re-exports in `src/receiver/mod.rs` and `src/lib.rs`; write `tests/transport_reconcile.rs` + `examples/factory_outbox_demo.rs`; gate in `herdr-board-d2`.
8. [coordinator] Merge dev-1 + dev-2; run the full-suite gate on the merged tree.

## Validation Strategy
- Unit — `cargo test --locked --lib`: `ProcessError` mapping; `CfQueueContract` translation (six fields → contract fields, no re-encoding); `HandoffResult` three-way mapping.
- Integration — `cargo test --locked --test process_lifecycle` (dev-1) and `cargo test --locked --test transport_reconcile` (dev-2).
- Edge cases (named fixtures): first click promotes + receives + returns a receipt (digest over the *real* six fields, not the placeholder); identical re-click → `ActiveAttemptExists` returning the existing active receipt; drifted-revision re-click on the same in-flight issue → converges (identity constraint), no second attempt; terminal receipt then re-click → fresh request (new digest if input changed); `promote` second time → `AlreadyPromoted`; transport `Failed` → receipt stays `handed-off`; `startup_sweep` surfaces only `handed-off` past `STALENESS_THRESHOLD`; `reconfirm` on a drifted record refuses (re-verify) rather than re-dispatching; `status_query(accepted=true)` → `accepted`; `cancel_receipt` → `cancelled` (terminal); actor mismatch (claimed ≠ proven) → `ActorMismatch`.
- E2E / smoke — `cargo run --example factory_outbox_demo` walks promote→process→receipt, then a `Failed` transport → `handed-off` → sweep surfaces → `reconfirm`/`status_query` resolves, against a fake transport + tempdir SQLite.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/factory/` (dev-1) + `src/receiver/transport.rs` + `src/receiver/reconcile.rs` (dev-2).

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --test process_lifecycle # promote->build->receive->receipt + convergence green
cargo test --locked --test transport_reconcile # sweep/reconfirm/status_query/cancel/translation green
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report >= 90% line coverage on src/factory/ + src/receiver/transport.rs + src/receiver/reconcile.rs
cargo run --example factory_outbox_demo      # stdout shows: receipt id + outcome, an identical re-click -> existing receipt, a Failed handoff -> handed-off -> sweep -> resolved
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% threshold; the example prints a receipt (with a digest over the real six-field request), an `ActiveAttemptExists` on an identical re-click (no second attempt), a `handed-off` receipt after a `Failed` transport (never silently re-run), and a terminal outcome after `reconfirm`/`status_query`. Evidence artifacts land in `intent/8-board-plugin/tickets/25-vs3-factory-outbox/evidence/`.

## Risks
- **Riskiest step — the composition boundary (dev-1 step 3).** `process_with_factory` must feed `receive()` a `CanonicalRequest` whose `body` comes from the *draft* and whose `identity`/`revision` come from the *card* — and must discard the placeholder digest `promote_to_factory_request` returns. If it hands `receive()` the placeholder record (empty identity) or the card's issue body instead of the draft's payload, the digest covers the wrong execution input and the whole G2 guarantee ("what runs == what was asked") silently breaks. Mitigation: a test asserting the receipt's digest equals `compute()` over the exact six-field request built from draft-body + card-identity/revision.
- **The identity-scoped convergence constraint (dev-1 step 2).** Adding `idx_receipts_active_identity` must be classified correctly or a drifted re-click either double-runs (constraint missing) or is misreported. Mitigation: `classify_insert_constraint` maps the new index to `ActiveAttemptExists`, and a test proves a drifted re-click on an in-flight issue converges while a terminal receipt still permits a fresh request.
- **Rejected and why:** computing/persisting the digest in the entrypoint instead of `receive()` (the digest is owned by `receive()`; the entrypoint only builds the request); a background/timer that re-dispatches `handed-off` receipts (sync never starts work — the sweep surfaces, never re-runs); re-owning cf-queue's request encoding in the transport (align on the field contract only — G2 §4 / founder boundary); reading the actor from the card/body (trust root only — `ActorMismatch` otherwise); scoping convergence to digest alone (a drifted re-click would double-run the same issue — identity-scoped instead); making `HandoffTransport` async (the receiver calls it synchronously between two committed write-ahead transactions — blocking by contract).
