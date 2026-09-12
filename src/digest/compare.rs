//! Dispatch/acceptance comparison: [`StoredFields`] and [`verify`].

use super::canonical::{CanonicalRequest, Identity};
use super::conflict::{Field, FieldDiff, Mismatch};
use super::hash::{compute, Digest};

/// The five canonical field values persisted atomically with a request's
/// digest.
///
/// This snapshot is the board's record of *what was requested* at persist
/// time. [`verify`] compares a dispatch- or acceptance-time
/// [`CanonicalRequest`] against it. The values here are already canonical:
/// `identity` re-canonicalizes via [`Identity::canonical`], while `revision`,
/// `factory`, `actor`, and `body` are stored verbatim (never re-encoded,
/// trimmed, or reordered).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFields {
    pub identity: Identity,
    pub revision: String,
    pub factory: String,
    pub actor: String,
    pub body: String,
}

impl From<&CanonicalRequest> for StoredFields {
    fn from(request: &CanonicalRequest) -> Self {
        Self {
            identity: request.identity.clone(),
            revision: request.revision.clone(),
            factory: request.factory.clone(),
            actor: request.actor.clone(),
            body: request.body.clone(),
        }
    }
}

impl StoredFields {
    /// Field-level diff between the persisted snapshot and a presented request.
    fn diff_against(&self, presented: &CanonicalRequest) -> Vec<FieldDiff> {
        let mut diffs = Vec::new();

        // Identity is canonicalized on both sides: a case-only difference is
        // not a drift, matching how the digest itself canonicalizes identity.
        let stored_identity = self.identity.canonical();
        let presented_identity = presented.identity.canonical();
        if stored_identity != presented_identity {
            diffs.push(FieldDiff {
                field: Field::Identity,
                old: stored_identity,
                new: presented_identity,
            });
        }

        push_diff(
            &mut diffs,
            Field::Revision,
            &self.revision,
            &presented.revision,
        );
        push_diff(
            &mut diffs,
            Field::Factory,
            &self.factory,
            &presented.factory,
        );
        push_diff(&mut diffs, Field::Actor, &self.actor, &presented.actor);
        push_diff(&mut diffs, Field::Body, &self.body, &presented.body);

        diffs
    }
}

fn push_diff(diffs: &mut Vec<FieldDiff>, field: Field, old: &str, new: &str) {
    if old != new {
        diffs.push(FieldDiff {
            field,
            old: old.to_owned(),
            new: new.to_owned(),
        });
    }
}

/// Verify that a dispatch- or acceptance-time request matches the stored
/// digest.
///
/// Recomputes the digest over `presented` and compares it to `stored`. On a
/// match the request is intact and the caller may proceed (`Ok(())`). On any
/// difference it returns a [`Mismatch`] carrying the per-field diff (persisted
/// vs presented) and both digest ids; the caller must **refuse to run** —
/// never recompute-and-overwrite the stored digest.
pub fn verify(
    stored: &Digest,
    stored_fields: &StoredFields,
    presented: &CanonicalRequest,
) -> Result<(), Mismatch> {
    let presented_digest = compute(presented);
    if presented_digest == *stored {
        return Ok(());
    }
    Err(Mismatch {
        field_diffs: stored_fields.diff_against(presented),
        stored_id: stored.digest_id(),
        presented_id: presented_digest.digest_id(),
    })
}
