//! Board rendering: a kanban of bordered columns, each holding bordered
//! cards with a left color accent, state badge, label chips, and assignee.
//! Pure over [`App`] — no store access, no sync/push/factory logic.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::outbox::{Card, Conflict};

use super::app::App;
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

/// Wrap the card's badges (state, labels, assignee, milestone, factory) into
/// whole chips at chip boundaries. Each returned string is a single rendered
/// line of at most `max_width` cells, chips separated by a single space. A
/// chip wider than `max_width` alone is the only elided case (trailing `…`).
fn badge_lines(card: &Card, max_width: usize) -> Vec<String> {
    let mut chips: Vec<String> = vec![state_badge(card)];
    for label in &card.fields.labels {
        chips.push(format!("[{label}]"));
    }
    if let Some(assignee) = &card.fields.assignee {
        chips.push(format!("@{assignee}"));
    }
    if let Some(milestone) = &card.fields.milestone {
        chips.push(format!("m:{milestone}"));
    }
    if card.factory_kind.is_factory_request() {
        chips.push("⚙".to_owned());
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    for chip in chips {
        let chip_len = chip.chars().count();
        if current.is_empty() {
            current = if chip_len <= max_width {
                chip
            } else {
                truncate(&chip, max_width)
            };
        } else if current.chars().count() + 1 + chip_len <= max_width {
            current.push(' ');
            current.push_str(&chip);
        } else {
            lines.push(current);
            current = if chip_len <= max_width {
                chip
            } else {
                truncate(&chip, max_width)
            };
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// Render the whole board.
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // toolbar
            Constraint::Min(1),    // columns
            Constraint::Length(1), // command bar
        ])
        .split(area);

    render_header(frame, vertical[0], app);
    render_toolbar(frame, vertical[1], app);
    render_columns(frame, vertical[2], app);
    render_command_bar(frame, vertical[3]);
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
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
        // Card inner width is `inner.width - 2` (card border); badges are
        // indented two cells, so the available chip width is `inner.width - 4`.
        let badge_lines = badge_lines(card, inner.width.saturating_sub(4) as usize);
        let height = 3 + badge_lines.len() as u16 + diff_rows;
        if y + height > inner.height {
            break;
        }
        let slot = Rect::new(inner.x, inner.y + y, inner.width, height);
        render_card(frame, slot, entry, app, is_focused, diff_rows, &badge_lines);
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
    badge_lines: &[String],
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

    let mut lines: Vec<Line<'_>> = vec![Line::from(vec![
        Span::styled("▌", Style::new().fg(accent).add_modifier(Modifier::BOLD)),
        Span::raw(format!(" {title}")),
    ])];
    for badge_line in badge_lines {
        lines.push(Line::from(Span::styled(
            format!("  {badge_line}"),
            Style::new().fg(if card.fields.state == "closed" {
                Color::DarkGray
            } else {
                Color::Gray
            }),
        )));
    }

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
    use super::{badge_lines, render};
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

    fn badge_card(number: u64, labels: Vec<String>) -> Card {
        let mut c = card(number, "to-do", "open");
        c.fields.labels = labels;
        c.fields.assignee = None;
        c.fields.milestone = None;
        c.factory_kind = FactoryKind::Ordinary;
        c
    }

    #[test]
    fn badge_lines_wraps_whole_chips_never_split() {
        // "• open" (6) + " [high-priority]" (16) = 22 > 20 → two lines; the
        // label chip (15 cells) stays whole on line 2.
        let c = badge_card(1, vec!["high-priority".to_owned()]);
        let lines = badge_lines(&c, 20);
        assert_eq!(
            lines,
            vec!["• open".to_owned(), "[high-priority]".to_owned()]
        );
        assert!(lines.join("\n").contains("high-priority"));
        assert!(!lines.join(" ").contains('…'));
    }

    #[test]
    fn badge_lines_single_line_when_short() {
        // "• open [bug] @founder m:v1 ⚙" = 29 cells, fits within 60.
        let c = card(1, "to-do", "open");
        let lines = badge_lines(&c, 60);
        assert_eq!(lines, vec!["• open [bug] @founder m:v1 ⚙".to_owned()]);
    }

    #[test]
    fn badge_lines_elides_only_an_oversized_chip() {
        // A 30-char label is a 32-cell chip, wider than the 20-cell line: the
        // only elided case — trailing `…`, never exceeding `max_width`.
        let c = badge_card(1, vec!["a".repeat(30)]);
        let lines = badge_lines(&c, 20);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "• open");
        assert!(lines[1].ends_with('…'));
        assert!(lines[1].chars().count() <= 20);
    }

    #[test]
    fn render_wraps_long_label_and_grows_card() {
        let store = Store::open_in_memory().expect("open store");

        let mut long = card(1, "to-do", "open");
        long.fields.labels = vec!["priority".to_owned()];
        long.fields.assignee = None;
        long.fields.milestone = None;
        long.factory_kind = FactoryKind::Ordinary;
        store.insert_card(&long).expect("insert long-label card");

        let app = App::new(store, RepoIdentity::new("thalixinc", "herdr-board"));
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("draw");

        let text = buffer_text(terminal.backend());
        assert!(text.contains("issue 1"), "title present");
        assert!(text.contains("• open"), "state badge present");
        assert!(
            text.contains("[priority]"),
            "full label chip present, not truncated mid-chip"
        );
        assert!(!text.contains('…'), "no mid-chip ellipsis");
    }
}
