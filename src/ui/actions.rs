//! Board actions and keybindings: the thin view over the frozen data layer.
//! Every action is explicit; nothing auto-fires.

use std::fmt;

use crossterm::event::{KeyCode, KeyModifiers};

use crate::create::{GitHubClient, RepoIdentity};
use crate::digest::Identity;
use crate::factory::{process_with_factory, ProcessError};
use crate::outbox::StoreError;
use crate::push::{apply_push, discard_push, publish_draft, PushError};
use crate::receiver::HandoffTransport;
use crate::sync::{apply_changes, defer_changes, sync, PullClient, SyncError};

use super::{App, Filters, BOARD_COLUMNS};

/// A user action. All explicit; nothing auto-fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `s` — pull-sync the scoped repo.
    Sync,
    /// `a` — apply the focused card's pull conflict.
    ApplyPull,
    /// `d` — defer the focused card's pull conflict.
    DeferPull,
    /// `p` — publish the oldest unpublished draft.
    Publish,
    /// `Shift+A` — apply the focused card's push conflict (board wins).
    ApplyPush,
    /// `Shift+D` — discard the focused card's push conflict.
    DiscardPush,
    /// `f` — process the focused card with the factory.
    ProcessFactory,
    /// `←` / `h` — move the focused card one column left.
    MoveLeft,
    /// `→` / `l` — move the focused card one column right.
    MoveRight,
    /// `j` — select the next card.
    SelectNext,
    /// `k` — select the previous card.
    SelectPrev,
    /// `/` — clear the filters.
    ClearFilters,
    /// `q` / `esc` / `ctrl+c` — quit.
    Quit,
}

/// Maps a key event to an [`Action`].
pub struct KeyMap;

impl KeyMap {
    /// Resolve a key press (code + modifiers) to an action, if bound.
    pub fn resolve(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<Action> {
        match (modifiers, code) {
            (KeyModifiers::NONE, KeyCode::Char('s')) => Some(Action::Sync),
            (KeyModifiers::NONE, KeyCode::Char('a')) => Some(Action::ApplyPull),
            (KeyModifiers::NONE, KeyCode::Char('d')) => Some(Action::DeferPull),
            (KeyModifiers::NONE, KeyCode::Char('p')) => Some(Action::Publish),
            (KeyModifiers::SHIFT, KeyCode::Char('A')) => Some(Action::ApplyPush),
            (KeyModifiers::SHIFT, KeyCode::Char('D')) => Some(Action::DiscardPush),
            (KeyModifiers::NONE, KeyCode::Char('f')) => Some(Action::ProcessFactory),
            (KeyModifiers::NONE, KeyCode::Left) => Some(Action::MoveLeft),
            (KeyModifiers::NONE, KeyCode::Char('h')) => Some(Action::MoveLeft),
            (KeyModifiers::NONE, KeyCode::Right) => Some(Action::MoveRight),
            (KeyModifiers::NONE, KeyCode::Char('l')) => Some(Action::MoveRight),
            (KeyModifiers::NONE, KeyCode::Char('j')) => Some(Action::SelectNext),
            (KeyModifiers::NONE, KeyCode::Char('k')) => Some(Action::SelectPrev),
            (KeyModifiers::NONE, KeyCode::Char('/')) => Some(Action::ClearFilters),
            (KeyModifiers::NONE, KeyCode::Char('q')) => Some(Action::Quit),
            (KeyModifiers::NONE, KeyCode::Esc) => Some(Action::Quit),
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => Some(Action::Quit),
            _ => None,
        }
    }
}

/// The data-layer dependencies the actions call. `C` is the (single) client —
/// it implements both [`PullClient`] and [`GitHubClient`]; `T` is the handoff
/// transport.
pub struct Deps<'a, C, T> {
    pub pull: &'a C,
    pub github: &'a C,
    pub transport: &'a T,
    pub repo: &'a RepoIdentity,
}

/// A dispatch failure.
#[derive(Debug)]
pub enum UiError {
    Store(StoreError),
    Sync(SyncError),
    Push(PushError),
    Process(ProcessError),
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiError::Store(e) => write!(f, "store error: {e}"),
            UiError::Sync(e) => write!(f, "sync error: {e}"),
            UiError::Push(e) => write!(f, "push error: {e}"),
            UiError::Process(e) => write!(f, "process error: {e}"),
        }
    }
}

impl std::error::Error for UiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            UiError::Store(e) => Some(e),
            UiError::Sync(e) => Some(e),
            UiError::Push(e) => Some(e),
            UiError::Process(e) => Some(e),
        }
    }
}

impl From<StoreError> for UiError {
    fn from(e: StoreError) -> Self {
        UiError::Store(e)
    }
}

impl From<SyncError> for UiError {
    fn from(e: SyncError) -> Self {
        UiError::Sync(e)
    }
}

impl From<PushError> for UiError {
    fn from(e: PushError) -> Self {
        UiError::Push(e)
    }
}

impl From<ProcessError> for UiError {
    fn from(e: ProcessError) -> Self {
        UiError::Process(e)
    }
}

