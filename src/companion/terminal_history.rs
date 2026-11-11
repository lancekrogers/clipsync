//! History list and selected-copy pane for the terminal companion.
use super::{
    client::{HistoryRow, Intent, View},
    terminal_image::GraphicsPreview,
    terminal_theme::Theme,
};
use chrono::{DateTime, Datelike, TimeZone};
use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Padding, Paragraph, Wrap},
    Frame,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use uuid::Uuid;

/// A History removal waiting for the user to answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Removal {
    Entry(Uuid),
    All { searching: bool },
}

impl Removal {
    pub fn question(self) -> &'static str {
        match self {
            Self::Entry(_) => "Delete this saved copy on this computer and connected devices?",
            Self::All { searching: false } => {
                "Clear all history and the clipboard on this computer and connected devices?"
            }
            Self::All { searching: true } => {
                "Clear ALL history, not only these matches, and the clipboard on this computer and connected devices?"
            }
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            Self::Entry(_) => "delete",
            Self::All { .. } => "clear all",
        }
    }

    fn intent(self) -> Intent {
        match self {
            Self::Entry(id) => Intent::DeleteEntry(id),
            Self::All { .. } => Intent::ClearHistory,
        }
    }
}

/// `d` deletes the selected copy and `D` clears everything, each only after `y`.
#[derive(Debug, Default)]
pub struct Confirm(Option<Removal>);

impl Confirm {
    pub fn pending(&self) -> Option<Removal> {
        self.0
    }

    /// Opens the question. A service without support gets the intent at once
    /// so the controller can explain; it never sends that service a removal.
    pub fn ask(&mut self, all: bool, view: &View) -> Option<Intent> {
        let (removal, supported) = if all {
            (
                Removal::All {
                    searching: !view.query.is_empty(),
                },
                view.history_clear_supported,
            )
        } else {
            (
                Removal::Entry(view.selected?),
                view.history_delete_supported,
            )
        };
        if !supported {
            return Some(removal.intent());
        }
        self.0 = Some(removal);
        None
    }

    /// Only `y` confirms; any other key cancels.
    pub fn answer(&mut self, key: KeyCode) -> Option<Intent> {
        let removal = self.0.take()?;
        matches!(key, KeyCode::Char('y' | 'Y')).then(|| removal.intent())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Link,
    Image,
    /// Only whitespace; would otherwise render as an empty row.
    Blank,
    /// Only rule or punctuation characters that look like interface chrome.
    LineArt,
}

impl Kind {
    pub fn of(row: &HistoryRow) -> Self {
        if row.content_type.starts_with("image/") {
            return Self::Image;
        }
        let text = row.preview.trim();
        if text.is_empty() {
            Self::Blank
        } else if text.chars().all(|ch| ch.is_whitespace() || is_line_art(ch)) {
            Self::LineArt
        } else if (text.starts_with("https://") || text.starts_with("http://"))
            && !text.contains(char::is_whitespace)
        {
            Self::Link
        } else {
            Self::Text
        }
    }

    fn glyph(self) -> &'static str {
        match self {
            Self::Image => "▣",
            Self::Link => "↗",
            Self::Text | Self::Blank | Self::LineArt => "¶",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Link => "Link",
            Self::Text | Self::Blank | Self::LineArt => "Text",
        }
    }
}

fn is_line_art(ch: char) -> bool {
    matches!(
        ch,
        '\u{2500}'
            ..='\u{259F}'
                | '-'
                | '='
                | '_'
                | '~'
                | '*'
                | '#'
                | '+'
                | '|'
                | '.'
                | '·'
                | '•'
                | '—'
                | '–'
    )
}

/// Fit text to a display width, marking a cut with an ellipsis.
pub fn truncate(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let cell = ch.width().unwrap_or(0);
        if used + cell + 1 > width {
            break;
        }
        out.push(ch);
        used += cell;
    }
    out.push('…');
    out
}

/// A short time for a list: minutes while recent, then day names and dates.
pub fn relative_time<Tz: TimeZone>(at: &DateTime<Tz>, now: &DateTime<Tz>) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let seconds = now.clone().signed_duration_since(at.clone()).num_seconds();
    let days = now
        .date_naive()
        .signed_duration_since(at.date_naive())
        .num_days();
    let clock = at.format("%-I:%M %p");
    if seconds < 60 {
        "just now".into()
    } else if seconds < 3600 {
        format!("{}m ago", seconds / 60)
    } else if days == 0 {
        format!("Today {clock}")
    } else if days == 1 {
        format!("Yesterday {clock}")
    } else if days < 7 {
        format!("{} {clock}", at.format("%a"))
    } else if at.year() == now.year() {
        at.format("%b %-d").to_string()
    } else {
        at.format("%b %-d, %Y").to_string()
    }
}

