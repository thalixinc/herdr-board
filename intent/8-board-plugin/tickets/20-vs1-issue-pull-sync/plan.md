# Plan: ticket #20 — VS1 — GitHub issue pull sync
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change

### Shared contract (authored first; committed to the base of *both* worktrees)
- `src/outbox/cards.rs` (dev-1) — the types + `Store` method *signatures* both halves bind to (dev-1 fills bodies; dev-2 codes against them)

### dev-1 — card store (worktree `/Users/chrismckenna/development/herdr-board-d1`, branch `vs/vs1-dev1`)
- `src/outbox/cards.rs` (new) — `Card`, `Conflict`, `CanonicalFields`, `CardField`, `CardFieldDiff`, and the `impl Store` card ops (`insert_card`, `get_card`, `list_cards`, `touch_card`, `record_conflict`, `get_conflict`, `apply_conflict`, `defer_conflict`) + label-list serialization
- `src/outbox/mod.rs` — add `mod cards;` + **migration v5** (`cards` + `card_conflicts`) + re-exports
- `src/lib.rs` — re-export the card-store types
- `tests/cards_store.rs` (new) — store CRUD, idempotent upsert, conflict record/apply/defer, label round-trip, restart persistence

### dev-2 — sync + read client + conflict mapping (worktree `/Users/chrismckenna/development/herdr-board-d2`, branch `vs/vs1-dev2`)
- `Cargo.toml` — add `ureq = "2"` (blocking HTTP, synchronous stack) + `serde`/`serde_json = "1"` (parse the read response)
- `src/sync/client.rs` (new) — `PullClient` trait (`list_issues`, `fetch_issue`), `IssueFull`, `Credentials::from_env()`/`from_herdr()`, `RealGitHubClient`
- `src/sync/mod.rs` (new) — `sync(repo) -> SyncSummary`, `IssueFull → CanonicalFields` mapping, revision compare + conflict detection, `apply_changes`, `defer_changes`, `DEFAULT_COLUMN`
- `src/sync/conflict.rs` (new) — field-diff computation over the 7 shared fields
- `src/lib.rs` — `pub mod sync;` + re-exports
- `tests/sync_lifecycle.rs` (new) — fake `PullClient` + in-memory store: insert / unchanged / conflict / apply / defer
- `examples/sync_demo.rs` (new) — runnable smoke of a full sync

`src/main.rs` is **unchanged**; the "changed" badge + "issue changed; apply?" prompt render with the TUI vertical slice. `src/digest/`, `src/receiver/`, `src/create/`, `src/card/`, `src/kind.rs` are consumed, not modified (VS1 pulls issues as ordinary cards; promotion is a later slice).

