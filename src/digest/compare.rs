//! Dispatch/acceptance comparison: [`StoredFields`] and [`verify`].

use crate::kind::FactoryKind;

use super::canonical::{CanonicalRequest, Identity};
use super::conflict::{Field, FieldDiff, Mismatch};
use super::hash::{compute_with_version, Digest};
use super::SCHEMA_VERSION;

/// The canonical field values — plus the schema version — persisted atomically
/// with a request's digest.
///
/// This snapshot is the board's record of *what was requested* at persist
/// time. [`verify`] compares a dispatch- or acceptance-time
/// [`CanonicalRequest`] against it, canonicalizing under the **persisted**
/// `schema_version` (version-on-record) rather than the compile-time constant,
/// so records persisted under an older schema still verify under their own
/// layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFields {
    pub schema_version: u64,
    pub identity: Identity,
    pub revision: String,
    pub factory: String,
    pub actor: String,
    pub body: String,
    pub factory_kind: FactoryKind,
}

impl From<&CanonicalRequest> for StoredFields {
    fn from(request: &CanonicalRequest) -> Self {
        Self {
            // A freshly-constructed request is canonicalized under the current
            // schema; records read back from the store carry their own version.
            schema_version: SCHEMA_VERSION,
            identity: request.identity.clone(),
            revision: request.revision.clone(),
            factory: request.factory.clone(),
            actor: request.actor.clone(),
            body: request.body.clone(),
            factory_kind: request.factory_kind,
        }
    }
}

impl StoredFields {
    /// Rebuild the [`CanonicalRequest`] this snapshot was taken from, dropping
    /// the schema version. Used to re-dispatch on reconciliation; the caller
    /// that needs the version keeps `self.schema_version`.
    pub fn to_request(&self) -> CanonicalRequest {
        CanonicalRequest {
            factory_kind: self.factory_kind,
            identity: self.identity.clone(),
            revision: self.revision.clone(),
            factory: self.factory.clone(),
            actor: self.actor.clone(),
            body: self.body.clone(),
        }
    }

    /// Field-level diff between the persisted snapshot and a presented request.
    fn diff_against(&self, presented: &CanonicalRequest) -> Vec<FieldDiff> {
        let mut diffs = Vec::new();

        push_diff(
            &mut diffs,
            Field::FactoryKind,
            self.factory_kind.as_str(),
            presented.factory_kind.as_str(),
        );

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
/// Recomputes the digest over `presented` under `stored_fields.schema_version`
/// and compares it to `stored`. On a match the request is intact and the
/// caller may proceed (`Ok(())`). On any difference it returns a [`Mismatch`]
/// carrying the per-field diff (persisted vs presented) and both digest ids;
/// the caller must **refuse to run** — never recompute-and-overwrite the
/// stored digest.
pub fn verify(
    stored: &Digest,
    stored_fields: &StoredFields,
    presented: &CanonicalRequest,
) -> Result<(), Mismatch> {
    let presented_digest = compute_with_version(presented, stored_fields.schema_version);
    if presented_digest == *stored {
        return Ok(());
    }
    Err(Mismatch {
        field_diffs: stored_fields.diff_against(presented),
        stored_id: stored.digest_id(),
        presented_id: presented_digest.digest_id(),
    })
}