fn when(row: &HistoryRow) -> String {
    match &row.copied_at {
        Some(at) => relative_time(at, &chrono::Local::now()),
        None => row.timestamp.clone(),
    }
}

fn count(characters: u32) -> String {
    let digits = characters.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    format!(
        "{grouped} character{}",
        if characters == 1 { "" } else { "s" }
    )
}

fn device<'a>(view: &View, row: &HistoryRow, theme: &Theme) -> Span<'a> {
    let style = if view.is_local(row.origin) {
        theme.muted()
    } else {
        theme.device(row.origin)
    };
    Span::styled(view.device_name(row.origin), style)
}

fn item<'a>(
    view: &View,
    row: &HistoryRow,
    theme: &Theme,
    width: u16,
    selected: bool,
) -> ListItem<'a> {
    let kind = Kind::of(row);
    let bar = if selected {
        theme.selection_bar()
    } else {
        Span::raw(" ")
    };
    let glyph = match kind {
        Kind::Image | Kind::Link => theme.accent(),
        _ => theme.muted(),
    };
    let strong = if selected {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let room = usize::from(width).saturating_sub(4);
    let quiet = theme.muted().add_modifier(Modifier::ITALIC);
    let preview = match kind {
        Kind::Image => Span::styled("Image", strong),
        Kind::Link => Span::styled(truncate(&row.preview, room), theme.link().patch(strong)),
        Kind::Text => Span::styled(truncate(&row.preview, room), strong),
        Kind::LineArt => Span::styled(
            format!("“{}”", truncate(row.preview.trim(), room.saturating_sub(2))),
            quiet,
        ),
        Kind::Blank => Span::styled(format!("Blank · {}", count(row.characters)), quiet),
    };
    let first = Line::from(vec![
        bar.clone(),
        Span::raw(" "),
        Span::styled(kind.glyph(), glyph),
        Span::raw(" "),
        preview,
    ]);
    let second = Line::from(vec![
        bar,
        Span::raw("   "),
        device(view, row, theme),
        Span::styled(format!(" · {}", when(row)), theme.muted()),
    ]);
    ListItem::new(vec![first, second]).style(if selected {
        theme.selection()
    } else {
        Style::default()
    })
}

pub struct Pane<'a> {
    pub view: &'a View,
    pub theme: &'a Theme,
    pub search: bool,
    pub scroll: u16,
    pub color: bool,
}

