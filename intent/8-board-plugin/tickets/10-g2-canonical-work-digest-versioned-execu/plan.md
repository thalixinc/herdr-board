# Plan: ticket #10 — G2 — Canonical work digest (versioned execution-input digest)
Epic: #8. Date: 2026-09-12. Status: ready.

## Files that change
- `Cargo.toml` — add `sha2 = "0.10"` (SHA-256 only; hex display is hand-rolled, no `hex`/`serde`/store deps yet)
- `src/lib.rs` — add `pub mod digest;` + re-export the public API (`Digest`, `DigestId`, `CanonicalRequest`, `compute`, `verify`, `Mismatch`)
- `src/digest/mod.rs` (new, dev-1) — module root: `pub const SCHEMA_VERSION: u64 = 1;`, module decls, re-exports, crate-level doc for the framing/version contract
- `src/digest/canonical.rs` (new, dev-1) — `Identity` (owner/repo lowercased, `number` as decimal ASCII) + `CanonicalRequest` (the five fields in fixed order) + length-prefix framing with the version frame first
- `src/digest/hash.rs` (new, dev-1) — `Digest([u8; 32])` (source of truth), `DigestId([u8; 8])` (16-hex display-only prefix), `sha256(framed_bytes) -> Digest`
- `src/digest/compare.rs` (new, dev-2) — `StoredFields` snapshot + `verify()` used at **both** dispatch and acceptance; returns `Ok(())` or `Mismatch`
- `src/digest/conflict.rs` (new, dev-2) — `Mismatch { field_diffs, stored_id, presented_id }`, `FieldDiff`, `Field`, and the `Refusal` outcome (factory refuses to run; never auto-recompute)
- `tests/digest_lifecycle.rs` (new, dev-2) — integration test of persist → dispatch → acceptance → drift → refusal
- `examples/digest_conflict.rs` (new, dev-2) — runnable smoke that walks a drift scenario and prints the conflict

`src/main.rs` is **unchanged**: the digest is a pure library concern. The visible-conflict *TUI widget* that renders `Mismatch` lands with the board vertical slice, not G2.

## Dev split

The work splits cleanly at the `compute` boundary. **Contract both sides share** (written first, in `src/digest/mod.rs`): `CanonicalRequest` (five canonical fields), `compute(&CanonicalRequest) -> Digest`, `Digest`/`DigestId` types, and `SCHEMA_VERSION`.

- **dev-1 = digest core + framing** (`Cargo.toml`, `src/lib.rs`, `src/digest/mod.rs`, `canonical.rs`, `hash.rs`). Independently implementable: given a `CanonicalRequest`, produce a deterministic `Digest` + human `DigestId`. Verified by unit tests (determinism, order, framing, version, identity, verbatim, digest id).
- **dev-2 = dispatch/acceptance comparison + mismatch policy** (`compare.rs`, `conflict.rs`, `tests/digest_lifecycle.rs`, `examples/digest_conflict.rs`). Independently implementable: given a stored digest + stored field snapshot + recomputed input, produce `Ok` or a `Mismatch` with a field-level diff and a refusal outcome. Depends only on dev-1's frozen `compute`/`CanonicalRequest` signature.

## Order of work
1. [dev-1] Add `sha2 = "0.10"` to `Cargo.toml`; `cargo build --locked` resolves it.
2. [dev-1] Write `src/digest/mod.rs`: `SCHEMA_VERSION = 1`, module decls, re-exports, and the framing/version contract as doc comment.
3. [dev-1] Implement `src/digest/canonical.rs`: `Identity::canonical()` (lowercase owner/repo; `#`; decimal-ASCII number, no leading zeros); `CanonicalRequest { identity, revision, factory, actor, body }`; length-prefix framing (`[u64 big-endian length][raw bytes]`, version frame first, then the five fields in fixed order).
4. [dev-1] Implement `src/digest/hash.rs`: SHA-256 over the framed bytes → `Digest`; `DigestId` = first 8 bytes as 16-hex.
5. [dev-1] Wire `pub mod digest;` + re-exports into `src/lib.rs`.
6. [dev-1] Unit tests (determinism, reorder sensitivity, version-inside-hash, framing non-ambiguity, identity case/leading-zero normalization, verbatim body bytes, digest-id prefix) + pin **one golden vector** (documented fixture → exact 64-hex digest).
7. [dev-2] Implement `src/digest/compare.rs`: `StoredFields` (the five canonical values persisted alongside the digest) + `verify(stored: &Digest, stored_fields: &StoredFields, presented: &CanonicalRequest) -> Result<(), Mismatch>`.
8. [dev-2] Implement `src/digest/conflict.rs`: `Field` enum (Identity/Revision/Factory/Actor/Body), `FieldDiff { field, old, new }`, `Mismatch { field_diffs, stored_id, presented_id }`, `Refusal` (refuse-to-run; no auto-recompute, no auto-win).
9. [dev-2] Write `tests/digest_lifecycle.rs`: full lifecycle — persist computes, dispatch+acceptance verify clean; body/edit drift at dispatch and revision drift at acceptance each yield a field-level `Mismatch`; assert the stored digest is **never** rewritten after a failed verify.
10. [dev-2] Write `examples/digest_conflict.rs`; run the full gate (test/clippy/fmt/llvm-cov/example).

