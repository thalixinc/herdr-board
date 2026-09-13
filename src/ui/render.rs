//! Board rendering: column layout, card badges/chips, the focused conflict
//! diff, the filter bar, and the status line. Pure over [`App`] — no store
//! access, no sync/push/factory logic.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::outbox::Conflict;

use super::app::App;
use super::model::{Entry, Filters};

/// Render the whole board into `frame`.
pub fn render(frame: &mut Frame<'_>, app: &App) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Min(1),    // columns
            Constraint::Length(1), // filter bar
            Constraint::Length(1), // status line
        ])
        .split(area);

    let title = format!("herdr-board — {}/{}", app.repo.owner, app.repo.repo);
    frame.render_widget(
        Paragraph::new(Span::styled(title, crate::ui::TITLE_STYLE)),
        vertical[0],
    );

    if app.model.columns.is_empty() {
        frame.render_widget(Paragraph::new("(empty board)"), vertical[1]);
    } else {
        let count = app.model.columns.len() as u32;
        let widths: Vec<Constraint> = (0..count).map(|_| Constraint::Ratio(1, count)).collect();
        let column_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(widths)
            .split(vertical[1]);
        for (i, column) in app.model.columns.iter().enumerate() {
            render_column(frame, column_areas[i], column, app);
        }
    }

    frame.render_widget(
        Paragraph::new(Span::raw(filter_bar_text(&app.filters))),
        vertical[2],
    );

    let status = app.status.as_deref().unwrap_or("ready");
    frame.render_widget(Paragraph::new(Span::raw(status)), vertical[3]);
}

fn render_column(frame: &mut Frame<'_>, area: Rect, column: &super::model::BoardColumn, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(column.name.as_str());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let focused = app.focused();
    let mut lines: Vec<Line<'_>> = Vec::new();
    for entry in &column.entries {
        let card = entry.card();
        lines.push(Line::from(Span::raw(card_line(entry))));

        let is_focused = focused.is_some_and(|f| f.identity == card.identity);
        if is_focused && card.conflict == Conflict::ApplyPending {
            lines.push(Line::from(Span::styled(
                "issue changed; apply?",
                crate::ui::CONFLICT_STYLE,
            )));
            if let Some(diffs) = app.conflicts.get(&card.identity) {
                for diff in diffs {
                    lines.push(Line::from(Span::raw(format!(
                        "  {}: {} → {}",
                        diff.field.as_str(),
                        diff.old,
                        diff.new
                    ))));
                }
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

/// The single-line rendering of one card entry (badges and chips, no diff).
fn card_line(entry: &Entry) -> String {
    let card = entry.card();
    let indent = if entry.is_indented() { "  " } else { "" };
    let marker = if matches!(entry, Entry::Epic(_)) {
        "▸ "
    } else {
        ""
    };

    let mut line = format!(
        "{indent}{marker}#{} {}",
        card.identity.number, card.fields.title
    );
    line.push_str(&format!(" [{}]", state_badge(card)));
    for label in &card.fields.labels {
        line.push_str(&format!(" [{}]", label));
    }
    if let Some(assignee) = &card.fields.assignee {
        line.push_str(&format!(" @{}", assignee));
    }
    if let Some(milestone) = &card.fields.milestone {
        line.push_str(&format!(" m:{}", milestone));
    }
    if card.factory_kind.is_factory_request() {
        line.push_str(" ⚙");
    }
    if card.conflict == Conflict::ApplyPending {
        line.push_str(" ✗");
    }
    line
}

fn state_badge(card: &crate::outbox::Card) -> String {
    match &card.fields.state_reason {
        Some(reason) => format!("{}:{}", card.fields.state, reason),
        None => card.fields.state.clone(),
    }
}

fn filter_bar_text(filters: &Filters) -> String {
    let mut parts = Vec::new();
    if !filters.labels.is_empty() {
        parts.push(format!("labels:{}", filters.labels.join(",")));
    }
    if let Some(assignee) = &filters.assignee {
        parts.push(format!("assignee:{assignee}"));
    }
    if let Some(state) = &filters.state {
        parts.push(format!("state:{state}"));
    }
    if let Some(title) = &filters.title {
        parts.push(format!("title:{title}"));
    }
    if parts.is_empty() {
        "filters: none".to_owned()
    } else {
        format!("filters: {}", parts.join(" "))
    }
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
    fn render_shows_columns_badges_and_conflict_diff() {
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

        let app = App::new(store, RepoIdentity::new("thalixinc", "herdr-board"));
        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("draw");

        let text = buffer_text(terminal.backend());

        assert!(
            text.contains("herdr-board — thalixinc/herdr-board"),
            "title"
        );
        assert!(text.contains("to-do"), "column header");
        assert!(text.contains("open"), "open state badge");
        assert!(text.contains("closed"), "closed state badge");
        assert!(text.contains("bug"), "label chip");
        assert!(text.contains("⚙"), "factory badge");
        assert!(text.contains("issue changed; apply?"), "conflict marker");
        assert!(text.contains("body"), "diff field");
        assert!(text.contains("old body"), "diff old value");
        assert!(text.contains("new body"), "diff new value");
    }

    #[test]
    fn render_empty_board() {
        let store = Store::open_in_memory().expect("open store");
        let app = App::new(store, RepoIdentity::new("o", "r"));
        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| render(frame, &app)).expect("draw");
        let text = buffer_text(terminal.backend());
        assert!(text.contains("(empty board)"));
        assert!(text.contains("filters: none"));
    }
}
