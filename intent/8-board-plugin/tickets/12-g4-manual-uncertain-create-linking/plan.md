# Plan: ticket #12 — G4 — Manual uncertain-create linking
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change
- `Cargo.toml` — **no change** (reuses `rusqlite`/`uuid`/`time` from G3; the GitHub client is an injectable trait with a fake in tests — no HTTP dep yet)
- `src/outbox/mod.rs` — add **migration v2** (`create_intents` table) + `pub mod intent;` + re-export `CreateIntent`, `CreateOutcome`, `IntentId`, `Marker`; extend `StoreError` with `CreateIntentNotPending` + `DuplicateMarker`
- `src/outbox/intent.rs` (new, dev-1) — `CreateIntent`, `CreateOutcome` (`pending`/`issued`/`created`/`failed`/`cancelled`), `IntentId`/`Marker` (uuid-v4 newtypes), and the store ops that enforce the guarded `pending→issued` transition
- `src/lib.rs` — add `pub mod create;` + re-export the create API (`Publisher`, `issue`, `link`, `cancel`, `candidates`, `search_marker`, `GitHubClient`, `RepoIdentity`, `CreateResult`, `Candidate`)
- `src/create/mod.rs` (new, dev-2) — the sealed `Publisher` + single entry `issue()` + `CreateError`; the write-ahead (insert `pending` → commit `issued` → call client → finalize)
- `src/create/github.rs` (new, dev-2) — `GitHubClient` trait (injectable seam), `RepoIdentity` (canonical `owner/repo`, lowercased), `CreateResult` (`Created(u64)` / `Failed(String)` / `Uncertain(String)`), `Candidate`, `Issue`, `marker_comment()`
- `src/create/link.rs` (new, dev-2) — manual `link`/`cancel` entrypoints, `candidates()` (marker-first, title+timestamp fallback), `normalize_title()`
- `src/create/reconcile.rs` (new, dev-2) — `startup_sweep` (list `issued`), `search_marker` (read-only candidate pre-population), the block-auto-replay guard
- `tests/create_intent_lifecycle.rs` (new, dev-1) — store ops + guarded transition + marker uniqueness
- `tests/create_receiver_lifecycle.rs` (new, dev-2) — full issue → lost-response → link/cancel flow against a fake client
- `examples/create_link_demo.rs` (new, dev-2) — runnable smoke of draft → issue → uncertain → sweep → link

`src/main.rs` is **unchanged**; the "awaiting link" badge renders with the board vertical slice. `src/receiver/` and `src/digest/` are untouched; G4 reuses `src/outbox/` (same `Store`, same SQLite file) and G3's reconcile *pattern*.

