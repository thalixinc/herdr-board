//! The board application state: the store, the scoped repo, the derived model,
//! the session filters, the selection, a status line, and the pre-loaded
//! conflict diffs (so the renderer stays pure).

use std::collections::HashMap;

use crate::create::RepoIdentity;
use crate::digest::Identity;
use crate::outbox::{Card, CardFieldDiff, Conflict, Store};

use super::model::{BoardModel, Filters};

/// The six board dimensions a filter can constrain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterDimension {
    Assignee,
    Label,
    State,
    Epic,
    Milestone,
    Repository,
    Title,
}

impl FilterDimension {
    /// The prompt label for the dimension.
    pub fn name(&self) -> &'static str {
        match self {
            FilterDimension::Assignee => "assignee",
            FilterDimension::Label => "label",
            FilterDimension::State => "state",
            FilterDimension::Epic => "epic",
            FilterDimension::Milestone => "milestone",
            FilterDimension::Repository => "repository",
            FilterDimension::Title => "title",
        }
    }

    /// The next dimension in the cycling order.
    pub fn next(&self) -> Self {
        match self {
            FilterDimension::Assignee => FilterDimension::Label,
            FilterDimension::Label => FilterDimension::State,
            FilterDimension::State => FilterDimension::Epic,
            FilterDimension::Epic => FilterDimension::Milestone,
            FilterDimension::Milestone => FilterDimension::Repository,
            FilterDimension::Repository => FilterDimension::Title,
            FilterDimension::Title => FilterDimension::Assignee,
        }
    }

    /// Set this dimension's filter from a non-empty `buffer` (empty clears it).
    pub fn apply_to(&self, filters: &mut Filters, buffer: &str) {
        match self {
            FilterDimension::Assignee => filters.assignee = nonempty(buffer),
            FilterDimension::Label => {
                if let Some(label) = nonempty(buffer) {
                    if !filters.labels.contains(&label) {
                        filters.labels.push(label);
                    }
                }
            }
            FilterDimension::State => filters.state = nonempty(buffer),
            FilterDimension::Epic => {
                filters.epic = buffer.trim().parse::<u64>().ok();
            }
            FilterDimension::Milestone => filters.milestone = nonempty(buffer),
            FilterDimension::Repository => filters.repository = nonempty(buffer),
            FilterDimension::Title => filters.title = nonempty(buffer),
        }
    }

    /// Clear this dimension's filter (turn it back into "no filter").
    pub fn clear_from(&self, filters: &mut Filters) {
        match self {
            FilterDimension::Assignee => filters.assignee = None,
            FilterDimension::Label => filters.labels.clear(),
            FilterDimension::State => filters.state = None,
            FilterDimension::Epic => filters.epic = None,
            FilterDimension::Milestone => filters.milestone = None,
            FilterDimension::Repository => filters.repository = None,
            FilterDimension::Title => filters.title = None,
        }
    }
}

/// The interactive filter input bar: off, or editing one dimension's buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterInput {
    Off,
    Active {
        dimension: FilterDimension,
        buffer: String,
    },
}

impl FilterInput {
    /// Whether the filter bar is currently open.
    pub fn is_active(&self) -> bool {
        matches!(self, FilterInput::Active { .. })
    }
}

/// The active dimension's buffer as a trimmed option: `None` when empty.
fn nonempty(buffer: &str) -> Option<String> {
    let trimmed = buffer.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// The board application. `model` is recomputed from the store on
/// [`App::reload`]; `conflicts` holds the per-field diffs of `apply-pending`
/// cards, keyed by identity, so the renderer never touches the store.
pub struct App {
    pub store: Store,
    pub repo: RepoIdentity,
    pub model: BoardModel,
    pub filters: Filters,
    pub focus: usize,
    pub status: Option<String>,
    pub conflicts: HashMap<Identity, Vec<CardFieldDiff>>,
    pub filter_input: FilterInput,
}

impl App {
    pub fn new(store: Store, repo: RepoIdentity) -> App {
        let mut app = App {
            store,
            repo,
            model: BoardModel::default(),
            filters: Filters::default(),
            focus: 0,
            status: None,
            conflicts: HashMap::new(),
            filter_input: FilterInput::Off,
        };
        app.reload();
        app
    }

    /// Recompute the model from the store and pre-load every `apply-pending`
    /// card's field diffs.
    pub fn reload(&mut self) {
        let cards = self
            .store
            .list_cards(&self.repo.owner, &self.repo.repo)
            .unwrap_or_default();
        let mut conflicts = HashMap::new();
        for card in &cards {
            if card.conflict == Conflict::ApplyPending {
                if let Ok(diffs) = self.store.get_conflict(&card.identity) {
                    conflicts.insert(card.identity.clone(), diffs);
                }
            }
        }
        self.model = BoardModel::from_cards(cards, &self.filters);
        self.conflicts = conflicts;
    }

    /// Move the selection to the next card (clamped to the last).
    pub fn select_next(&mut self) {
        let len = self.model.len();
        if len == 0 {
            return;
        }
        self.focus = (self.focus + 1).min(len - 1);
    }

    /// Move the selection to the previous card (clamped to the first).
    pub fn select_prev(&mut self) {
        self.focus = self.focus.saturating_sub(1);
    }

    /// The focused card, if any.
    pub fn focused(&self) -> Option<&Card> {
        self.model.nth(self.focus)
    }

    /// Replace the filters and recompute the model.
    pub fn set_filter(&mut self, filters: Filters) {
        self.filters = filters;
        self.reload();
    }

    /// Open the filter bar, defaulting to the assignee dimension.
    pub fn open_filter(&mut self) {
        self.filter_input = FilterInput::Active {
            dimension: FilterDimension::Assignee,
            buffer: String::new(),
        };
    }

    /// Cycle the active dimension to the next one, preserving the buffer.
    pub fn cycle_filter_dimension(&mut self) {
        if let FilterInput::Active { dimension, .. } = &mut self.filter_input {
            *dimension = dimension.next();
        }
    }

    /// Append a character to the filter buffer.
    pub fn push_filter_char(&mut self, c: char) {
        if let FilterInput::Active { buffer, .. } = &mut self.filter_input {
            buffer.push(c);
        }
    }

    /// Remove the last character from the filter buffer.
    pub fn pop_filter_char(&mut self) {
        if let FilterInput::Active { buffer, .. } = &mut self.filter_input {
            buffer.pop();
        }
    }

    /// Apply the buffer to the active dimension (an empty buffer clears it),
    /// then recompute the model. The bar stays open for further edits.
    pub fn apply_filter(&mut self) {
        let (dimension, buffer) = match &self.filter_input {
            FilterInput::Active {
                dimension, buffer, ..
            } => (*dimension, buffer.clone()),
            FilterInput::Off => return,
        };
        let mut filters = self.filters.clone();
        dimension.apply_to(&mut filters, &buffer);
        self.set_filter(filters);
    }

    /// Clear the active dimension's filter and recompute the model.
    pub fn clear_filter_dimension(&mut self) {
        let dimension = match &self.filter_input {
            FilterInput::Active { dimension, .. } => *dimension,
            FilterInput::Off => return,
        };
        let mut filters = self.filters.clone();
        dimension.clear_from(&mut filters);
        self.set_filter(filters);
    }

    /// Close the filter bar without applying the buffer.
    pub fn close_filter(&mut self) {
        self.filter_input = FilterInput::Off;
    }
}
