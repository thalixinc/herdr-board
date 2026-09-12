# Plan: ticket #13 — G5 — Factory-kind at draft creation
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change
- `src/kind.rs` (new, dev-1) — `FactoryKind` enum (`Ordinary`/`FactoryRequest`), canonical serialization (`ordinary`/`factory-request`), `as_str`, `FromStr`, `Display`, `is_factory_request`
- `src/digest/canonical.rs` (dev-1) — add `factory_kind: FactoryKind` to `CanonicalRequest` as the **first data field**; update framing order + docs
- `src/digest/mod.rs` (dev-1) — `SCHEMA_VERSION = 2`; framing-table doc update
- `src/digest/hash.rs` (dev-1) — add `compute_with_version(&CanonicalRequest, u64) -> Digest`; `compute` = `compute_with_version(_, SCHEMA_VERSION)`
- `src/digest/compare.rs` (dev-1) — `StoredFields` gains `schema_version: u64` + `factory_kind: FactoryKind`; `verify` canonicalizes under `stored_fields.schema_version` (version-on-record), not the compile-time constant
- `src/digest/conflict.rs` (dev-1) — `Field` enum gains `FactoryKind`
- `src/outbox/mod.rs` (dev-1) — migration: `ALTER TABLE requests ADD COLUMN schema_version INTEGER NOT NULL DEFAULT 1; ADD COLUMN factory_kind TEXT NOT NULL DEFAULT 'factory-request'`
- `src/outbox/receipt.rs` (dev-1) — `RequestRecord` gains `schema_version` + `factory_kind`; `insert_pending` writes both
- `src/receiver/mod.rs` (dev-1) — `receive()` builds `RequestRecord` with `schema_version: SCHEMA_VERSION` + `factory_kind: request.factory_kind`; `reconstruct_request` reads both back
- `src/lib.rs` (dev-1 + dev-2) — `pub mod kind; pub mod card;` + re-export `FactoryKind`, `create_draft`, `promote_to_factory_request`, `Draft`
- `src/card/mod.rs` (new, dev-2) — the draft store: `drafts` table, `create_draft()` (atomic INSERT with `factory_kind NOT NULL DEFAULT 'ordinary'`), `promote_to_factory_request()` (single-transaction `Ordinary→FactoryRequest` + persist the request), `get_draft()`
- `src/outbox/mod.rs` (dev-2) — migration: `drafts` table + `create_intents.factory_kind` backfill (`NULL`→`'ordinary'`) + NOT NULL enforcement
- `tests/digest_v2.rs` (new, dev-1) — v1↔v2 framing + version-on-record verification + cross-version mismatch
- `tests/draft_creation_lifecycle.rs` (new, dev-2) — atomic draft creation, monotonic transition, enforce-across-paths, inertness
- `examples/factory_kind_demo.rs` (new, dev-2) — runnable smoke: draft → promote → demote-rejected → inertness

`src/main.rs` is **unchanged** (the "factory request → coordinator/planner" badge renders with the board vertical slice). G2/G3/G4 are consumed, not replaced: G2's framing is bumped in place; G3's `requests` table is extended; G4's `create_intents.factory_kind` is made non-null and populated from the draft.

## Resolved decisions (design left these open — resolved here, or named assumptions)
- **factory_kind JOINS the canonical set → `SCHEMA_VERSION` 1→2** (design q1: recommended). Position = first data field (discriminator reads first): `frame(version) || frame(factory_kind) || frame(identity) || frame(revision) || frame(factory) || frame(actor) || frame(body)`.
- **Existing digests remain verifiable via *version-on-record*.** `verify()` canonicalizes under the persisted record's `schema_version`, not the compile-time constant. A v1 request (`schema_version=1`, no `factory_kind` frame) still verifies under the v1 layout; a v2 request verifies under v2. The design's "stray v1 fails verify" remark is superseded: with the `schema_version` column (DEFAULT 1 backfill), old records *verify*, they don't fail. This is the standing migration path for every future kind (design q5).
- **G4 `create_intents.factory_kind` = the same `FactoryKind` enum, never NULL after G5.** The publish path copies the draft's `factory_kind` into the create-intent in the same INSERT. **Invariant:** at every lifecycle point a draft has a definite kind; publish copies it verbatim; no path yields a NULL kind. Backfill pre-G5 `NULL` rows → `'ordinary'`, then enforce NOT NULL.
- **Target-factory binding stays at the explicit action** (design q2): `which` factory is G2's `factory` field, captured at "Process with factory" — not at draft creation. Draft fixes only the *kind*.
- **Demotion never permitted** (design q3): `FactoryRequest→Ordinary` is rejected; "cancel" is a terminal *outcome* on the request, not a kind change.
- **Named assumption (G4 coordination):** G4's `CreateIntent` gains `factory_kind: FactoryKind` (non-optional) and its `issue()` copies the draft's kind. If G4 has not merged when G5's dev-2 lands, dev-2's contract states the field and the coordinator sequences G4 first; the `create_intents` NOT NULL enforcement is G5's migration step either way.

