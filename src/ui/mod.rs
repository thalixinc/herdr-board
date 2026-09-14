//! The board TUI: the pure board model, the app state, and the renderer.

mod actions;
mod app;
mod event;
mod model;
mod render;

pub use actions::{dispatch, Action, Deps, KeyMap, UiError};
pub use app::{App, FilterDimension, FilterInput};
pub use event::run_loop;
pub use model::{
    is_epic, parent_of, BoardColumn, BoardModel, Entry, Filters, ParentRef, BOARD_COLUMNS,
    UNCATEGORIZED,
};
pub use render::render;
