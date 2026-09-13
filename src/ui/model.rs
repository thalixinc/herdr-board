//! The board model: pure grouping, filtering, and a 2-level epic→task
//! hierarchy over a `Vec<Card>`. No store access, no network.

use std::collections::BTreeMap;

use crate::outbox::Card;

/// The board's columns, board-local and never derived from labels or state.
///
/// The first column matches `crate::sync::DEFAULT_COLUMN`; a card whose
/// `column` is none of these renders in a trailing "uncategorized" column.
pub const BOARD_COLUMNS: &[&str] = &["to-do", "in-progress", "done"];

/// The trailing column name for cards whose `column` is unknown (never dropped).
pub const UNCATEGORIZED: &str = "uncategorized";

/// A reference to a parent epic by issue number, parsed from `Parent epic: <N>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParentRef {
    pub number: u64,
}

/// Whether a card is an epic (`epic` label).
pub fn is_epic(card: &Card) -> bool {
    card.fields.labels.iter().any(|label| label == "epic")
}

/// Parse a `Parent epic: <N>` reference from a card body. Tolerant of spacing,
/// an optional `#`, and trailing whitespace; the marker is case-insensitive.
pub fn parent_of(body: &str) -> Option<ParentRef> {
    for line in body.lines() {
        if let Some(number) = parse_parent_line(line) {
            return Some(ParentRef { number });
        }
    }
    None
}

fn parse_parent_line(line: &str) -> Option<u64> {
    // Collapse all whitespace to single spaces for a tolerant match, then
    // lowercase so the marker is case-insensitive.
    let collapsed: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = collapsed.to_ascii_lowercase();
    let rest = lower.strip_prefix("parent epic")?.trim_start();
    let rest = rest.strip_prefix(':')?.trim_start();
    let number = rest.strip_prefix('#').unwrap_or(rest).trim();
    number.parse::<u64>().ok()
}

/// One renderable row in a column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// An epic (a group head).
    Epic(Card),
    /// A child, indented under its parent epic.
    Child(Card),
    /// An ungrouped card (floats).
    Card(Card),
}

impl Entry {
    /// The card this entry renders.
    pub fn card(&self) -> &Card {
        match self {
            Entry::Epic(card) | Entry::Child(card) | Entry::Card(card) => card,
        }
    }

    /// Whether this entry renders indented (a child).
    pub fn is_indented(&self) -> bool {
        matches!(self, Entry::Child(_))
    }
}

/// A board column: a name plus its cards in render order (epics as group heads,
/// their children indented beneath, ungrouped cards floating).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardColumn {
    pub name: String,
    pub entries: Vec<Entry>,
}

/// The whole board: columns in `BOARD_COLUMNS` order (plus "uncategorized").
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BoardModel {
    pub columns: Vec<BoardColumn>,
}

impl BoardModel {
    /// Build the model: filter `cards`, group by `card.column` in column order
    /// (unknown columns → a trailing "uncategorized" column), and arrange each
    /// column's cards into the 2-level epic→task hierarchy.
    pub fn from_cards(cards: Vec<Card>, filters: &Filters) -> BoardModel {
        let filtered: Vec<Card> = cards
            .into_iter()
            .filter(|card| filters.matches(card))
            .collect();

        let mut known: BTreeMap<&'static str, Vec<Card>> = BTreeMap::new();
        let mut uncategorized: Vec<Card> = Vec::new();
        for card in filtered {
            let column = BOARD_COLUMNS
                .iter()
                .copied()
                .find(|name| *name == card.column.as_str());
            match column {
                Some(name) => known.entry(name).or_default().push(card),
                None => uncategorized.push(card),
            }
        }

        let mut columns = Vec::new();
        for name in BOARD_COLUMNS {
            if let Some(cards) = known.remove(name) {
                columns.push(column_from_cards((*name).to_owned(), cards));
            }
        }
        if !uncategorized.is_empty() {
            columns.push(column_from_cards(UNCATEGORIZED.to_owned(), uncategorized));
        }

        BoardModel { columns }
    }

