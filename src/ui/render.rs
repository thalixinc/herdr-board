//! Board rendering: a kanban of bordered columns, each holding bordered
//! cards with a left color accent, state badge, label chips, and assignee.
//! Pure over [`App`] — no store access, no sync/push/factory logic.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::outbox::{Card, Conflict};

use super::app::{App, FilterInput};
use super::model::{BoardColumn, Entry};

/// Left-accent + badge colors, keyed to card state (open/closed/conflict) and
/// factory intent.
const ACCENT_OPEN: Color = Color::Green;
const ACCENT_CLOSED: Color = Color::DarkGray;
const ACCENT_CONFLICT: Color = Color::Red;
const ACCENT_FACTORY: Color = Color::Cyan;
const BORDER: Color = Color::DarkGray;
const FOCUSED: Color = Color::Yellow;
const HEADER: Color = Color::Cyan;

/// The header row: board mark, name, running indicator, and repo scope.
const CMD_BAR: &str = "[ s Sync ]  [ ←→ Move ]  [ f Factory ]  [ a Apply ]  [ q Quit ]";

/// The left-accent color for a card.
fn accent(card: &Card) -> Color {
    if card.conflict == Conflict::ApplyPending {
        ACCENT_CONFLICT
    } else if card.factory_kind.is_factory_request() {
        ACCENT_FACTORY
    } else if card.fields.state == "closed" {
        ACCENT_CLOSED
    } else {
        ACCENT_OPEN
    }
}

/// The human state badge: `• open`, `✓ closed[:reason]`, or `✗ changed`.
fn state_badge(card: &Card) -> String {
    if card.conflict == Conflict::ApplyPending {
        return "✗ changed".to_owned();
    }
    match card.fields.state.as_str() {
        "closed" => match &card.fields.state_reason {
            Some(reason) => format!("✓ closed:{reason}"),
            None => "✓ closed".to_owned(),
        },
        _ => "• open".to_owned(),
    }
}

