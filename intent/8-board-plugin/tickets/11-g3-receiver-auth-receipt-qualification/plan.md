# Plan: ticket #11 — G3 — Receiver/auth/receipt qualification
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change
- `Cargo.toml` — add `rusqlite = { version = "0.32", features = ["bundled"] }`, `uuid = { version = "1", features = ["v4"] }`, `time = { version = "0.3", features = ["formatting"] }`
- `src/lib.rs` — add `pub mod outbox; pub mod receiver;` + re-export the public API (`Store`, `Receipt`, `Outcome`, `Receiver`, `receive`, `Handoff`, `HandoffTransport`, `Actor`, `TrustRoot`, reconcile entrypoints)
- `src/outbox/mod.rs` (new, dev-1) — `Store` (connection + migrations): `open(path)`, `open_in_memory()`, `default_path()`; `PRAGMA journal_mode=WAL` + `foreign_keys=ON`; migration v1 DDL (`requests`, `receipts`, two partial unique indexes below)
- `src/outbox/receipt.rs` (new, dev-1) — `Receipt`, `Outcome` (`pending`/`handed-off`/`accepted`/`refused`/`cancelled`), `ReceiptId`/`HandoffId` (uuid-v4 newtypes); insert/transition/query ops that enforce dedup + active-attempt uniqueness
- `src/receiver/mod.rs` (new, dev-2) — the sealed `Receiver` + single entry `receive()` + `ReceiverError`; the three-transaction write-ahead (pending → handed-off → terminal)
- `src/receiver/actor.rs` (new, dev-2) — `Actor`, `ActorSource`, `TrustRoot` (`from_env()` + OS-user fallback), `prove_actor()`
- `src/receiver/handoff.rs` (new, dev-2) — `Handoff { handoff_id, request }`, `HandoffTransport` trait (injectable seam), `ExternalResponse` (verbatim, untrusted), `HandoffResult`
- `src/receiver/reconcile.rs` (new, dev-2) — `startup_sweep`, `reconfirm`, `status_query`, `replay` entrypoints
- `tests/outbox_lifecycle.rs` (new, dev-1) — store CRUD, dedup, active-attempt, restart persistence
- `tests/receiver_lifecycle.rs` (new, dev-2) — full receive flow against a fake transport
- `examples/receiver_demo.rs` (new, dev-2) — runnable smoke of the whole receive → re-delivery → drift-refusal flow

`src/main.rs` is **unchanged**; the visible "pending, outcome unknown" card state renders with the board vertical slice. `src/digest/` is consumed as-is (its API is frozen).