/// The default target factory for the `f` action (the chooser is a later slice).
const DEFAULT_FACTORY: &str = "coordinator";

/// Run one action against the board, then reload the model.
///
/// `Quit` is a no-op here (the event loop intercepts it before dispatch).
pub fn dispatch<C, T>(app: &mut App, action: Action, deps: &Deps<C, T>) -> Result<(), UiError>
where
    C: PullClient + GitHubClient,
    T: HandoffTransport,
{
    if matches!(action, Action::Quit) {
        return Ok(());
    }
    let result = run_action(app, action, deps);
    app.reload();
    result
}

fn run_action<C, T>(app: &mut App, action: Action, deps: &Deps<C, T>) -> Result<(), UiError>
where
    C: PullClient + GitHubClient,
    T: HandoffTransport,
{
    match action {
        Action::Sync => {
            let summary = sync(&app.store, deps.pull, deps.repo).map_err(UiError::Sync)?;
            app.status = Some(format!(
                "sync: {} inserted, {} unchanged, {} conflicted",
                summary.inserted, summary.unchanged, summary.conflicted
            ));
        }
        Action::ApplyPull => {
            let Some(identity) = focused_identity(app) else {
                return no_card(app);
            };
            apply_changes(&app.store, deps.pull, deps.repo, identity.number)
                .map_err(UiError::Sync)?;
            app.status = Some(format!("applied pull changes to #{}", identity.number));
        }
        Action::DeferPull => {
            let Some(identity) = focused_identity(app) else {
                return no_card(app);
            };
            defer_changes(&app.store, &identity).map_err(UiError::Sync)?;
            app.status = Some(format!("deferred pull changes on #{}", identity.number));
        }
        Action::Publish => {
            let Some(draft) = unpublished_draft(app)? else {
                return no_draft(app);
            };
            publish_draft(&app.store, deps.pull, &draft, deps.repo, &[], None)
                .map_err(UiError::Push)?;
            app.status = Some(format!("published draft {}", draft.title));
        }
        Action::ApplyPush => {
            let Some(identity) = focused_identity(app) else {
                return no_card(app);
            };
            apply_push(&app.store, deps.github, &identity).map_err(UiError::Push)?;
            app.status = Some(format!("applied push changes to #{}", identity.number));
        }
        Action::DiscardPush => {
            let Some(identity) = focused_identity(app) else {
                return no_card(app);
            };
            discard_push(&app.store, deps.pull, &identity).map_err(UiError::Push)?;
            app.status = Some(format!("discarded push changes on #{}", identity.number));
        }
        Action::ProcessFactory => {
            let identity = focused_identity(app);
            let draft = unpublished_draft(app)?;
            match (identity, draft) {
                (Some(identity), Some(draft)) => {
                    process_with_factory(
                        &app.store,
                        deps.transport,
                        &draft.draft_id,
                        &identity,
                        DEFAULT_FACTORY,
                    )
                    .map_err(UiError::Process)?;
                    app.status = Some(format!(
                        "processed #{} with factory {DEFAULT_FACTORY}",
                        identity.number
                    ));
                }
                (Some(_), None) => app.status = Some("no draft to process".to_owned()),
                (None, _) => app.status = Some("no card focused".to_owned()),
            }
        }
        Action::MoveLeft | Action::MoveRight => {
            let delta = if matches!(action, Action::MoveLeft) {
                -1
            } else {
                1
            };
            let Some((identity, current)) = app
                .focused()
                .map(|card| (card.identity.clone(), card.column.clone()))
            else {
                return no_card(app);
            };
            let next = shift_column(&current, delta);
            if next != current {
                app.store
                    .set_column(&identity, &next)
                    .map_err(UiError::Store)?;
                app.status = Some(format!("moved #{} to {next}", identity.number));
            }
        }
        Action::SelectNext => app.select_next(),
        Action::SelectPrev => app.select_prev(),
        Action::ClearFilters => {
            app.set_filter(Filters::default());
            app.status = Some("filters cleared".to_owned());
        }
        Action::Quit => {}
    }
    Ok(())
}

fn focused_identity(app: &App) -> Option<Identity> {
    app.focused().map(|card| card.identity.clone())
}

fn unpublished_draft(app: &App) -> Result<Option<crate::Draft>, UiError> {
    Ok(app
        .store
        .list_drafts()
        .map_err(UiError::Store)?
        .into_iter()
        .find(|draft| draft.published_at.is_none()))
}

fn no_card(app: &mut App) -> Result<(), UiError> {
    app.status = Some("no card focused".to_owned());
    Ok(())
}

fn no_draft(app: &mut App) -> Result<(), UiError> {
    app.status = Some("no draft to publish".to_owned());
    Ok(())
}

/// Shift a known column left or right within [`BOARD_COLUMNS`]; unknown columns
/// are left unchanged.
fn shift_column(current: &str, delta: i32) -> String {
    let Some(index) = BOARD_COLUMNS.iter().position(|column| *column == current) else {
        return current.to_owned();
    };
    let next = (index as i32 + delta).clamp(0, BOARD_COLUMNS.len() as i32 - 1) as usize;
    BOARD_COLUMNS[next].to_owned()
}