## Store shape — migration delta (so validation is concrete)
- `requests` (existing, G3): add `schema_version INTEGER NOT NULL DEFAULT 1`, `factory_kind TEXT NOT NULL DEFAULT 'factory-request'`. The default is honest: a `CanonicalRequest` only ever comes from a `FactoryRequest` card.
- `drafts` (new, dev-2): `draft_id TEXT PK, title TEXT NOT NULL, body TEXT NOT NULL, labels TEXT NOT NULL, assignee TEXT, factory_kind TEXT NOT NULL DEFAULT 'ordinary', created_at INTEGER NOT NULL`. `factory_kind` is a first-class column from the instant the row exists — there is no "kind unset" state and no post-hoc UPDATE that sets it.
- `create_intents` (G4): `UPDATE create_intents SET factory_kind='ordinary' WHERE factory_kind IS NULL`, then NOT NULL (authored `NOT NULL DEFAULT 'ordinary'` in G4, or a guarded table rebuild in G5 if G4 shipped nullable).

## Dev split
Contract both halves share (written first, in `src/kind.rs`): `FactoryKind` + its canonical serialization. G3's `Store`/`insert_pending` and G2's `compute`/`verify` are the frozen boundaries dev-2 builds on.

- **dev-1 = FactoryKind domain + G2 schema bump (v1→v2) + version-on-record verification + G3 ripple.** `src/kind.rs`, `src/digest/{canonical,mod,hash,compare,conflict}.rs`, `src/outbox/{mod,receipt}.rs`, `src/receiver/mod.rs`, `src/lib.rs`, `tests/digest_v2.rs`. Independently implementable: `FactoryKind` exists, the canonical set is six fields, `SCHEMA_VERSION=2`, `verify` is version-aware, and the crate builds + existing tests pass (golden vectors updated — every digest changes). Touches no draft/card store.
- **dev-2 = draft store + atomic creation + monotonic transition + enforce-across-paths + G4 backfill.** `src/card/mod.rs`, `src/outbox/mod.rs` (drafts migration), `src/lib.rs`, `tests/draft_creation_lifecycle.rs`, `examples/factory_kind_demo.rs`. Independently implementable: consumes `FactoryKind` (dev-1) + `Store`/`insert_pending` (G3, updated by dev-1); implements `create_draft` + `promote_to_factory_request` and the enforce-across-paths tests. *(dev-2 session is degraded — this half is self-contained and reassignable.)*

## Order of work
1. [dev-1] `src/kind.rs`: `FactoryKind` + canonical serialization + round-trip tests.
2. [dev-1] `src/digest/canonical.rs`: add `factory_kind` as first data field; reorder framing; update docs.
3. [dev-1] `src/digest/mod.rs`: `SCHEMA_VERSION = 2`; update framing table.
4. [dev-1] `src/digest/hash.rs`: `compute_with_version`; `compute` wrapper.
5. [dev-1] `src/digest/compare.rs`: `StoredFields` + `schema_version` + `factory_kind`; `verify` canonicalizes under `stored_fields.schema_version`.
6. [dev-1] `src/digest/conflict.rs`: `Field::FactoryKind`; diff display.
7. [dev-1] `src/outbox/{mod,receipt}.rs` + `src/receiver/mod.rs`: `RequestRecord`/`requests`/`receive`/`reconstruct_request` carry `schema_version` + `factory_kind`.
8. [dev-1] `src/lib.rs`: `pub mod kind;` + re-exports; update existing tests + golden vectors (v2 digests).
9. [dev-1] `tests/digest_v2.rs`: v1↔v2 framing, version-on-record verify, cross-version mismatch.
10. [dev-2] `src/card/mod.rs`: `drafts` table + `create_draft` (atomic INSERT) + `promote_to_factory_request` (single tx: set kind + persist request) + `get_draft`.
11. [dev-2] `src/outbox/mod.rs`: migration for `drafts` + `create_intents.factory_kind` backfill/NOT NULL.
12. [dev-2] `src/lib.rs`: `pub mod card;` + re-exports; G4 `CreateIntent`/`issue()` copy the draft kind (coordination).
13. [dev-2] `tests/draft_creation_lifecycle.rs` + `examples/factory_kind_demo.rs`.
14. Full gate: test / clippy / fmt / llvm-cov / example.

