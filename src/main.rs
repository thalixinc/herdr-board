use std::env;
use std::io;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::{Frame, Terminal};

/// The herdr runtime context injected into every plugin command (plugins.mdx:
/// "Commands and environment"). `HERDR_ENV` proves we run inside a Herdr pane.
fn herdr_context() -> Vec<(&'static str, String)> {
    let vars = [
        "HERDR_PLUGIN_ID",
        "HERDR_PLUGIN_ENTRYPOINT_ID",
        "HERDR_WORKSPACE_ID",
        "HERDR_TAB_ID",
        "HERDR_PANE_ID",
        "HERDR_BIN_PATH",
        "HERDR_SOCKET_PATH",
    ];
    vars.iter()
        .map(|name| {
            let value = env::var(name).unwrap_or_else(|_| "<unset>".to_string());
            (*name, value)
        })
        .collect()
}

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    loop {
        terminal.draw(draw)?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        return Ok(())
                    }
                    _ => {}
                }
            }
        }
    }
}

fn draw(frame: &mut Frame) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let title = Paragraph::new(Text::from(Line::from(vec![
        Span::styled(
            herdr_board::PLUGIN_NAME,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  v{}", herdr_board::version())),
    ])))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" herdr-board "),
    );
    frame.render_widget(title, chunks[0]);

    let banner = Paragraph::new(
        "G1 native-tab proof: this pane runs the herdr-board plugin as a native Herdr TAB",
    )
    .style(Style::default().fg(Color::Yellow));
    frame.render_widget(banner, chunks[1]);

    let mut lines = vec![Line::from(Span::styled(
        "Injected Herdr context",
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    for (name, value) in herdr_context() {
        lines.push(Line::from(vec![
            Span::styled(format!("  {name:<28} "), Style::default().fg(Color::Green)),
            Span::raw(value),
        ]));
    }
    let context = Paragraph::new(Text::from(lines))
        .block(Block::default().borders(Borders::ALL).title(" context "));
    frame.render_widget(context, chunks[2]);

    let footer = Paragraph::new("q quit · esc quit · ctrl+c quit")
        .style(Style::default().fg(Color::DarkGray));
    frame.render_widget(footer, chunks[3]);
}