## Resolved decisions (design left these open — resolved here, or named assumptions)
- **SQLite crate** — `rusqlite` (bundled, synchronous). Rejected `sqlx` (async/tokio + compile-time macros; the TUI stack is synchronous and single-writer — no async runtime exists). **DB path** — `$HERDR_PLUGIN_STATE_DIR/herdr-board.sqlite3` (G1-documented state dir); `Store::default_path()` falls back to `~/.local/state/herdr-board/` when the env is unset (bare `cargo run`). Tests use `open_in_memory()`/tempdir, never the env.
- **Actor trust root** — `HERDR_WORKSPACE_ID` (G1 verified herdr injects it; no dedicated operator-id field exists in 0.9.0). **Named assumption:** when `HERDR_WORKSPACE_ID` is absent, fall back to the OS user (`USER`/`whoami`); `actor_source` records which was used.
- **Handoff mechanism** — an in-process injectable `HandoffTransport` trait. The real coordinator/planner transport (subprocess/CLI) is a later vertical slice; receiver + outbox + receipt are fully implemented and tested against a fake transport now. The trait's `ExternalResponse` is recorded verbatim and treated as untrusted.
- **Dedup scoping (design's "digest UNIQUE" is ambiguous)** — dedup and active-attempt uniqueness are enforced by **partial** unique indexes over *non-terminal* receipts only. G2 forbids salting the digest (same input ⇒ same digest), and the convergence rule requires a terminal receipt to *not* block a fresh attempt of identical input. A global `UNIQUE(digest)` would be wrong.
- **Staleness threshold** — `STALENESS_THRESHOLD = 5 minutes` (const, overridable): a `handed-off` receipt older than this surfaces as "outcome unknown" in the sweep. Re-dispatch is always human-gated.
- **Carried forward (non-blocking)** — cf-queue's exact acceptance verb + status-query surface (design q4): the handoff trait's `ExternalResponse` is opaque, so this is a G3-later/vertical-slice dependency, not a G3 blocker.

## Store shape (so validation is concrete)
- `requests(request_id PK, digest BLOB UNIQUE NOT NULL, digest_id TEXT, identity TEXT, revision TEXT, factory TEXT, actor TEXT, actor_source TEXT, body TEXT, created_at INTEGER)` — the durable home of G2's `Digest` + `StoredFields` (G2 deferred "persist atomically with the request"; G3 delivers it because dedup/active-attempt key on `request_id`).
- `receipts(receipt_id TEXT PK, handoff_id TEXT UNIQUE NOT NULL, request_id FK→requests, digest BLOB, digest_id TEXT, actor TEXT, actor_source TEXT, identity TEXT, revision TEXT, factory TEXT, outcome TEXT CHECK(pending|handed-off|accepted|refused|cancelled), external_response TEXT NULL, created_at INTEGER, finalized_at INTEGER NULL)`.
- Partial unique indexes (both are the race/restart-safe enforcement):
  - `ON receipts(request_id) WHERE outcome IN ('pending','handed-off')` — **at most one active attempt per request**.
  - `ON receipts(digest) WHERE outcome IN ('pending','handed-off')` — **never double-accept an in-flight input**.
- Global `UNIQUE(handoff_id)` — idempotent re-delivery of the same attempt (any outcome).
- Receipts never store credentials (constraint); digest stored as the 32-byte blob, `digest_id` as the 16-hex display.

## Dev split
Contract both halves share (written first, in `src/outbox/mod.rs`): the `Store` API (`open`/`open_in_memory`/`default_path`, receipt ops), and the `Receipt`/`Outcome`/`ReceiptId`/`HandoffId`/`RequestRecord` types. `src/digest/` types (`Digest`, `DigestId`, `StoredFields`, `verify`, `Mismatch`, `Refusal`) are already frozen.

- **dev-1 = durable outbox + receipt store** (`Cargo.toml`, `src/lib.rs`, `src/outbox/*`, `tests/outbox_lifecycle.rs`). Independently implementable: schema, `Store`, receipts, dedup + active-attempt constraints. Verified purely against SQLite (tempdir + in-memory) — no external system, no env.
- **dev-2 = receiver orchestration + actor provenance + handoff seam + reconciliation** (`src/receiver/*`, `tests/receiver_lifecycle.rs`, `examples/receiver_demo.rs`). Independently implementable: consumes dev-1's `Store` and G2's `verify`, plus the injectable `HandoffTransport`; implements `receive()` and the reconcile entrypoints. Verified with a fake transport.

## Order of work
1. [dev-1] Add `rusqlite`/`uuid`/`time` to `Cargo.toml`; `cargo build --locked` resolves them.
2. [dev-1] `src/outbox/mod.rs`: `Store` + migration v1 DDL (two tables + two partial unique indexes) + WAL/foreign-keys pragmas + `default_path()`.
3. [dev-1] `src/outbox/receipt.rs`: `Receipt`/`Outcome`/`ReceiptId`/`HandoffId`; `insert_pending` (acquire slot), `transition_to`, `get_by_handoff_id`, `get_active_by_digest`, `get_active_by_request`, `list_non_terminal`.
4. [dev-1] Wire `pub mod outbox;` + re-exports in `src/lib.rs`.
5. [dev-1] `tests/outbox_lifecycle.rs`: CRUD; dedup (same digest in-flight → constraint); active-attempt (second in-flight row for same request → constraint); restart persistence (close + reopen same file); WAL survives reopen.
6. [dev-2] `src/receiver/actor.rs`: `Actor`/`ActorSource`/`TrustRoot`/`prove_actor()` (env → OS-user fallback).
7. [dev-2] `src/receiver/handoff.rs`: `Handoff`, `HandoffTransport`, `ExternalResponse`, `HandoffResult`.
8. [dev-2] `src/receiver/mod.rs`: `receive()` — prove actor → check `handoff.request.actor` matches trust root → `verify()` → dedup → acquire active slot → insert `pending` → commit `handed-off` → call transport → commit `accepted`/`refused`. Return `Receipt` or `ReceiverError` (`Refusal(Mismatch)`, actor mismatch, active-attempt-exists, transport error).
9. [dev-2] `src/receiver/reconcile.rs`: `startup_sweep` (list non-terminal; surface stale `handed-off`), `reconfirm` (re-dispatch same digest + same `handoff_id` → idempotent), `status_query` (record external answer, board stays authority), `replay` (idempotent no-op).
10. [dev-2] Wire `pub mod receiver;` + re-exports in `src/lib.rs`.
11. [dev-2] `tests/receiver_lifecycle.rs` + `examples/receiver_demo.rs`.
12. Full gate: test / clippy / fmt / llvm-cov / example.

## Validation Strategy
- Unit — `cargo test --locked --lib`: store ops + receipt ops + actor provenance + error mapping.
- Integration — `cargo test --locked --test outbox_lifecycle` (dev-1) and `cargo test --locked --test receiver_lifecycle` (dev-2).
- Edge cases (named fixtures in the suites): empty actor → refuse; actor swapped after persist (forged) → `ReceiverError::ActorMismatch`; identical re-delivery (same `handoff_id`) → idempotent no-op, same receipt id returned; same input with a *new* `handoff_id` while in-flight → deduped to the active receipt; revision-bumped body → `Refusal` with a `Mismatch` field diff; crash between `handed-off` and finalize (reopen → `handed-off` row persists and the sweep surfaces it); two racing receives for one request → exactly one active receipt (constraint aborts the loser).
- E2E / smoke — `cargo run --example receiver_demo` walks receive → re-delivery no-op → drift refusal against a fake transport + a real tempdir SQLite file, and prints the receipt + outcome + refusal.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/outbox/` + `src/receiver/`.

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --test outbox_lifecycle  # dedup + active-attempt + restart-persistence green
cargo test --locked --test receiver_lifecycle # receive + idempotency + drift-refusal + crash-recovery green
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report >= 90% line coverage on src/outbox/ + src/receiver/
cargo run --example receiver_demo            # stdout shows: receipt id + outcome, re-delivery no-op (same receipt id), then a drift REFUSED with field diff
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% threshold; the example prints a UUID receipt id and a terminal outcome for the clean path, the *same* receipt id on idempotent re-delivery, and a `REFUSED` (never `accepted`) for the drifted input. Evidence artifacts land in `intent/8-board-plugin/tickets/11-g3-receiver-auth-receipt-qualification/evidence/`.

## Risks
- **Riskiest step — the transactional write-ahead + the partial-index constraint scoping (steps 2–3 and 8).** If the dedup/active-attempt indexes are scoped wrong (e.g. a global `UNIQUE(digest)`), the board either double-accepts a replayed handoff or blocks a legitimate fresh attempt of identical input after a terminal outcome — both break the "exactly one proven, deduplicated, receipted handoff" invariant, and the digest cannot be salted to disambiguate (G2 forbids it). Mitigation: non-terminal-scoped partial indexes, and a dedicated concurrency/restart test that proves exactly-one-winner and idempotent re-delivery.
- **`HERDR_WORKSPACE_ID` trust-root semantics (named assumption).** If herdr 0.9.0's workspace id does not actually identify the operator (e.g. one workspace shared by many users), the OS-user fallback carries the provenance. G3 records `actor_source` on every receipt so a wrong assumption is visible, not silent.
- **Rejected and why:** `sqlx`/async SQLite (unnecessary async runtime + macros; TUI is synchronous, `rusqlite` bundled is the boring fit); a spawned-subprocess handoff in G3 (deferred — the transport is a later slice; the injectable `HandoffTransport` trait keeps receiver + outbox fully testable now); reading the actor from the request body/card (payload is data, not context — forgeable); in-memory active-attempt tracking (a `Mutex<HashSet>` does not survive restart or cross-process races — the store-level partial unique index is the only race/restart-safe enforcement); a global `UNIQUE(digest)` (blocks a legitimate fresh attempt of identical input after a terminal outcome, contradicting G2's no-salt determinism and the convergence rule); auto re-dispatch in the startup sweep (sync never starts work — the sweep surfaces, never dispatches).