## Validation Strategy
- Unit — `cargo test --locked --lib`: `FactoryKind` serialization/round-trip; v2 framing (version frame = `"2"`, `factory_kind` first); `compute` deterministic; `compute_with_version(v1)` ≠ `compute_with_version(v2)` for identical fields; `Field::FactoryKind` diff.
- Integration — `cargo test --locked --test digest_v2` (dev-1) and `cargo test --locked --test draft_creation_lifecycle` (dev-2).
- Edge cases (named fixtures): draft created with default `ordinary`; `promote_to_factory_request` sets kind + persists the request in one transaction (a rollback leaves neither kind changed nor a request row — crash-safety); second promote → rejected; demote (`FactoryRequest→Ordinary`) → rejected; a v1 record (`schema_version=1`) verifies under v1 and a v2 record under v2, but a v1 record presented as v2 (and vice-versa) → `Mismatch`; `create_intents.factory_kind` never NULL after backfill; `receive()` only accepts a `factory_kind == FactoryRequest` request (structurally — no ordinary card builds a `CanonicalRequest`).
- E2E / smoke — `cargo run --example factory_kind_demo` walks create-ordinary-draft → promote → demote-rejected → inertness against a tempdir SQLite file.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/kind.rs` + `src/digest/` + `src/card/`.

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --test digest_v2         # v1/v2 framing + version-on-record + cross-version mismatch green
cargo test --locked --test draft_creation_lifecycle # atomic creation + monotonic transition + enforce-across-paths + inertness green
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report >= 90% line coverage on src/kind.rs + src/digest/ + src/card/
cargo run --example factory_kind_demo        # stdout shows: draft 'ordinary' -> promoted 'factory-request' + a v2 digest, demote REFUSED, no dispatch
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% threshold; the example prints a draft with `ordinary`, a promotion to `factory-request` with a new v2 digest (different from any v1 digest), a rejected demote, and no work dispatched. Evidence artifacts land in `intent/8-board-plugin/tickets/13-g5-factory-kind-at-draft-creation/evidence/`.

## Risks
- **Riskiest step — the G2 schema bump + version-on-record verification (steps 3–5).** Changing the canonical set invalidates every digest and re-touches the frozen `verify`/`CanonicalRequest`/`StoredFields`/`RequestRecord`/`receive` contract. If `verify` canonicalizes with the *compile-time* version instead of the record's `schema_version`, old records either fail spuriously or a tampered version frame is silently accepted. Mitigation: version-on-record (`compute_with_version` + `StoredFields.schema_version`), the version still hashed as frame 0, golden vectors for *both* v1 and v2 framing, and a cross-version mismatch test.
- **The atomic `promote_to_factory_request` transition (step 10).** If "set `factory_kind=FactoryRequest`" and "persist the request" are not one transaction, a crash leaves a factory card with no request — the exact bug G5 closes. Mitigation: single transaction + a rollback test asserting neither the kind flip nor the request row survives a failed commit.
- **Rejected and why:** leaving factory-kind out of the digest (G2 explicitly flagged it in; it future-proofs for new kinds without another bump); a second `FactoryKind` enum in G4/board duplicating the digest one (one canonical definition + serialization); setting the kind via a post-hoc UPDATE after draft creation (the crash-drop bug — it must be atomic at creation); binding the target `factory` at draft (the *which* factory resolves at the explicit action — G2's `factory` field); permitting `FactoryRequest→Ordinary` demotion (cancel is a terminal outcome, not a kind change).