    /// The number of cards (all entries, all columns).
    pub fn len(&self) -> usize {
        self.columns.iter().map(|column| column.entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The cards in render order (column-major, then column-entry order).
    pub fn cards(&self) -> Vec<&Card> {
        self.columns
            .iter()
            .flat_map(|column| column.entries.iter())
            .map(|entry| entry.card())
            .collect()
    }

    /// The card at flat index `i`, if any.
    pub fn nth(&self, i: usize) -> Option<&Card> {
        self.columns
            .iter()
            .flat_map(|column| column.entries.iter())
            .map(|entry| entry.card())
            .nth(i)
    }
}

fn column_from_cards(name: String, mut cards: Vec<Card>) -> BoardColumn {
    cards.sort_by_key(|card| card.identity.number);

    let mut epics = Vec::new();
    let mut children: BTreeMap<u64, Vec<Card>> = BTreeMap::new();
    let mut floats = Vec::new();

    for card in cards {
        if is_epic(&card) {
            epics.push(card);
        } else if let Some(parent) = parent_of(&card.fields.body) {
            children.entry(parent.number).or_default().push(card);
        } else {
            floats.push(card);
        }
    }

    let mut entries = Vec::new();
    for epic in epics {
        let number = epic.identity.number;
        entries.push(Entry::Epic(epic));
        if let Some(kids) = children.remove(&number) {
            entries.extend(kids.into_iter().map(Entry::Child));
        }
    }
    // Orphaned children (their epic is absent from this column) float too.
    for kids in children.into_values() {
        floats.extend(kids);
    }
    floats.sort_by_key(|card| card.identity.number);
    entries.extend(floats.into_iter().map(Entry::Card));

    BoardColumn { name, entries }
}

/// The session filters, AND-combined. Every active filter must match for a card
/// to be shown.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filters {
    /// A card matches if it carries ANY of these labels (empty = no filter).
    pub labels: Vec<String>,
    /// Exact assignee match.
    pub assignee: Option<String>,
    /// Exact state match (`"open"` / `"closed"`).
    pub state: Option<String>,
    /// Case-insensitive title substring.
    pub title: Option<String>,
}

impl Filters {
    /// Whether `card` passes every active filter.
    pub fn matches(&self, card: &Card) -> bool {
        if !self.labels.is_empty()
            && !self
                .labels
                .iter()
                .any(|label| card.fields.labels.contains(label))
        {
            return false;
        }
        if let Some(assignee) = &self.assignee {
            if card.fields.assignee.as_deref() != Some(assignee.as_str()) {
                return false;
            }
        }
        if let Some(state) = &self.state {
            if card.fields.state != *state {
                return false;
            }
        }
        if let Some(title) = &self.title {
            let title = title.to_ascii_lowercase();
            if !card.fields.title.to_ascii_lowercase().contains(&title) {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{parent_of, BoardModel, Filters, ParentRef, BOARD_COLUMNS};
    use crate::outbox::{CanonicalFields, Card, Conflict};
    use crate::{FactoryKind, Identity};

    fn card(number: u64, column: &str, title: &str) -> Card {
        Card {
            identity: Identity::new("thalixinc", "herdr-board", number),
            url: format!("https://github.com/thalixinc/herdr-board/issues/{number}"),
            fields: CanonicalFields {
                title: title.to_owned(),
                body: String::new(),
                state: "open".to_owned(),
                state_reason: None,
                labels: vec![],
                assignee: None,
                milestone: None,
            },
            column: column.to_owned(),
            factory_kind: FactoryKind::Ordinary,
            revision: "rev".to_owned(),
            conflict: Conflict::None,
            synced_at: 0,
        }
    }

    fn with_labels(mut card: Card, labels: &[&str]) -> Card {
        card.fields.labels = labels.iter().map(|l| l.to_string()).collect();
        card
    }

    fn with_body(mut card: Card, body: &str) -> Card {
        card.fields.body = body.to_owned();
        card
    }

    #[test]
    fn is_epic_matches_epic_label() {
        assert!(super::is_epic(&with_labels(
            card(1, "to-do", "e"),
            &["epic"]
        )));
        assert!(!super::is_epic(&with_labels(
            card(2, "to-do", "t"),
            &["bug", "epic-adjacent"]
        )));
    }

    #[test]
    fn parent_of_parses_variants() {
        assert_eq!(parent_of("Parent epic: 42"), Some(ParentRef { number: 42 }));
        assert_eq!(parent_of("Parent epic:#7"), Some(ParentRef { number: 7 }));
        assert_eq!(
            parent_of("  parent   epic :   #9   "),
            Some(ParentRef { number: 9 })
        );
        assert_eq!(
            parent_of("some text\nParent epic: 3"),
            Some(ParentRef { number: 3 })
        );
        assert_eq!(parent_of("no parent here"), None);
        assert_eq!(parent_of("Parent epic:"), None);
    }

    #[test]
    fn from_cards_groups_in_column_order() {
        let cards = vec![
            card(1, "done", "a"),
            card(2, "to-do", "b"),
            card(3, "in-progress", "c"),
            card(4, "to-do", "d"),
        ];
        let model = BoardModel::from_cards(cards, &Filters::default());
        let names: Vec<&str> = model.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["to-do", "in-progress", "done"]);
        // Column order + number order within a column.
        assert_eq!(model.columns[0].entries.len(), 2);
        assert_eq!(model.columns[0].entries[0].card().identity.number, 2);
        assert_eq!(model.columns[0].entries[1].card().identity.number, 4);
    }

    #[test]
    fn unknown_column_falls_to_uncategorized() {
        let cards = vec![card(1, "to-do", "a"), card(2, "blocked", "b")];
        let model = BoardModel::from_cards(cards, &Filters::default());
        let names: Vec<&str> = model.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["to-do", "uncategorized"]);
        assert_eq!(model.columns[1].entries[0].card().identity.number, 2);
    }

    #[test]
    fn hierarchy_groups_epic_children_and_floats() {
        let cards = vec![
            with_labels(card(1, "to-do", "epic"), &["epic"]),
            with_body(card(2, "to-do", "child"), "Parent epic: 1"),
            card(3, "to-do", "float"),
            card(4, "to-do", "orphan"), // parent epic absent → floats
        ];
        let model = BoardModel::from_cards(cards, &Filters::default());
        let entries = &model.columns[0].entries;
        assert_eq!(entries.len(), 4);
        assert!(matches!(entries[0], super::Entry::Epic(_)));
        assert!(matches!(entries[1], super::Entry::Child(_)));
        assert!(matches!(entries[2], super::Entry::Card(_)));
        assert!(matches!(entries[3], super::Entry::Card(_)));
    }

    #[test]
    fn filters_and_combine() {
        let card = with_labels(
            {
                let mut c = card(1, "to-do", "Fix the board sync");
                c.fields.assignee = Some("founder".to_owned());
                c
            },
            &["bug"],
        );
        assert!(Filters::default().matches(&card));

        // Each single filter passes.
        let label = Filters {
            labels: vec!["bug".to_owned()],
            ..Filters::default()
        };
        assert!(label.matches(&card));
        let assignee = Filters {
            assignee: Some("founder".to_owned()),
            ..Filters::default()
        };
        assert!(assignee.matches(&card));
        let title = Filters {
            title: Some("BOARD SYNC".to_owned()),
            ..Filters::default()
        };
        assert!(title.matches(&card));

        // AND-combined: a single non-matching filter excludes.
        let and = Filters {
            labels: vec!["bug".to_owned()],
            assignee: Some("someone-else".to_owned()),
            ..Filters::default()
        };
        assert!(!and.matches(&card));

        // Label any-match.
        let any = Filters {
            labels: vec!["enhancement".to_owned(), "bug".to_owned()],
            ..Filters::default()
        };
        assert!(any.matches(&card));
    }

    #[test]
    fn board_columns_match_default() {
        assert_eq!(BOARD_COLUMNS[0], crate::sync::DEFAULT_COLUMN);
    }
}
