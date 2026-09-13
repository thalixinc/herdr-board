//! The crossterm event loop: poll → map to Action → dispatch → reload → render.
//! No timer, no tick-driven sync.

use std::io;

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::create::GitHubClient;
use crate::receiver::HandoffTransport;
use crate::sync::PullClient;

use super::actions::{dispatch, Action, Deps, KeyMap};
use super::{render, App};

/// Run the board event loop until the user quits.
pub fn run_loop<C, T>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    deps: &Deps<C, T>,
) -> io::Result<()>
where
    C: PullClient + GitHubClient,
    T: HandoffTransport,
{
    let keymap = KeyMap;
    loop {
        terminal.draw(|frame| render(frame, app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let Some(action) = keymap.resolve(key.code, key.modifiers) else {
                continue;
            };
            if matches!(action, Action::Quit) {
                return Ok(());
            }
            if let Err(error) = dispatch(app, action, deps) {
                app.status = Some(format!("error: {error}"));
            }
        }
    }
}
