//! SHA-256 over the framed bytes: [`Digest`], [`DigestId`], and [`compute`].

use std::fmt;

use sha2::{Digest as _, Sha256};

use super::canonical::CanonicalRequest;
use super::SCHEMA_VERSION;

/// The full 32-byte SHA-256 digest — the source of truth for comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Digest(pub [u8; 32]);

impl Digest {
    /// The underlying 32 bytes.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The human-facing 16-hex prefix (first eight bytes) — display-only,
    /// never compared.
    pub fn digest_id(&self) -> DigestId {
        let mut id = [0u8; 8];
        id.copy_from_slice(&self.0[..8]);
        DigestId(id)
    }

    /// The full digest as 64 lowercase hex characters.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// The first eight bytes of a [`Digest`], rendered as 16 hex characters.
/// Display-only shorthand for humans to spot drift; never compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DigestId(pub [u8; 8]);

impl DigestId {
    /// The prefix as 16 lowercase hex characters.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }
}

impl fmt::Display for DigestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Compute the canonical work digest under the current schema: SHA-256 over
/// the version-first length-prefix framing of the six canonical fields.
pub fn compute(request: &CanonicalRequest) -> Digest {
    compute_with_version(request, SCHEMA_VERSION)
}

/// Compute the digest under an explicit schema version, selecting that
/// version's field layout. Used by [`compute`] and by version-on-record
/// verification: [`super::verify`] canonicalizes under the persisted record's
/// version rather than the compile-time constant.
pub fn compute_with_version(request: &CanonicalRequest, version: u64) -> Digest {
    digest_bytes(&request.framed_bytes_with_version(version))
}

fn digest_bytes(bytes: &[u8]) -> Digest {
    let hash = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&hash);
    Digest(out)
}

const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX_LOWER[(b >> 4) as usize] as char);
        s.push(HEX_LOWER[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::{compute, compute_with_version, CanonicalRequest, Digest};
    use crate::digest::{Identity, SCHEMA_VERSION};
    use crate::kind::FactoryKind;

    /// The pinned golden fixture. Any change to framing, field order, or
    /// version that alters the digest inputs must fail this test — the only
    /// sanctioned fix is a `SCHEMA_VERSION` bump (which is itself a new vector).
    fn golden_request() -> CanonicalRequest {
        CanonicalRequest {
            factory_kind: FactoryKind::FactoryRequest,
            identity: Identity::new("ThalixInc", "herdr-board", 42),
            revision: "r1".into(),
            factory: "coordinator".into(),
            actor: "founder".into(),
            body: "build the board\nwith care".into(),
        }
    }

    #[test]
    fn golden_vector() {
        // Fixture (version 2):
        //   factory_kind = factory-request
        //   identity     = thalixinc/herdr-board#42
        //   revision     = r1
        //   factory      = coordinator
        //   actor        = founder
        //   body         = "build the board\nwith care"
        let digest = compute(&golden_request());
        assert_eq!(
            digest.to_hex(),
            "9570b03cf8d9a02b93c2bc363283b7e6ed4479b955c86f63808b32eca470c8d6"
        );
        assert_eq!(digest.digest_id().to_hex(), "9570b03cf8d9a02b");
    }

    #[test]
    fn same_input_same_digest() {
        let a = compute(&golden_request());
        let b = compute(&golden_request());
        assert_eq!(a, b);
        assert_eq!(a.as_bytes(), b.as_bytes());
    }

    #[test]
    fn reorder_changes_digest() {
        let mut swapped = golden_request();
        std::mem::swap(&mut swapped.revision, &mut swapped.body);
        assert_ne!(
            compute(&golden_request()),
            compute(&swapped),
            "field order is part of the digest contract"
        );
    }

    #[test]
    fn version_is_inside_the_hash() {
        // Same six fields, different schema version → different digest.
        let v1 = compute_with_version(&golden_request(), 1);
        let v2 = compute_with_version(&golden_request(), 2);
        assert_ne!(v1, v2, "a version bump must change the digest");
        assert_eq!(v2, compute(&golden_request()));
        assert_eq!(v2, compute_with_version(&golden_request(), SCHEMA_VERSION));
    }

    #[test]
    fn body_is_verbatim() {
        let base = golden_request();

        let mut trailing_newline = base.clone();
        trailing_newline.body = "build the board\nwith care\n".into();
        assert_ne!(compute(&base), compute(&trailing_newline));

        let mut trailing_space = base.clone();
        trailing_space.body = "build the board\nwith care ".into();
        assert_ne!(compute(&base), compute(&trailing_space));

        // Unicode + emoji + embedded newline are preserved byte-for-byte.
        let mut unicode = base.clone();
        unicode.body = "héllo\nworld 🌍\r\n".into();
        assert_ne!(compute(&base), compute(&unicode));
    }

    #[test]
    fn digest_id_is_16_hex_prefix() {
        let digest: Digest = compute(&golden_request());
        let id = digest.digest_id();

        assert_eq!(id.0, digest.0[..8], "DigestId is the first eight bytes");
        let id_hex = id.to_hex();
        let full_hex = digest.to_hex();
        assert_eq!(id_hex, &full_hex[..16]);
        assert_eq!(id_hex.len(), 16);
        assert!(
            id_hex
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "DigestId is lowercase hex"
        );
    }
}
