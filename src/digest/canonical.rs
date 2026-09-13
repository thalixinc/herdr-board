//! Canonical identity, request shape, and length-prefix framing.

use crate::kind::FactoryKind;

/// The stable identity of an issue: `owner/repo#number`.
///
/// `owner` and `repo` are ASCII-lowercased; `number` is rendered as decimal
/// ASCII with no leading zeros. The canonical string form is
/// `owner/repo#number` (see [`Identity::canonical`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity {
    pub owner: String,
    pub repo: String,
    pub number: u64,
}

impl Identity {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>, number: u64) -> Self {
        Self {
            owner: owner.into(),
            repo: repo.into(),
            number,
        }
    }

    /// The canonical `owner/repo#number` form: `owner`/`repo` lowercased,
    /// `number` as decimal ASCII with no leading zeros.
    pub fn canonical(&self) -> String {
        format!(
            "{}/{}#{}",
            self.owner.to_lowercase(),
            self.repo.to_lowercase(),
            self.number
        )
    }
}

/// The six canonical fields of a factory request, in the fixed order used for
/// framing. Field order is part of the digest contract: a reorder is a schema
/// bump.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalRequest {
    /// The card's factory-kind discriminator — the first data field (a
    /// discriminator reads first).
    pub factory_kind: FactoryKind,
    pub identity: Identity,
    pub revision: String,
    pub factory: String,
    pub actor: String,
    pub body: String,
}

impl CanonicalRequest {
    /// Framing with an explicit version. The version selects the field layout:
    /// v1 predates `factory_kind` in the digest (five data fields); v2 inserts
    /// it as the first data field.
    pub(crate) fn framed_bytes_with_version(&self, version: u64) -> Vec<u8> {
        let version_ascii = version.to_string();
        let identity = self.identity.canonical();

        let frames: Vec<&[u8]> = if version <= 1 {
            vec![
                version_ascii.as_bytes(),
                identity.as_bytes(),
                self.revision.as_bytes(),
                self.factory.as_bytes(),
                self.actor.as_bytes(),
                self.body.as_bytes(),
            ]
        } else {
            vec![
                version_ascii.as_bytes(),
                self.factory_kind.as_str().as_bytes(),
                identity.as_bytes(),
                self.revision.as_bytes(),
                self.factory.as_bytes(),
                self.actor.as_bytes(),
                self.body.as_bytes(),
            ]
        };

        // Single allocation: framing is O(total bytes), never quadratic.
        let capacity: usize = frames.iter().map(|f| 8 + f.len()).sum();
        let mut out = Vec::with_capacity(capacity);
        for field in frames {
            out.extend_from_slice(&(field.len() as u64).to_be_bytes());
            out.extend_from_slice(field);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{CanonicalRequest, Identity};
    use crate::kind::FactoryKind;

    /// Encode a single frame as `[length: u64 big-endian][raw bytes]` — the
    /// same framing the production path applies to each field.
    fn frame(field: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + field.len());
        out.extend_from_slice(&(field.len() as u64).to_be_bytes());
        out.extend_from_slice(field);
        out
    }

    fn request() -> CanonicalRequest {
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
    fn identity_lowercases_owner_and_repo() {
        let id = Identity::new("ThalixInc", "Herdr-Board", 42);
        assert_eq!(id.canonical(), "thalixinc/herdr-board#42");
    }

    #[test]
    fn identity_number_has_no_leading_zeros() {
        // A single-digit number must not be zero-padded.
        assert_eq!(Identity::new("o", "r", 7).canonical(), "o/r#7");

        // A number arriving as a zero-padded decimal string normalizes to the
        // same canonical form as the bare integer.
        let raw = "0042".parse::<u64>().expect("valid decimal ASCII");
        assert_eq!(Identity::new("o", "r", raw).canonical(), "o/r#42");
    }

    #[test]
    fn framing_disambiguates_adjacent_fields() {
        // "ab" + "c" and "a" + "bc" are byte-identical when naively concatenated…
        assert_eq!(
            [&b"ab"[..], &b"c"[..]].concat(),
            [&b"a"[..], &b"bc"[..]].concat()
        );

        // …but their length-prefixed framings differ (same total length, different bytes).
        let left = [frame(b"ab"), frame(b"c")].concat();
        let right = [frame(b"a"), frame(b"bc")].concat();
        assert_eq!(left.len(), right.len());
        assert_ne!(left, right, "length-prefix framing must not be ambiguous");
    }

    #[test]
    fn framing_changes_with_version() {
        let req = request();
        assert_ne!(
            req.framed_bytes_with_version(1),
            req.framed_bytes_with_version(2),
            "the version frame must alter the framed bytes"
        );
    }
}