impl Pane<'_> {
    pub fn render(&self, frame: &mut Frame, area: Rect, graphics: &mut GraphicsPreview) {
        let wide = area.width >= 90;
        let columns = Layout::default()
            .direction(if wide {
                Direction::Horizontal
            } else {
                Direction::Vertical
            })
            .constraints(if wide {
                [Constraint::Percentage(50), Constraint::Percentage(50)]
            } else {
                [Constraint::Percentage(55), Constraint::Percentage(45)]
            })
            .split(area);
        self.list(frame, columns[0]);
        self.detail(frame, columns[1], graphics);
    }

    fn list(&self, frame: &mut Frame, area: Rect) {
        let (view, theme) = (self.view, self.theme);
        let search = if self.search {
            vec![
                Span::styled(" / ", theme.key()),
                Span::styled(
                    view.query.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled("▏ ", theme.accent()),
            ]
        } else if view.query.is_empty() {
            vec![
                Span::styled(" / ", theme.key()),
                Span::styled("search ", theme.muted()),
            ]
        } else {
            vec![
                Span::styled(" / ", theme.key()),
                Span::raw(format!("{} ", view.query)),
            ]
        };
        let block = theme
            .block(" History ", true)
            .title_top(Line::from(search).right_aligned());
        if view.history.is_empty() {
            let message = if view.history_loading {
                "Loading…"
            } else if view.query.is_empty() {
                "Nothing copied yet."
            } else {
                "No matches."
            };
            frame.render_widget(
                Paragraph::new(Line::styled(message, theme.muted()))
                    .block(block.padding(Padding::horizontal(1))),
                area,
            );
            return;
        }
        let width = area.width.saturating_sub(2);
        let rows = view
            .history
            .iter()
            .map(|row| item(view, row, theme, width, Some(row.id) == view.selected))
            .collect::<Vec<_>>();
        let mut selection = ListState::default().with_selected(
            view.history
                .iter()
                .position(|row| Some(row.id) == view.selected),
        );
        frame.render_stateful_widget(List::new(rows).block(block), area, &mut selection);
    }

    fn detail(&self, frame: &mut Frame, area: Rect, graphics: &mut GraphicsPreview) {
        let (view, theme) = (self.view, self.theme);
        let row = view
            .history
            .iter()
            .find(|row| Some(row.id) == view.selected);
        let key = |text: &'static str| Span::styled(text, theme.key());
        let quiet = |text: String| Span::styled(text, theme.muted());
        let mut block = match row {
            _ if view.detail_loading => theme
                .frame(false)
                .title(Span::styled(" Loading selected copy… ", theme.muted())),
            Some(row) => {
                let kind = Kind::of(row);
                let source = Line::from(vec![
                    Span::raw(" "),
                    device(view, row, theme),
                    quiet(format!(" · {} ", when(row))),
                ]);
                let room = usize::from(area.width.saturating_sub(4)).saturating_sub(source.width());
                let mut title = vec![
                    Span::raw(" "),
                    Span::styled(kind.glyph(), theme.accent()),
                    Span::styled(format!(" {}", kind.label()), theme.title()),
                ];
                let characters = format!(" · {}", count(row.characters));
                if kind != Kind::Image && 8 + characters.width() <= room {
                    title.push(quiet(characters));
                }
                title.push(Span::raw(" "));
                let block = theme.frame(false).title(Line::from(title));
                if room >= 8 {
                    block.title_top(source.right_aligned())
                } else {
                    block
                }
            }
            None => theme
                .frame(false)
                .title(Span::styled(" Preview ", theme.title())),
        };
        let mut keys = vec![Span::raw(" ")];
        if row.is_some() && !view.detail_loading {
            if view.can_act() && view.detail.is_some() {
                keys.extend([key("c"), quiet(" copy".into())]);
            } else {
                keys.push(quiet("Copy unavailable".into()));
            }
            if view.history_delete_supported {
                keys.extend([quiet("  ".into()), key("d"), quiet(" delete".into())]);
            }
            if view.image_preview.is_none() {
                keys.extend([
                    quiet("  ".into()),
                    key("PgUp/PgDn"),
                    quiet(" scroll".into()),
                ]);
            }
            if view.detail_pages() > 1 {
                keys.extend([
                    quiet("  ".into()),
                    key("[ ]"),
                    quiet(format!(
                        " page {}/{}",
                        view.detail_page + 1,
                        view.detail_pages()
                    )),
                ]);
            }
            keys.push(Span::raw(" "));
            block = block.title_bottom(Line::from(keys));
        }
        if let Some(preview) = &view.image_preview {
            let inner = block.inner(area);
            frame.render_widget(block, area);
            if !graphics.render(frame, inner) {
                frame.render_widget(
                    Paragraph::new(super::terminal_image::lines(
                        preview,
                        inner.width,
                        inner.height,
                        self.color,
                    )),
                    inner,
                );
            }
            return;
        }
        let image_without_preview =
            view.detail.is_some() && row.is_some_and(|row| Kind::of(row) == Kind::Image);
        let hint = |lead: &'static str, tail: &'static str| {
            Line::from(vec![
                Span::styled(lead, theme.muted()),
                key("c"),
                Span::styled(tail, theme.muted()),
            ])
        };
        let body = if image_without_preview {
            vec![hint(
                "Image preview unavailable. Press ",
                " to copy it again.",
            )]
        } else if let Some(text) = view.visible_detail() {
            text.lines()
                .map(|line| Line::raw(line.to_owned()))
                .collect()
        } else if view.history.is_empty() {
            vec![Line::styled(
                if view.query.is_empty() {
                    "New copies appear here while the service is running."
                } else {
                    "No copies match your search. Clear the query or copy text while the service is running."
                },
                theme.muted(),
            )]
        } else {
            vec![Line::from(vec![
                key("↑/↓"),
                Span::styled(" select a saved copy. Press ", theme.muted()),
                key("c"),
                Span::styled(" to copy it again.", theme.muted()),
            ])]
        };
        frame.render_widget(
            Paragraph::new(body)
                .scroll((self.scroll, 0))
                .wrap(Wrap { trim: false })
                .block(block.padding(Padding::horizontal(1))),
            area,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    fn row(preview: &str, content_type: &str) -> HistoryRow {
        HistoryRow {
            id: uuid::Uuid::nil(),
            preview: preview.into(),
            timestamp: String::new(),
            source: String::new(),
            characters: preview.chars().count() as u32,
            content_type: content_type.into(),
            origin: uuid::Uuid::nil(),
            copied_at: None,
        }
    }

    fn view(selected: Option<Uuid>, supported: bool) -> View {
        View {
            selected,
            history_delete_supported: supported,
            history_clear_supported: supported,
            ..View::default()
        }
    }

    #[test]
    fn removal_waits_for_an_explicit_yes() {
        let id = Uuid::from_u128(5);
        let view = view(Some(id), true);
        let mut confirm = Confirm::default();
        for cancel in [
            KeyCode::Char('n'),
            KeyCode::Esc,
            KeyCode::Char('q'),
            KeyCode::Enter,
            KeyCode::Char('d'),
        ] {
            assert!(confirm.ask(false, &view).is_none());
            assert_eq!(confirm.pending(), Some(Removal::Entry(id)));
            assert!(confirm.answer(cancel).is_none(), "{cancel:?} must cancel");
            assert_eq!(confirm.pending(), None);
        }
        confirm.ask(false, &view);
        assert!(
            matches!(confirm.answer(KeyCode::Char('y')), Some(Intent::DeleteEntry(target)) if target == id)
        );
        confirm.ask(true, &view);
        assert_eq!(confirm.pending(), Some(Removal::All { searching: false }));
        assert!(matches!(
            confirm.answer(KeyCode::Char('Y')),
            Some(Intent::ClearHistory)
        ));
        assert!(
            confirm.answer(KeyCode::Char('y')).is_none(),
            "an answer is used once"
        );
    }

    #[test]
    fn delete_needs_a_selection_and_clear_while_searching_says_everything_goes() {
        let mut confirm = Confirm::default();
        assert!(confirm.ask(false, &view(None, true)).is_none());
        assert_eq!(confirm.pending(), None);
        let searching = View {
            query: "lunch".into(),
            ..view(None, true)
        };
        confirm.ask(true, &searching);
        let removal = confirm.pending().unwrap();
        assert_eq!(removal, Removal::All { searching: true });
        assert!(removal.question().contains("not only these matches"));
    }

    #[test]
    fn an_older_service_is_not_asked_and_is_left_to_explain() {
        let id = Uuid::from_u128(5);
        let mut confirm = Confirm::default();
        assert!(matches!(
            confirm.ask(false, &view(Some(id), false)),
            Some(Intent::DeleteEntry(target)) if target == id
        ));
        assert!(matches!(
            confirm.ask(true, &view(Some(id), false)),
            Some(Intent::ClearHistory)
        ));
        assert_eq!(confirm.pending(), None);
    }

    #[test]
    fn previews_that_look_like_chrome_are_classified() {
        assert_eq!(Kind::of(&row("────────", "text/plain")), Kind::LineArt);
        assert_eq!(Kind::of(&row("  --- ", "text/plain")), Kind::LineArt);
        assert_eq!(Kind::of(&row("   ", "text/plain")), Kind::Blank);
        assert_eq!(Kind::of(&row("[Image]", "image/png")), Kind::Image);
        assert_eq!(
            Kind::of(&row("https://example.com/a", "text/plain")),
            Kind::Link
        );
        assert_eq!(
            Kind::of(&row("see https://example.com", "text/plain")),
            Kind::Text
        );
        assert_eq!(Kind::of(&row("a ── b", "text/plain")), Kind::Text);
    }

    #[test]
    fn relative_times_read_naturally() {
        let zone = FixedOffset::west_opt(5 * 3600).unwrap();
        let now = zone.with_ymd_and_hms(2026, 11, 10, 21, 30, 0).unwrap();
        let ago = |minutes: i64| relative_time(&(now - chrono::Duration::minutes(minutes)), &now);
        assert_eq!(
            relative_time(&(now + chrono::Duration::seconds(2)), &now),
            "just now"
        );
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(3), "3m ago");
        assert_eq!(ago(59), "59m ago");
        assert_eq!(ago(135), "Today 7:15 PM");
        assert_eq!(ago(24 * 60 + 12), "Yesterday 9:18 PM");
        assert_eq!(ago(3 * 24 * 60), "Sat 9:30 PM");
        assert_eq!(ago(30 * 24 * 60), "Oct 11");
        assert_eq!(ago(400 * 24 * 60), "Oct 6, 2025");
    }

    #[test]
    fn truncation_respects_display_width() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
        assert_eq!(truncate("日本語テキスト", 7), "日本語…");
        assert_eq!(count(1), "1 character");
        assert_eq!(count(12345), "12,345 characters");
    }
}
