//! Push-direction conflict mapping: board pending values vs GitHub current,
//! over the five pushable fields (title/body/labels/assignee/milestone).
//! `state`/`state_reason` are never pushed (open/close is a separate action);
//! `column`/`factory_kind` are board-local and never pushed.

use crate::outbox::{CardField, CardFieldDiff, PendingWrite};
use crate::sync::IssueFull;

/// Compute the fields where the board's pending values differ from GitHub's
/// current values — i.e. what `apply_push` would overwrite on GitHub.
pub fn field_diffs(pending: &PendingWrite, current: &IssueFull) -> Vec<CardFieldDiff> {
    let mut diffs = Vec::new();

    if let Some(title) = &pending.title {
        if title != &current.title {
            diffs.push(CardFieldDiff {
                field: CardField::Title,
                old: current.title.clone(),
                new: title.clone(),
            });
        }
    }
    if let Some(body) = &pending.body {
        if body != &current.body {
            diffs.push(CardFieldDiff {
                field: CardField::Body,
                old: current.body.clone(),
                new: body.clone(),
            });
        }
    }
    if let Some(labels) = &pending.labels {
        if labels != &current.labels {
            diffs.push(CardFieldDiff {
                field: CardField::Labels,
                old: current.labels.join(","),
                new: labels.join(","),
            });
        }
    }
    if let Some(assignee) = &pending.assignee {
        let new = match assignee {
            None => String::new(),
            Some(v) => v.clone(),
        };
        let old = current.assignee.clone().unwrap_or_default();
        if old != new {
            diffs.push(CardFieldDiff {
                field: CardField::Assignee,
                old,
                new,
            });
        }
    }
    if let Some(milestone) = &pending.milestone {
        let new = match milestone {
            None => String::new(),
            Some(v) => v.clone(),
        };
        let old = current.milestone.clone().unwrap_or_default();
        if old != new {
            diffs.push(CardFieldDiff {
                field: CardField::Milestone,
                old,
                new,
            });
        }
    }

    diffs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::Identity;

    fn pending() -> PendingWrite {
        PendingWrite {
            identity: Identity::new("o", "r", 1),
            base_revision: "r1".to_owned(),
            title: Some("Board title".to_owned()),
            body: Some("Board body".to_owned()),
            labels: Some(vec!["bug".to_owned(), "enhancement".to_owned()]),
            assignee: Some(Some("alice".to_owned())),
            milestone: Some(None),
            outcome: crate::outbox::WriteOutcome::Pending,
        }
    }

    fn current() -> IssueFull {
        IssueFull {
            number: 1,
            title: "GH title".to_owned(),
            body: "GH body".to_owned(),
            state: "open".to_owned(),
            state_reason: None,
            labels: vec!["bug".to_owned()],
            assignee: Some("bob".to_owned()),
            milestone: Some("v1".to_owned()),
            updated_at: "r2".to_owned(),
            url: "u".to_owned(),
        }
    }

    #[test]
    fn reports_only_the_pushed_fields_that_differ() {
        let diffs = field_diffs(&pending(), &current());
        let fields: Vec<CardField> = diffs.iter().map(|d| d.field).collect();
        // title, body, labels, assignee, milestone all differ; state/state_reason
        // are never pushed.
        assert_eq!(
            fields,
            vec![
                CardField::Title,
                CardField::Body,
                CardField::Labels,
                CardField::Assignee,
                CardField::Milestone,
            ]
        );
    }

    #[test]
    fn unedited_fields_are_not_reported() {
        let mut p = pending();
        p.body = None; // not pushing body — GitHub's change there is not our conflict
        let diffs = field_diffs(&p, &current());
        let fields: Vec<CardField> = diffs.iter().map(|d| d.field).collect();
        assert!(!fields.contains(&CardField::Body));
    }

    #[test]
    fn matching_values_are_not_reported() {
        let mut p = pending();
        p.title = Some("GH title".to_owned());
        p.assignee = Some(Some("bob".to_owned()));
        p.milestone = Some(Some("v1".to_owned()));
        p.labels = Some(vec!["bug".to_owned()]);
        let diffs = field_diffs(&p, &current());
        let fields: Vec<CardField> = diffs.iter().map(|d| d.field).collect();
        assert_eq!(fields, vec![CardField::Body]); // only body still differs
    }
}
