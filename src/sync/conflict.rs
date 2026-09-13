//! Conflict mapping: field-level drift between the board's copy and the
//! fetched issue, over the seven GitHub-canonical fields only.

use crate::outbox::{CanonicalFields, CardField, CardFieldDiff};

/// Compute the drifted fields between the board's copy (`old`) and the fetched
/// issue (`new`), in canonical field order.
///
/// Only the seven GitHub-canonical fields are compared — `column` and
/// `factory_kind` are board-local and never part of a conflict.
pub fn field_diffs(old: &CanonicalFields, new: &CanonicalFields) -> Vec<CardFieldDiff> {
    let mut diffs = Vec::new();

    if old.title != new.title {
        diffs.push(CardFieldDiff {
            field: CardField::Title,
            old: old.title.clone(),
            new: new.title.clone(),
        });
    }
    if old.body != new.body {
        diffs.push(CardFieldDiff {
            field: CardField::Body,
            old: old.body.clone(),
            new: new.body.clone(),
        });
    }
    if old.state != new.state {
        diffs.push(CardFieldDiff {
            field: CardField::State,
            old: old.state.clone(),
            new: new.state.clone(),
        });
    }
    if old.state_reason != new.state_reason {
        diffs.push(CardFieldDiff {
            field: CardField::StateReason,
            old: render_option(&old.state_reason),
            new: render_option(&new.state_reason),
        });
    }
    if old.labels != new.labels {
        diffs.push(CardFieldDiff {
            field: CardField::Labels,
            old: render_labels(&old.labels),
            new: render_labels(&new.labels),
        });
    }
    if old.assignee != new.assignee {
        diffs.push(CardFieldDiff {
            field: CardField::Assignee,
            old: render_option(&old.assignee),
            new: render_option(&new.assignee),
        });
    }
    if old.milestone != new.milestone {
        diffs.push(CardFieldDiff {
            field: CardField::Milestone,
            old: render_option(&old.milestone),
            new: render_option(&new.milestone),
        });
    }

    diffs
}

fn render_option(value: &Option<String>) -> String {
    value.as_deref().unwrap_or("").to_owned()
}

fn render_labels(labels: &[String]) -> String {
    labels.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbox::CardField;

    fn fields() -> CanonicalFields {
        CanonicalFields {
            title: "Title".to_owned(),
            body: "Body".to_owned(),
            state: "open".to_owned(),
            state_reason: None,
            labels: vec![],
            assignee: None,
            milestone: None,
        }
    }

    #[test]
    fn no_drift_produces_no_diffs() {
        let f = fields();
        assert!(field_diffs(&f, &f).is_empty());
    }

    #[test]
    fn each_field_diff_reported_in_canonical_order() {
        let old = fields();
        let new = CanonicalFields {
            title: "New title".to_owned(),
            body: "New body".to_owned(),
            state: "closed".to_owned(),
            state_reason: Some("completed".to_owned()),
            labels: vec!["bug".to_owned()],
            assignee: Some("alice".to_owned()),
            milestone: Some("v1".to_owned()),
        };

        let diffs = field_diffs(&old, &new);
        let fields: Vec<CardField> = diffs.iter().map(|d| d.field).collect();
        assert_eq!(
            fields,
            vec![
                CardField::Title,
                CardField::Body,
                CardField::State,
                CardField::StateReason,
                CardField::Labels,
                CardField::Assignee,
                CardField::Milestone,
            ]
        );
    }

    #[test]
    fn option_and_labels_rendered_as_strings() {
        let old = fields();
        let new = CanonicalFields {
            labels: vec!["bug".to_owned(), "enhancement".to_owned()],
            assignee: Some("alice".to_owned()),
            ..fields()
        };

        let diffs = field_diffs(&old, &new);
        assert_eq!(diffs.len(), 2);
        assert_eq!(diffs[0].field, CardField::Labels);
        assert_eq!(diffs[0].old, "");
        assert_eq!(diffs[0].new, "bug,enhancement");
        assert_eq!(diffs[1].field, CardField::Assignee);
        assert_eq!(diffs[1].old, "");
        assert_eq!(diffs[1].new, "alice");
    }
}
