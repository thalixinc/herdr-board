//! The board application state: the store, the scoped repo, the derived model,
//! the session filters, the selection, a status line, and the pre-loaded
//! conflict diffs (so the renderer stays pure).

use std::collections::HashMap;

use crate::create::RepoIdentity;
use crate::digest::Identity;
use crate::outbox::{Card, CardFieldDiff, Conflict, Store};

use super::model::{BoardModel, Filters};

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
}
