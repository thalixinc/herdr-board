# Plan: ticket #23 — VS2 — card→issue push
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change

### Shared contract (authored first; committed to the base of *both* worktrees)
- `src/create/github.rs` (dev-1) — `IssuePatch` + `UpdateResult` types + the `GitHubClient::update_issue` method *signature* both halves bind to

### dev-1 — publish/update write seam (worktree `/Users/chrismckenna/development/herdr-board-d1`, branch `vs/vs2-dev1`)
- `src/create/github.rs` — add `IssuePatch`, `UpdateResult`, and `update_issue` to the `GitHubClient` trait (one publish trait; no new `PublishClient`)
- `src/sync/client.rs` — `impl GitHubClient for RealGitHubClient` (`create_issue`, `search_issues`, `get_issue`, `update_issue` via `ureq` + `serde`); one construction point `RealGitHubClient::new(Credentials)`, token in memory only
- `src/lib.rs` — re-export `IssuePatch`, `UpdateResult`
- `tests/update_seam.rs` (new) — fake client + `IssuePatch` tri-state semantics + `UpdateResult` three-way

### dev-2 — pending-write store + push orchestration + draft wiring (worktree `/Users/chrismckenna/development/herdr-board-d2`, branch `vs/vs2-dev2`)
- `src/outbox/pending.rs` (new) — `PendingWrite`, `WriteOutcome`, and the `Store` pending-write ops (`record_write` (merge), `get_pending_write`, `finalize_write`, `discard_write`)
- `src/outbox/mod.rs` — `mod pending;` + **migration v6** (`pending_writes` + partial unique index) + re-exports
- `src/push/mod.rs` (new) — `publish_draft` (draft→intent→`create::issue`→link card on `created`), `push_changes` (fetch → base-revision gate → update/conflict), `apply_push`, `discard_push`
- `src/push/conflict.rs` (new) — push-direction field diff (board pending values vs GitHub current)
- `src/lib.rs` — `pub mod push;` + re-exports
- `tests/push_lifecycle.rs` (new) — draft publish → linked card; edit → pending write → push equal → `written`; push different → conflict → apply/discard
- `examples/push_demo.rs` (new) — runnable smoke

`src/main.rs` is **unchanged**; the "issue changed; apply?" prompt renders with the TUI vertical slice. G4's `create_intents`/`GitHubClient`/`create::{issue,link,cancel,candidates}` and VS1's `outbox::cards` + `sync::client` are consumed as-is, not modified (dev-1 only *extends* `GitHubClient` and *adds* a real impl).

## Resolved decisions (design left these open — resolved here, or named assumptions)
- **Publish client** — extend the existing `GitHubClient` trait (not a new `PublishClient`): add `update_issue(repo, number, patch) -> UpdateResult`. `RealGitHubClient` (VS1) now implements both `PullClient` and `GitHubClient` — one construction point, one token, in-memory only, never persisted. `IssuePatch` uses single-`Option` for title/body/labels (replace-on-set; labels replace the whole set) and double-`Option` for assignee/milestone (`None` = leave, `Some(None)` = clear, `Some(Some(v))` = set). `UpdateResult::Updated { updated_at }` carries the PATCH response's new `updated_at` (design q5: trust the response; the *next* push's gate re-verifies — no extra fetch).
- **Pending-write model** — a `pending_writes` table (migration v6) keyed `(owner, repo, number)`. **At most one unresolved write per card** via a partial unique index over non-terminal outcomes (`pending | failed | uncertain`); `written | discarded` are terminal. A new edit **merges** field-by-field (last-wins) into the existing unresolved row; `base_revision` stays **pinned** to the original base (design q3 — the gate answers "did GitHub change since I started editing", so it must not advance). Store tri-state for assignee/milestone as `NULL` (unchanged) / `''` (clear) / value (set).
- **Visible-conflict gate** — push compares `fetch_issue(…).updated_at` (current) vs `base_revision`: equal → `update_issue` → `written`; different → surface a field diff between the board's pending values and GitHub's current values (push direction), block until **apply** (human-confirmed board-wins PATCH) or **discard** (drop the write + re-sync to GitHub). Never silent overwrite.
- **Retry / idempotency** — create dedup = G4's marker + block-auto-replay (unchanged); update dedup = value-idempotency + revision gate. A `failed`/`uncertain` update is re-attempted **explicitly** (never auto) with the same values, re-running the gate each time (design q4).
- **Field scope** — `title`/`body`/`labels`/`assignee`/`milestone` are pushed; `state`/`state_reason` are **not** (open/close is a separate explicit action; column moves never close — design q1); `column`/`factory_kind` are **never** pushed. CF hold/stage labels are never silently stripped: a removal appears in the diff and is human-confirmed (design q2: warn + confirm, not hard-refuse).
- **Convergence with VS1** — a card in VS1 `Conflict::ApplyPending` must resolve the pull conflict first; `push_changes` refuses while the pull conflict is unresolved (design q6), so edits never build on an already-stale base.
- **Draft wiring** *(named assumption)* — `publish_draft` builds the `CreateIntent` from the draft's `title`/`body`/`factory_kind`; `repo`, `labels`, `assignee` are supplied at publish time (the full board card model carrying labels/assignee is a later slice). On `Created(number)` it re-fetches via `PullClient::fetch_issue` to seed the card's `revision` + canonical fields; if that fetch fails, the card is inserted with `revision=""` and the next sync backfills it.