/// Truncate to `max` chars with a trailing ellipsis.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Render the whole board.
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let bar_active = app.filter_input.is_active();

    let mut constraints = vec![
        Constraint::Length(1), // header
        Constraint::Length(1), // toolbar
    ];
    if bar_active {
        constraints.push(Constraint::Length(1)); // filter prompt
    }
    constraints.push(Constraint::Min(1)); // columns
    constraints.push(Constraint::Length(1)); // command bar

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    render_header(frame, vertical[0], app);
    render_toolbar(frame, vertical[1], app);
    let mut index = 2;
    if bar_active {
        render_filter_prompt(frame, vertical[index], app);
        index += 1;
    }
    render_columns(frame, vertical[index], app);
    render_command_bar(frame, vertical[index + 1]);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let line = Line::from(vec![
        Span::styled("◆ ", Style::new().fg(HEADER).add_modifier(Modifier::BOLD)),
        Span::styled(
            "herdr-board",
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("● running", Style::new().fg(Color::Green)),
        Span::raw("   "),
        Span::styled(
            format!("{}/{}", app.repo.owner, app.repo.repo),
            Style::new().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_toolbar(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let mut spans: Vec<Span<'_>> = Vec::new();
    let tabs: [(&str, Option<&str>); 3] = [
        (" All ", None),
        (" Open ", Some("open")),
        (" Closed ", Some("closed")),
    ];
    for (label, state) in tabs {
        let active = app.filters.state.as_deref() == state;
        let style = if active {
            Style::new().fg(Color::Black).bg(Color::Green)
        } else {
            Style::new().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!("[{label}]"), style));
        spans.push(Span::raw(" "));
    }

    // Any non-state filters still active, shown as trailing chips.
    for label in &app.filters.labels {
        spans.push(Span::styled(
            format!("[label:{label}]"),
            Style::new().fg(Color::Magenta),
        ));
        spans.push(Span::raw(" "));
    }
    if let Some(assignee) = &app.filters.assignee {
        spans.push(Span::styled(
            format!("[@{assignee}]"),
            Style::new().fg(Color::Magenta),
        ));
        spans.push(Span::raw(" "));
    }
    if let Some(title) = &app.filters.title {
        spans.push(Span::styled(
            format!("[“{title}”]"),
            Style::new().fg(Color::Magenta),
        ));
        spans.push(Span::raw(" "));
    }
    if let Some(epic) = app.filters.epic {
        spans.push(Span::styled(
            format!("[epic:#{epic}]"),
            Style::new().fg(Color::Magenta),
        ));
        spans.push(Span::raw(" "));
    }
    if let Some(milestone) = &app.filters.milestone {
        spans.push(Span::styled(
            format!("[m:{milestone}]"),
            Style::new().fg(Color::Magenta),
        ));
        spans.push(Span::raw(" "));
    }
    if let Some(repository) = &app.filters.repository {
        spans.push(Span::styled(
            format!("[repo:{repository}]"),
            Style::new().fg(Color::Magenta),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The filter-input prompt line: `assignee > founder_` style, buffer echoed.
fn render_filter_prompt(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let FilterInput::Active { dimension, buffer } = &app.filter_input else {
        return;
    };
    let line = Line::from(vec![
        Span::styled(
            format!("{} > ", dimension.name()),
            Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        ),
        Span::styled(buffer.clone(), Style::new().fg(Color::White)),
        Span::styled("_", Style::new().fg(Color::Cyan)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_columns(frame: &mut Frame<'_>, area: Rect, app: &App) {
    if app.model.columns.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "(empty board)",
                Style::new().fg(Color::DarkGray),
            )),
            area,
        );
        return;
    }

    let count = app.model.columns.len() as u32;
    let widths: Vec<Constraint> = (0..count).map(|_| Constraint::Ratio(1, count)).collect();
    let column_areas = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(widths)
        .split(area);
    for (i, column) in app.model.columns.iter().enumerate() {
        render_column(frame, column_areas[i], column, app);
    }
}

/// The column header: uppercase name + card count.
fn column_header(column: &BoardColumn) -> String {
    format!(
        "{} · {}",
        column.name.to_ascii_uppercase(),
        column.entries.len()
    )
}

fn render_column(frame: &mut Frame<'_>, area: Rect, column: &BoardColumn, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(column_header(column))
        .title_style(Style::new().fg(Color::White).add_modifier(Modifier::BOLD))
        .border_style(Style::new().fg(BORDER));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let focused = app.focused();
    let mut y: u16 = 0;
    for entry in &column.entries {
        let card = entry.card();
        let is_focused = focused.is_some_and(|f| f.identity == card.identity);
        let diff_rows = if is_focused && card.conflict == Conflict::ApplyPending {
            app.conflicts
                .get(&card.identity)
                .map(|d| d.len())
                .unwrap_or(0) as u16
                + 1
        } else {
            0
        };
        let height = 4 + diff_rows;
        if y + height > inner.height {
            break;
        }
        let slot = Rect::new(inner.x, inner.y + y, inner.width, height);
        render_card(frame, slot, entry, app, is_focused, diff_rows);
        y += height;
    }
}

fn render_card(
    frame: &mut Frame<'_>,
    area: Rect,
    entry: &Entry,
    app: &App,
    is_focused: bool,
    diff_rows: u16,
) {
    let card = entry.card();
    let accent = accent(card);
    let border = if is_focused { FOCUSED } else { BORDER };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let indent = if entry.is_indented() { "  " } else { "" };
    let marker = if matches!(entry, Entry::Epic(_)) {
        "▸ "
    } else {
        ""
    };
    let title = truncate(
        &format!(
            "{indent}{marker}#{} {}",
            card.identity.number, card.fields.title
        ),
        width.saturating_sub(2),
    );

    let mut badges: Vec<String> = vec![state_badge(card)];
    for label in &card.fields.labels {
        badges.push(format!("[{label}]"));
    }
    if let Some(assignee) = &card.fields.assignee {
        badges.push(format!("@{assignee}"));
    }
    if let Some(milestone) = &card.fields.milestone {
        badges.push(format!("m:{milestone}"));
    }
    if card.factory_kind.is_factory_request() {
        badges.push("⚙".to_owned());
    }
    let badge_text = truncate(&badges.join(" "), width.saturating_sub(2));

    let mut lines: Vec<Line<'_>> = vec![
        Line::from(vec![
            Span::styled("▌", Style::new().fg(accent).add_modifier(Modifier::BOLD)),
            Span::raw(format!(" {title}")),
        ]),
        Line::from(Span::styled(
            format!("  {badge_text}"),
            Style::new().fg(if card.fields.state == "closed" {
                Color::DarkGray
            } else {
                Color::Gray
            }),
        )),
    ];

    if is_focused && card.conflict == Conflict::ApplyPending {
        lines.push(Line::from(Span::styled(
            "issue changed; apply?",
            Style::new()
                .fg(ACCENT_CONFLICT)
                .add_modifier(Modifier::BOLD),
        )));
        if let Some(diffs) = app.conflicts.get(&card.identity) {
            for diff in diffs {
                lines.push(Line::from(Span::styled(
                    format!("  {}: {} → {}", diff.field.as_str(), diff.old, diff.new),
                    Style::new().fg(ACCENT_CONFLICT),
                )));
            }
        }
    }

    // Ensure the card block stays filled to its reserved height.
    while lines.len() < diff_rows as usize + 2 {
        lines.push(Line::from(""));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_command_bar(frame: &mut Frame<'_>, area: Rect) {
    let mut spans: Vec<Span<'_>> = Vec::new();
    let mut parts = CMD_BAR.split("]  ");
    if let Some(first) = parts.next() {
        spans.push(Span::styled(
            format!("{first}]"),
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ));
    }
    for part in parts {
        spans.push(Span::styled(
            format!(" {part}"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::create::RepoIdentity;
    use crate::outbox::{CanonicalFields, Card, CardField, CardFieldDiff, Conflict, Store};
    use crate::ui::app::App;
    use crate::{FactoryKind, Identity};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn card(number: u64, column: &str, state: &str) -> Card {
        Card {
            identity: Identity::new("thalixinc", "herdr-board", number),
            url: format!("https://github.com/thalixinc/herdr-board/issues/{number}"),
            fields: CanonicalFields {
                title: format!("issue {number}"),
                body: String::new(),
                state: state.to_owned(),
                state_reason: None,
                labels: vec!["bug".to_owned()],
                assignee: Some("founder".to_owned()),
                milestone: Some("v1".to_owned()),
            },
            column: column.to_owned(),
            factory_kind: FactoryKind::FactoryRequest,
            revision: "rev".to_owned(),
            conflict: Conflict::None,
            synced_at: 0,
        }
    }

    fn buffer_text(backend: &TestBackend) -> String {
        let buffer = backend.buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[test]
    fn render_shows_header_columns_badges_and_conflict() {
        let store = Store::open_in_memory().expect("open store");

        let mut conflicted = card(1, "to-do", "open");
        conflicted.conflict = Conflict::ApplyPending;
        store.insert_card(&conflicted).expect("insert conflicted");
        store
            .record_conflict(
                &conflicted.identity,
                &[CardFieldDiff {
                    field: CardField::Body,
                    old: "old body".to_owned(),
                    new: "new body".to_owned(),
                }],
                1000,
            )
            .expect("record conflict");

        let mut closed = card(2, "done", "closed");
        closed.factory_kind = FactoryKind::Ordinary;
        store.insert_card(&closed).expect("insert closed");

        let mut plain_open = card(3, "in-progress", "open");
        plain_open.factory_kind = FactoryKind::Ordinary;
        store.insert_card(&plain_open).expect("insert plain open");

        let app = App::new(store, RepoIdentity::new("thalixinc", "herdr-board"));
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("draw");

        let text = buffer_text(terminal.backend());

        assert!(text.contains("herdr-board"), "header name");
        assert!(text.contains("● running"), "running indicator");
        assert!(text.contains("TO-DO"), "uppercase column header");
        assert!(text.contains("open"), "open state badge");
        assert!(text.contains("closed"), "closed state badge");
        assert!(text.contains("bug"), "label chip");
        assert!(text.contains("⚙"), "factory badge");
        assert!(text.contains("issue changed; apply?"), "conflict marker");
        assert!(text.contains("old body"), "diff old value");
        assert!(text.contains("new body"), "diff new value");
        assert!(text.contains("[ s Sync ]"), "command bar");
    }

    #[test]
    fn render_empty_board() {
        let store = Store::open_in_memory().expect("open store");
        let app = App::new(store, RepoIdentity::new("o", "r"));
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let text = buffer_text(terminal.backend());
        assert!(text.contains("TO-DO · 0"), "empty column still renders");
        assert!(text.contains("DONE · 0"), "empty column still renders");
        assert!(text.contains("[ All ]"), "toolbar tab");
    }

    #[test]
    fn render_filter_bar_filters_columns() {
        let store = Store::open_in_memory().expect("open store");

        store
            .insert_card(&card(1, "to-do", "open"))
            .expect("insert founder card");

        let mut other = card(2, "to-do", "open");
        other.fields.assignee = Some("someone-else".to_owned());
        other.fields.title = "someone else's issue".to_owned();
        store.insert_card(&other).expect("insert other card");

        let mut app = App::new(store, RepoIdentity::new("thalixinc", "herdr-board"));
        app.open_filter();
        for c in "founder".chars() {
            app.push_filter_char(c);
        }
        app.apply_filter();

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let text = buffer_text(terminal.backend());

        assert!(
            text.contains("assignee > founder"),
            "prompt echoes the active dimension and buffer"
        );
        assert!(text.contains("issue 1"), "matching card renders");
        assert!(
            !text.contains("someone else's issue"),
            "non-matching card is filtered out"
        );
        assert!(text.contains("[@founder]"), "assignee chip renders");
    }

    #[test]
    fn render_filter_chips_all_dimensions() {
        let store = Store::open_in_memory().expect("open store");
        store
            .insert_card(&card(1, "to-do", "open"))
            .expect("insert card");

        let mut app = App::new(store, RepoIdentity::new("thalixinc", "herdr-board"));
        app.filters.assignee = Some("founder".to_owned());
        app.filters.labels = vec!["bug".to_owned()];
        app.filters.title = Some("sync".to_owned());
        app.filters.epic = Some(10);
        app.filters.milestone = Some("v1".to_owned());
        app.filters.repository = Some("thalixinc/herdr-board".to_owned());

        let backend = TestBackend::new(160, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let text = buffer_text(terminal.backend());

        assert!(text.contains("[epic:#10]"), "epic chip renders");
        assert!(text.contains("[m:v1]"), "milestone chip renders");
        assert!(
            text.contains("[repo:thalixinc/herdr-board]"),
            "repository chip renders"
        );
        assert!(text.contains("[label:bug]"), "label chip renders");
        assert!(text.contains("[@founder]"), "assignee chip renders");
    }
}
