//! The board TUI: the pure board model, the app state, and the renderer.

mod app;
mod model;
mod render;

pub use app::App;
pub use model::{
    is_epic, parent_of, BoardColumn, BoardModel, Entry, Filters, ParentRef, BOARD_COLUMNS,
    UNCATEGORIZED,
};
pub use render::render;

use ratatui::style::{Color, Modifier, Style};

/// The board title style.
pub(crate) const TITLE_STYLE: Style = Style::new().add_modifier(Modifier::BOLD);

/// The "issue changed; apply?" conflict marker style.
pub(crate) const CONFLICT_STYLE: Style =
    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD);