## Shared contract (frozen first — the seam between the halves)
- **Types (dev-1)**: `IssuePatch { title: Option<String>, body: Option<String>, labels: Option<Vec<String>>, assignee: Option<Option<String>>, milestone: Option<Option<String>> }`; `UpdateResult { Updated { updated_at: String } | Failed(String) | Uncertain(String) }`.
- **Method (dev-1)**: `GitHubClient::update_issue(&self, repo: &RepoIdentity, number: u64, patch: &IssuePatch) -> UpdateResult`.
- **Store ops (dev-2, internal to dev-2's deliverable but named here for clarity)**: `record_write(&PendingWrite)`, `get_pending_write(&Identity) -> Option<PendingWrite>`, `finalize_write(&Identity, WriteOutcome, updated_at: Option<&str>)`, `discard_write(&Identity)`.

dev-1 is self-contained (trait + types + real HTTP + fake; no store, no flow). dev-2 depends only on dev-1's frozen `IssuePatch`/`UpdateResult`/`update_issue` plus already-frozen VS1 `PullClient`/`cards` ops and G4's outbox. **Merge order:** dev-1 merges first; dev-2's worktree is cut from a base containing the contract and rebases onto dev-1.

## Dev split (worktrees)
- **dev-1 → `/Users/chrismckenna/development/herdr-board-d1` (branch `vs/vs2-dev1`)**: `src/create/github.rs`, `src/sync/client.rs`, `src/lib.rs`, `tests/update_seam.rs` — the write seam (trait extension + real `GitHubClient` impl + fake). Independently testable (no store, no network in tests).
- **dev-2 → `/Users/chrismckenna/development/herdr-board-d2` (branch `vs/vs2-dev2`)**: `src/outbox/pending.rs`, `src/outbox/mod.rs` (migration v6), `src/push/{mod,conflict}.rs`, `src/lib.rs`, `tests/push_lifecycle.rs`, `examples/push_demo.rs` — the pending-write store + push orchestration + draft wiring + conflict resolution. Consumes dev-1's contract + VS1/G4 frozen surfaces.

## Order of work
1. [contract] Author `IssuePatch` + `UpdateResult` + `update_issue` signature in `src/create/github.rs`; commit to both worktree bases.
2. [dev-1] Implement `update_issue` + `IssuePatch`/`UpdateResult`; `impl GitHubClient for RealGitHubClient` (create/search/get/update via ureq+serde).
3. [dev-1] Re-export types in `src/lib.rs`; write `tests/update_seam.rs`; gate in `herdr-board-d1`.
4. [dev-2] `src/outbox/mod.rs`: `mod pending;` + migration v6 (`pending_writes` + partial unique index).
5. [dev-2] `src/outbox/pending.rs`: `PendingWrite`/`WriteOutcome` + store ops (merge-on-record, finalize, discard).
6. [dev-2] `src/push/conflict.rs`: push-direction field diff over the 7 shared fields.
7. [dev-2] `src/push/mod.rs`: `publish_draft` (draft→intent→create→link), `push_changes` (fetch → gate → update/conflict), `apply_push`, `discard_push`.
8. [dev-2] `pub mod push;` + re-exports in `src/lib.rs`; write `tests/push_lifecycle.rs` + `examples/push_demo.rs`; gate in `herdr-board-d2`.
9. [coordinator] Rebase dev-2 onto dev-1; run the full-suite gate on the merged tree.

## Validation Strategy
- Unit — `cargo test --locked --lib`: `IssuePatch` tri-state (assignee `Some(None)` clears, `None` leaves, `Some(Some)` sets; labels replace-set); `UpdateResult` three-way; `WriteOutcome::is_terminal`.
- Integration — `cargo test --locked --test update_seam` (dev-1) and `cargo test --locked --test push_lifecycle` (dev-2).
- Edge cases (named fixtures): draft publish → `created` → linked card (identity + `revision` from re-fetch); edit records a `pending` write (inert, no GitHub call); second edit merges field-by-field, `base_revision` unchanged; push with equal revision → `written` + `revision` advanced; push with drifted revision → conflict + diff (never PATCH); `apply_push` PATCHes board values over GitHub (human-confirmed) and updates `revision`; `discard_push` drops the write and re-syncs; `uncertain` update re-attempted only on the next explicit push with the gate re-run; push on an `ApplyPending` card refused; CF hold/stage label removal appears in the diff (never silently stripped); `column`/`factory_kind`/`state` never in the pending write or the PATCH.
- E2E / smoke — `cargo run --example push_demo` walks draft → publish → linked card → edit → push → (drifted) conflict → apply, against a fake client + tempdir SQLite.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/create/github.rs` (dev-1) + `src/outbox/pending.rs` + `src/push/`.

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --test update_seam       # IssuePatch tri-state + UpdateResult three-way green
cargo test --locked --test push_lifecycle    # publish/link + pending-write merge + gate + apply/discard green
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report >= 90% line coverage on src/create/github.rs + src/outbox/pending.rs + src/push/
cargo run --example push_demo                # stdout shows: draft published -> card linked, edit -> pending write, drifted push -> conflict diff, apply -> written + new revision
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% threshold; the example prints a linked card after publish, a `pending` write on edit (no GitHub call), a `conflict` with field diff on a drifted push (never silently overwriting), and a `written` outcome with the advanced `revision` after `apply`. Evidence artifacts land in `intent/8-board-plugin/tickets/23-vs2-card-push/evidence/`.

## Risks
- **Riskiest step — the base-revision gate + apply/discard boundary (dev-2 steps 6–7).** The slice's safety is "never silently overwrite": if the gate compares the wrong token or `apply_push` touches fields outside the 7 shared ones (or `column`/`factory_kind`/`state`), the board clobbers a concurrent GitHub edit or board-local state. Mitigation: `base_revision` = the card's `revision` at edit time (pinned), compare against `fetch_issue().updated_at`, `IssuePatch` restricted to the 7 fields, and tests asserting `column`/`factory_kind`/`state` survive every path.
- **The `IssuePatch` tri-state encoding (dev-1 step 2).** Getting "clear vs leave-unchanged" wrong for assignee/milestone (or "replace vs append" for labels) silently drops or fails to clear a field on GitHub. Mitigation: double-`Option` semantics pinned by unit tests (Some(None) clears; None leaves; Some(Some) sets) and the `''` DB sentinel round-trip test.
- **Rejected and why:** a new `PublishClient` trait (G4's `GitHubClient` already is the publish seam — extend it, one trait); auto re-attempt of `failed`/`uncertain` updates (sync never auto-writes — always explicit); a create-style marker for updates (the issue number is already known; value-idempotency + the revision gate is the dedup); pushing `state`/`state_reason` from board edits (open/close is a separate explicit action; column moves never close); pushing `column`/`factory_kind` (board-local/intent, never GitHub-canonical); advancing `base_revision` on merge (loses the "changed since I started" signal); hard-refusing CF label removal (surface in the diff + human-confirm, matching the visible-conflict philosophy).
