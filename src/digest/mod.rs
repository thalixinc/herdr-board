//! Canonical work digest — a versioned, deterministic fingerprint of the
//! execution input a board persists for a "Process with factory" request.
//!
//! # Contract
//!
//! The digest is computed over a **framed byte string** built from a
//! [`CanonicalRequest`]'s five canonical fields. The same logical input always
//! produces the same [`Digest`]; any change to the field set, field order, or a
//! field's canonicalization rule is a schema bump ([`SCHEMA_VERSION`]), because
//! the version is hashed too.
//!
//! # Framing (version 1)
//!
//! The digest input is the concatenation of six frames, in this fixed order:
//!
//! ```text
//! frame(version) || frame(identity) || frame(revision)
//!     || frame(factory) || frame(actor) || frame(body)
//! ```
//!
//! Each frame is `[length: u64 big-endian][raw bytes]`, where `length` is the
//! byte length of the raw bytes that follow. There are **no delimiters**: a
//! length prefix is unambiguous even when a value contains newlines, Unicode,
//! or bytes that happen to look like a length prefix.
//!
//! Canonical forms of the six frames:
//!
//! | # | Frame    | Canonical form |
//! |---|----------|----------------|
//! | 0 | version  | [`SCHEMA_VERSION`] as decimal ASCII (no leading zeros) |
//! | 1 | identity | `owner/repo#number` — `owner`/`repo` ASCII-lowercased, `number` decimal ASCII (no leading zeros) |
//! | 2 | revision | opaque token, stored verbatim |
//! | 3 | factory  | target factory identifier, verbatim |
//! | 4 | actor    | trusted actor identity, verbatim |
//! | 5 | body     | request body as persisted, verbatim UTF-8 bytes |
//!
//! The digest is **SHA-256** over this framed byte string ([`compute`]). The
//! full 32-byte [`Digest`] is the source of truth for comparison; its first
//! eight bytes, rendered as 16 lowercase hex characters, form the human-facing
//! [`DigestId`] — display-only, never compared.
//!
//! # Determinism rules
//!
//! - Fixed field order (above).
//! - Length-prefix framing, never delimiters.
//! - Values hashed verbatim; identity is the sole canonicalization.
//! - The schema version is inside the hash (first frame).
//! - No timestamps, salts, or nonces participate.
//!
//! A digest is computed once at persist time and compared at dispatch and
//! acceptance; it is **never recomputed to repair a mismatch** — a mismatch is
//! a conflict to surface, not an error to silently fix.

/// The digest schema version. This is the first frame of the hashed input, so
/// it is *inside* the digest: bumping it changes every digest even when the
/// five field values are byte-identical.
///
/// Bump whenever the field set, field order, or any field's canonicalization
/// rule changes.
pub const SCHEMA_VERSION: u64 = 1;

mod canonical;
mod compare;
mod conflict;
mod hash;

pub use canonical::{CanonicalRequest, Identity};
pub use compare::{verify, StoredFields};
pub use conflict::{Field, FieldDiff, Mismatch, Refusal};
pub use hash::{compute, Digest, DigestId};