## Resolved decisions (design left these open — resolved here, or named assumptions)
- **Create intent reuses G3's outbox as a *new table + dedicated enum*, not by extending `Outcome`.** `Outcome` is the handoff state machine (a `Receipt`'s terminal states are `accepted`/`refused`/`cancelled`); create states `issued`/`created`/`failed` are a *different* domain and would break that closure. So: **migration v2** adds `create_intents` with its own `CreateOutcome` enum and its own store ops, living in the same `Store`/SQLite file (same WAL, same `PRAGMA user_version` ladder). This is "reuse the outbox", not a parallel store.
- **Idempotency marker** — a UUID v4, stored as the bare UUID in the `marker` column (`TEXT NOT NULL UNIQUE`). The HTML comment `<!-- herdr-board:intent:<uuid> -->` is *derived* via `marker_comment()` and appended to the body only at send time — the intent's `body` column stays the human text verbatim. **Title+timestamp fallback**: exact-title search scoped to the repo, filtered to candidates within ±`STALENESS_THRESHOLD` of `issued_at`; `normalize_title()` = trim + collapse internal whitespace + ASCII-lowercase.
- **GitHub client seam** — injectable `GitHubClient` trait (mirrors G3's `HandoffTransport`), so `issue`/`search`/`link` are testable with a fake; no real network, no credentials in any code path. The token lives in the herdr-provided credential surface and is held by the client *constructor*, opaque to the create/link logic; it is never written to an intent row, card body, or marker.
- **Cancel semantics** — `cancel` only flips `cancelled` (stop tracking); it never deletes anything on GitHub. Deletion is a separate, explicit, human-confirmed destructive action — out of scope.
- **Staleness** — reuse G3's `STALENESS_THRESHOLD` (already re-exported); do **not** introduce a second value.
- **Fresh-retry after `failed`** — always a new intent (new `IntentId` + new `Marker`); the old `failed` row stays terminal, history intact.
- **Named assumption (design q1)** — GitHub's create API preserves and its search indexes the body HTML comment. If not, move the marker to a literal footer line; the reconciliation design is unchanged, only the marker's location.

## Store shape — migration v2 delta (so validation is concrete)
```sql
CREATE TABLE create_intents (
    intent_id    TEXT PRIMARY KEY NOT NULL,             -- UUID v4
    marker       TEXT NOT NULL UNIQUE,                  -- idempotency marker (bare UUID v4)
    repo         TEXT NOT NULL,                         -- canonical 'owner/repo', lowercased
    title        TEXT NOT NULL,
    body         TEXT NOT NULL,                         -- human body verbatim; marker comment appended at send
    labels       TEXT NOT NULL,                         -- JSON array, verbatim
    assignee     TEXT,
    factory_kind TEXT,                                  -- G5 field, nullable until G5 lands
    outcome      TEXT NOT NULL CHECK (outcome IN ('pending','issued','created','failed','cancelled')),
    issue_number INTEGER,                               -- set only on 'created'
    created_at   INTEGER NOT NULL,                      -- UTC unix seconds
    issued_at    INTEGER,                               -- set on pending -> issued
    finalized_at INTEGER                               -- set on terminal
);
```
`marker UNIQUE` = one intent per marker (the reconciliation key). **No partial unique index** (unlike G3): a create intent is keyed by a single marker, so block-auto-replay is enforced by a *guarded transition* — `UPDATE … SET outcome='issued' WHERE intent_id=? AND outcome='pending'` (0 rows ⇒ already issued/terminal ⇒ refuse). G3 needed partial indexes because two *different* `handoff_id`s race on one request/digest; no analogous race exists here.

## Dev split
Contract both halves share (written first, in `src/outbox/intent.rs`): `CreateIntent`, `CreateOutcome`, `IntentId`, `Marker`, and the `Store` ops (`insert_pending`, `mark_issued`, `finalize_created`, `finalize_failed`, `cancel`, `list_issued`, `get_by_marker`). G3's `Store`/`open`/`default_path`/`StoreError` are already frozen.

- **dev-1 = durable create-intent outbox** (`src/outbox/mod.rs` migration v2, `src/outbox/intent.rs`, `src/lib.rs` re-exports, `tests/create_intent_lifecycle.rs`). Independently implementable: schema v2, `CreateIntent`/`CreateOutcome`/`IntentId`/`Marker`, the guarded `pending→issued` transition, `list_issued`. Verified purely against SQLite (tempdir + in-memory) — no network, no client.
- **dev-2 = create orchestration + GitHub seam + manual link/cancel + reconciliation** (`src/create/{mod,github,link,reconcile}.rs`, `src/lib.rs` re-exports, `tests/create_receiver_lifecycle.rs`, `examples/create_link_demo.rs`). Independently implementable: consumes dev-1's store types + the injectable `GitHubClient`; implements `issue()`, link/cancel, candidate search, startup sweep, block-auto-replay. Verified with a fake client. *(dev-2 session is degraded — this half is self-contained and reassignable.)*

## Order of work
1. [dev-1] `src/outbox/mod.rs`: add `MIGRATION_V2` (`create_intents`) and extend `migrate()` to run it (`PRAGMA user_version` 1→2); extend `StoreError` with `CreateIntentNotPending` + `DuplicateMarker`.
2. [dev-1] `src/outbox/intent.rs`: `CreateIntent`/`CreateOutcome`/`IntentId`/`Marker`; `insert_pending` (marker UNIQUE), `mark_issued` (guarded conditional UPDATE), `finalize_created(number)`, `finalize_failed`, `cancel`, `list_issued`, `get_by_marker`.
3. [dev-1] Wire `pub mod intent;` + re-exports in `src/outbox/mod.rs` and `src/lib.rs`.
4. [dev-1] `tests/create_intent_lifecycle.rs`: CRUD; guarded transition (second `mark_issued` on an `issued` intent → `CreateIntentNotPending`); `DuplicateMarker` on re-insert; `list_issued`; restart persistence (close + reopen same file, migration v2 idempotent).
5. [dev-2] `src/create/github.rs`: `GitHubClient` trait + `RepoIdentity` + `CreateResult` (three-way) + `Candidate`/`Issue` + `marker_comment()`.
6. [dev-2] `src/create/mod.rs`: `issue()` — `insert_pending` → commit `mark_issued` → `client.create_issue` → on `Created` `finalize_created`, on `Failed` `finalize_failed`, on `Uncertain` leave `issued` (no transition). Return the intent + outcome; `CreateError` for store/client/not-pending failures.
7. [dev-2] `src/create/link.rs`: `link(intent_id, number)` (guarded from `issued`), `cancel(intent_id)` (guarded from `pending`/`issued`), `candidates(intent_id)` (marker search → title+timestamp fallback), `normalize_title()`.
8. [dev-2] `src/create/reconcile.rs`: `startup_sweep(store)` → `list_issued()`; `search_marker(client, store)` → read-only candidate pre-population for each `issued` intent; assert no path auto-issues/auto-links.
9. [dev-2] Wire `pub mod create;` + re-exports in `src/lib.rs`.
10. [dev-2] `tests/create_receiver_lifecycle.rs` + `examples/create_link_demo.rs`.
11. Full gate: test / clippy / fmt / llvm-cov / example.

## Validation Strategy
- Unit — `cargo test --locked --lib`: intent ops, `CreateOutcome` mapping, marker/comment derivation, guarded transition.
- Integration — `cargo test --locked --test create_intent_lifecycle` (dev-1) and `cargo test --locked --test create_receiver_lifecycle` (dev-2).
- Edge cases (named fixtures in the suites): lost response (`CreateResult::Uncertain` → intent stays `issued`, no `issue_number`); definite error (`Failed` → `failed`, terminal, safe to re-attempt as a fresh intent); double-issue race (second `mark_issued` on an `issued` intent → refused, exactly one create sent); duplicate marker (constraint); re-link after `created` → refused (guarded); `cancel` from `issued` allowed, from `created` refused (already linked); marker search miss → title+timestamp fallback with `normalize_title` case/whitespace; sweep surfaces `issued` and never auto-issues/auto-links.
- E2E / smoke — `cargo run --example create_link_demo` walks draft → issue (fake `Uncertain`) → startup sweep → link, against a tempdir SQLite file, and prints the intent + outcome + linked number.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/outbox/` + `src/create/`.

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --test create_intent_lifecycle   # guarded transition + marker uniqueness + restart green
cargo test --locked --test create_receiver_lifecycle # issue/lost-response/link/cancel/block-auto-replay green
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report >= 90% line coverage on src/outbox/ + src/create/
cargo run --example create_link_demo         # stdout shows: intent id + outcome, an 'issued' (awaiting link) state after a lost response, then a linked number after manual link
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% threshold; the example prints an intent id and the `issued` outcome after a lost response (never `created`, never a re-issue), then the human-confirmed linked issue number. Evidence artifacts land in `intent/8-board-plugin/tickets/12-g4-manual-uncertain-create-linking/evidence/`.

## Risks
- **Riskiest step — the block-auto-replay guarantee (step 2 + step 6).** If the `pending→issued` transition is not a *guarded conditional UPDATE*, or any code path re-issues an `issued`/`created` intent, a lost response turns into a duplicate GitHub issue — an irreversible external mutation and the exact bug this ticket exists to prevent. Mitigation: the compare-and-set UPDATE (`WHERE outcome='pending'`), no re-issue path anywhere, and a dedicated test proving a second `issue()` on an `issued` intent is refused and exactly one create is sent.
- **Marker searchability (named assumption, design q1).** If GitHub strips/does not index the body HTML comment, the primary marker search misses and resolution leans on the heuristic title+timestamp fallback. The create call itself still returns the number in the non-lost case; only the lost-response case depends on search. Non-blocking — reconciliation is unchanged if the marker moves to a literal footer.
- **Rejected and why:** extending `Outcome` with `issued`/`created`/`failed` (breaks the `Receipt` state-machine closure and pollutes the `receipts` CHECK + partial indexes); a parallel store/connection (violates "reuse G3's outbox" — one SQLite file + `Store` keeps a single WAL and migration ladder); auto-replaying `issued` intents in the startup sweep (sync never starts work — surface only); treating the marker as a credential (it is a public opaque UUID with no authority); inferring "definitely not created" from a search miss (GitHub search is eventually consistent — only a definite create-call error means `failed`); deleting a possibly-created issue on cancel (a separate destructive action, out of scope).
