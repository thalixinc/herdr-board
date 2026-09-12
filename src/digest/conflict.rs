//! Field-level conflict types: what drifted, and the refusal outcome.

use std::fmt;

use super::hash::DigestId;

/// The six canonical fields a work digest covers, in framing order.
///
/// The order here matches the digest's fixed framing order (factory-kind,
/// identity, revision, factory, actor, body). It is informational for conflict
/// display; it is not itself hashed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    FactoryKind,
    Identity,
    Revision,
    Factory,
    Actor,
    Body,
}

impl Field {
    /// The field's stable, human-facing name.
    pub fn as_str(&self) -> &'static str {
        match self {
            Field::FactoryKind => "factory-kind",
            Field::Identity => "identity",
            Field::Revision => "revision",
            Field::Factory => "factory",
            Field::Actor => "actor",
            Field::Body => "body",
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One field that drifted between what was persisted and what was presented.
///
/// `old` is the value persisted alongside the stored digest (identity in its
/// canonical `owner/repo#number` form, the other four verbatim); `new` is the
/// value presented at dispatch/acceptance time. `old`/`new` are *rendered*
/// values for the human conflict view — the digest comparison itself is over
/// the full framed bytes, never these strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDiff {
    pub field: Field,
    pub old: String,
    pub new: String,
}

impl fmt::Display for FieldDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "  {}: {:?} -> {:?}", self.field, self.old, self.new)
    }
}

/// A field-level conflict: the presented input's digest does not match the
/// stored digest.
///
/// `field_diffs` is the per-field old-vs-new diff between the stored field
/// snapshot and the presented request; `stored_id` and `presented_id` are the
/// two display-only digest ids shown to the human resolver. An empty
/// `field_diffs` means no single field drifted — the stored digest itself does
/// not correspond to the stored fields (a schema-version change or a corrupt
/// persisted record) — and the two ids still differ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    pub field_diffs: Vec<FieldDiff>,
    pub stored_id: DigestId,
    pub presented_id: DigestId,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stored digest id:   {}", self.stored_id)?;
        f.write_str("\n")?;
        write!(f, "presented digest id: {}", self.presented_id)?;
        if self.field_diffs.is_empty() {
            f.write_str(
                "\n  (no field drifted; the stored digest does not match the stored fields)",
            )?;
        } else {
            for diff in &self.field_diffs {
                f.write_str("\n")?;
                write!(f, "{}", diff)?;
            }
        }
        Ok(())
    }
}

/// The refusal outcome: the factory must not run.
///
/// Produced from a [`Mismatch`] at the factory boundary. It is deliberately
/// not recoverable by recomputation — there is **no** auto-recompute and
/// **no** auto-win. Resolution is human: re-confirm on the current input (a
/// fresh persist, hence a new digest) or cancel the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub mismatch: Mismatch,
}

impl From<Mismatch> for Refusal {
    fn from(mismatch: Mismatch) -> Self {
        Self { mismatch }
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("REFUSED: factory will not run")
    }
}