## Validation Strategy
- Unit — `cargo test --locked --lib digest` covers: same-input→same-digest (determinism); swapped `revision`/`body` → different digest (order); `SCHEMA_VERSION` bump → different digest with identical fields (version hashed); framing is unambiguous for `["ab","c"]` vs `["a","bc"]`; identity `Owner/Repo#42` ≡ `owner/repo#42` and `#0042` ≡ `#42`; body trailing newline/whitespace preserved byte-for-byte.
- Integration — `cargo test --locked --test digest_lifecycle` exercises persist→dispatch→acceptance and the two drift points.
- Edge cases (named fixtures in the suites above) — empty `body`/`revision`/`actor`; Unicode + emoji + embedded newlines in `body`; a field containing the bytes of a fake length prefix (framing must not be fooled); a ~1 MiB body (framing stays O(n), no quadratic re-copy).
- E2E / smoke — `cargo run --example digest_conflict` walks persist→dispatch→acceptance, then a drifted re-dispatch, and prints the digest id, the per-field diff, and `REFUSED`.
- Lint — `cargo clippy --locked --all-targets -- -D warnings` and `cargo fmt --all -- --check`, both exit 0.
- Coverage — `cargo llvm-cov --all-features --fail-under-lines 90`; target ≥ 90% line coverage on `src/digest/`.

## Proof
Definition of done — all commands below run green in this checkout:
```sh
cargo test --locked                          # test result: ok; 0 failed (unit + integration)
cargo test --locked --lib digest             # digest unit suite green (determinism/order/framing/version/golden vector)
cargo test --locked --test digest_lifecycle  # lifecycle integration green (clean path + both drift points + no-silent-repair)
cargo clippy --locked --all-targets -- -D warnings   # exit 0, zero warnings
cargo fmt --all -- --check                   # exit 0
cargo llvm-cov --all-features --fail-under-lines 90  # report shows >= 90% line coverage on src/digest/
cargo run --example digest_conflict          # stdout shows: 16-hex digest id, a FieldDiff (old vs new), two digest ids, "REFUSED: factory will not run"
```
Expected: `cargo test`/`clippy`/`fmt` exit 0; `llvm-cov` passes the 90% line threshold; the example prints a stable digest id and, for the drifted input, a field-level mismatch with `stored_id ≠ presented_id` and the `REFUSED` refusal (never `Ok`). Evidence artifacts land in `intent/8-board-plugin/tickets/10-g2-canonical-work-digest-versioned-execu/evidence/`.

## Risks
- **Riskiest step — the framing/versioning determinism contract (step 3 + the golden vector).** A subtly ambiguous frame (e.g. relying on a delimiter a value could contain) or an omitted version frame silently makes digests collide or drift across processes, voiding the entire integrity guarantee — and because the version is hashed, a framing bug cannot be fixed in-place without a schema bump. Mitigation: length-prefix framing (never delimiters), version as the first frame, and a pinned golden-vector test so any framing/version change fails loudly; the only sanctioned fix is a `SCHEMA_VERSION` bump (§2).
- **`revision` semantics are open (design §5.1).** If G3 wires a revision token that does not change on content edits, body drift between persist and acceptance will not be caught. G2 is deliberately agnostic (stores the token verbatim); this is a G3 dependency, not a G2 blocker — flagged to G3, non-blocking.
- **Rejected and why:** delimiter-separated encoding (a field may contain the delimiter — newline/Unicode — so it is ambiguous); recompute-to-repair on mismatch (destroys the exact "what runs == what was persisted" guarantee); hashing cf-queue's serialized bytes (violates the §4 field-contract boundary — the board owns only its canonical representation); hand-rolling SHA-256 (boring over clever — use the `sha2` crate); timestamps/salts/nonces in the digest (breaks same-input→same-digest determinism).