## Resolved decisions (design left these open — resolved here, or named assumptions)
- **`cards` schema (migration v5)** — `cards` uses the composite primary key `(owner, repo, number)` (the issue identity *is* the key; `UNIQUE` is a PK property, not a separate index), plus a `card_conflicts` table keyed `(owner, repo, number, field)`. Columns per the design table; `column TEXT NOT NULL` (board-local, sync never rewrites it), `factory_kind TEXT NOT NULL DEFAULT 'ordinary'`, `conflict TEXT NOT NULL DEFAULT 'none' CHECK (conflict IN ('none','apply-pending'))`, `revision` = GitHub `updated_at` verbatim (G3's decision). `labels` uses the same serialization convention as `create_intents.labels`.
- **Read client** — a read-only `PullClient` trait (`list_issues`/`fetch_issue`) implemented by one `RealGitHubClient` that *also* implements G4's publish `GitHubClient` — one construction point, one credential. A fake `PullClient` stands in for tests. **Token** — `Credentials::from_env()` mirrors `TrustRoot::from_env()`: (1) `HERDR_GITHUB_TOKEN` env, else (2) a token file under `HERDR_PLUGIN_CONFIG_DIR` (`github.token`). Held in memory only; **never** written to `cards`/any table/body/prompt. *(Named assumption: herdr 0.9.0 has no managed-secrets API — G1 confirmed — so env + config file is the mechanism.)*
- **Bounded sync** — `sync(repo)` is one explicit fetch to completion: no timer, no loop, no scheduler, no auto-dispatch. `list_issues` returns a finite `Vec<IssueFull>` (the real client paginates with a hard cap). Idempotent upsert via the identity key.
- **Visible-conflict rule** — a card is in conflict iff `stored.revision != fetched.updated_at`; the diff is computed over the 7 shared fields (title/body/state/state_reason/labels/assignee/milestone — never `column`/`factory_kind`), stored in `card_conflicts`, and the card flagged `apply-pending`. Resolution is two human actions: **apply** (accept canonical fields + set `revision`, clear conflict) or **defer** (keep the flag; re-surfaces next sync). Never silent overwrite.
- **Scoped repo / default column / pagination / state_reason / retention** *(named assumptions)*: sync targets one repo passed explicitly as `RepoIdentity` (the manifest/`HERDR_PLUGIN_CONTEXT_JSON` wiring is a later slice); default landing `column = "to-do"` (const `DEFAULT_COLUMN`, board-configurable later); list is single-page-capped in VS1; `state_reason` is `NULL` when REST doesn't expose it (GraphQL is a later slice); conflict retention reuses G3's `STALENESS_THRESHOLD` (no new constant).

## Shared contract (frozen first — the seam between the halves)
- **Types (dev-1, `src/outbox/cards.rs`)**: `Card { identity: Identity, url, fields: CanonicalFields, column, factory_kind: FactoryKind, revision, conflict: Conflict, synced_at }`; `CanonicalFields { title, body, state, state_reason: Option, labels: Vec<String>, assignee: Option, milestone: Option }`; `Conflict { None | ApplyPending }`; `CardField { Title | Body | State | StateReason | Labels | Assignee | Milestone }`; `CardFieldDiff { field, old: String, new: String }`.
- **Store ops (dev-1)**: `insert_card(&Card)`; `get_card(&Identity) -> Option<Card>`; `list_cards(owner, repo) -> Vec<Card>`; `touch_card(&Identity, synced_at)`; `record_conflict(&Identity, &[CardFieldDiff], detected_at)`; `get_conflict(&Identity) -> Vec<CardFieldDiff>`; `apply_conflict(&Identity, &CanonicalFields, revision) -> Card`; `defer_conflict(&Identity)`.
- **Client + payload (dev-2)**: `PullClient { list_issues(&RepoIdentity) -> Vec<IssueFull>; fetch_issue(&RepoIdentity, number) -> Option<IssueFull> }`; `IssueFull { number, title, body, state, state_reason, labels, assignee, milestone, updated_at, url }`.
- **Entrypoint (dev-2)**: `sync(store, client, repo) -> SyncSummary { inserted, unchanged, conflicted }`; `apply_changes(store, client, repo, number) -> Card`; `defer_changes(store, identity)`.

dev-1 is self-contained (no dependency on dev-2); dev-2 depends only on dev-1's frozen `outbox::cards` types + ops. **Merge order:** dev-1 merges first (independently testable); dev-2's worktree is cut from a base containing the contract module so it compiles, then rebases onto dev-1 and merges.

## Order of work
1. [contract] Author `src/outbox/cards.rs` types + `Store` method signatures; commit to both worktree bases.
2. [dev-1] `src/outbox/mod.rs`: `mod cards;` + migration v5 (`cards` + `card_conflicts` DDL).
3. [dev-1] Implement the `Store` card ops + label serialization in `src/outbox/cards.rs`.
4. [dev-1] Re-export types in `src/lib.rs`; write `tests/cards_store.rs`; gate (test/clippy/fmt/llvm-cov) in `herdr-board-d1`.
5. [dev-2] `src/sync/client.rs`: `IssueFull`, `PullClient`, `Credentials::from_env()`, `RealGitHubClient` (ureq + serde).
6. [dev-2] `src/sync/conflict.rs`: field-diff computation over the 7 shared fields.
7. [dev-2] `src/sync/mod.rs`: `IssueFull → CanonicalFields` mapping, `sync()` (insert/unchanged/conflict), `apply_changes`, `defer_changes`, `DEFAULT_COLUMN`.
8. [dev-2] `pub mod sync;` + re-exports in `src/lib.rs`; write `tests/sync_lifecycle.rs` + `examples/sync_demo.rs`; gate in `herdr-board-d2`.
9. [coordinator] Rebase dev-2 onto dev-1; run the full-suite gate on the merged tree.

## Validation Strategy
- Unit — `cargo test --locked --lib`: `CardField`/`Conflict`/`CanonicalFields` mapping; label-list serialization round-trip; field-diff equality.
- Integration — `cargo test --locked --test cards_store` (dev-1) and `cargo test --locked --test sync_lifecycle` (dev-2).
- Edge cases (named fixtures): first pull inserts with default `column` + `factory_kind=ordinary` + `conflict=none`; unchanged re-sync is a no-op (bumps `synced_at`, no content change, no conflict); changed `updated_at` → `apply-pending` + `card_conflicts` rows for exactly the drifted fields; identical re-sync after a conflict stays flagged (never silent overwrite); `apply` accepts only the 7 shared fields (assert `column`/`factory_kind` untouched); `defer` keeps the flag; duplicate identity insert → unique-constraint (idempotent); label list with CF hold/stage labels preserved verbatim; empty labels/assignee/milestone; `state_reason` NULL tolerated; restart persistence (close + reopen).
- E2E / smoke — `cargo run --example sync_demo` walks a full sync against a fake `PullClient` + tempdir SQLite, then re-syncs with a changed body and prints the conflict + apply.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/outbox/cards.rs` + `src/sync/`.

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --test cards_store       # store CRUD + idempotent upsert + conflict record/apply/defer green
cargo test --locked --test sync_lifecycle    # insert/unchanged/conflict/apply/defer green
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report >= 90% line coverage on src/outbox/cards.rs + src/sync/
cargo run --example sync_demo                # stdout shows: N inserted, then a changed issue -> conflict with field diff, then apply clears it
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% threshold; the example prints the inserted count, a `conflicted` card with its per-field diff after a re-sync (never silently overwritten), and a cleared conflict after `apply`. Evidence artifacts land in `intent/8-board-plugin/tickets/20-vs1-issue-pull-sync/evidence/`.

## Risks
- **Riskiest step — the revision-gated conflict detection + the apply/defer boundary (dev-2 steps 6–7).** The whole slice's safety is "never silently overwrite": if the revision gate is wrong (e.g. comparing the wrong token, or `apply` touching `column`/`factory_kind`), the board overwrites GitHub-canonical fields silently or clobbers board-local state. Mitigation: `revision` = `updated_at` verbatim (G3), diff over only the 7 shared fields, `apply` explicitly restricted to `CanonicalFields`, and a test asserting `column`/`factory_kind` survive apply.
- **Credentials handling (dev-2 step 5).** The token must never reach SQLite/card content. Mitigation: `Credentials::from_env()` mirrors `TrustRoot::from_env()`, the token is held in the client only, and a test/redaction step confirms no credential string appears in any card/table column.
- **Rejected and why:** a background/timer sync loop (REQUIREMENTS: sync never auto-runs; `autospin=ask` — bounded explicit only); storing the token in the client's persistence or `cards` (constraint — injected + in-memory only); a separate `GitHubClient` trait for read vs publish (one `RealGitHubClient` implements both, one construction point); mapping labels/state to `column` (no auto label↔column equivalence — board-local); `apply` overwriting `column`/`factory_kind` (board-local/board-intent, never GitHub-canonical); a synthetic `card_id` key instead of the identity PK (the issue identity *is* the key — `UNIQUE(owner,repo,number)` as the primary key).
