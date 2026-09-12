//! The factory-kind discriminator: whether a card is a plain board card or a
//! "Process with factory" card. It is board-local intent, fixed at draft
//! creation, and inert — sync never starts work from it.
//!
//! One canonical definition and serialization; G4's `create_intents.factory_kind`
//! column and G2's digest (via the canonical field set) both consume this enum.

use std::fmt;
use std::str::FromStr;

/// A card-level discriminator with exactly two variants today.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FactoryKind {
    /// A plain board card: sync-display only, never hands to a factory.
    #[default]
    Ordinary,
    /// A card that will hand a request to coordinator/planner via the G3
    /// bridge. Which factory is G2's `factory` field, resolved at the explicit
    /// action — not here.
    FactoryRequest,
}

impl FactoryKind {
    /// The canonical wire/serialized form.
    pub fn as_str(&self) -> &'static str {
        match self {
            FactoryKind::Ordinary => "ordinary",
            FactoryKind::FactoryRequest => "factory-request",
        }
    }

    /// Whether this card hands a request to a factory.
    pub fn is_factory_request(&self) -> bool {
        matches!(self, FactoryKind::FactoryRequest)
    }
}

impl fmt::Display for FactoryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error returned when a string is not a known [`FactoryKind`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidFactoryKind(pub String);

impl fmt::Display for InvalidFactoryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid factory kind {:?}", self.0)
    }
}

impl std::error::Error for InvalidFactoryKind {}

impl FromStr for FactoryKind {
    type Err = InvalidFactoryKind;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ordinary" => Ok(FactoryKind::Ordinary),
            "factory-request" => Ok(FactoryKind::FactoryRequest),
            other => Err(InvalidFactoryKind(other.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FactoryKind;

    #[test]
    fn canonical_serialization_round_trips() {
        for kind in [FactoryKind::Ordinary, FactoryKind::FactoryRequest] {
            let s = kind.as_str();
            let parsed: FactoryKind = s.parse().expect("known kind parses");
            assert_eq!(parsed, kind);
            assert_eq!(parsed.to_string(), s);
        }
    }

    #[test]
    fn default_is_ordinary() {
        assert_eq!(FactoryKind::default(), FactoryKind::Ordinary);
        assert!(!FactoryKind::Ordinary.is_factory_request());
        assert!(FactoryKind::FactoryRequest.is_factory_request());
    }

    #[test]
    fn unknown_kind_is_rejected() {
        assert!("not-a-kind".parse::<FactoryKind>().is_err());
    }
}
