//! Optional terminal renderer for the same companion controller as the GUI.
use super::{
    app::{Companion, SetupIntent, SetupState},
    client::{Intent, View},
    model::{Connection, Route},
    terminal_theme::{Depth, Theme, Tone},
};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Padding, Paragraph, Wrap},
    Terminal,
};
use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::Duration,
};

struct Restore;
impl Drop for Restore {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
    }
}
pub fn run(config: Option<PathBuf>, daemon_binary: Option<PathBuf>) -> Result<()> {
    anyhow::ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "ClipSync TUI needs an interactive terminal"
    );
    let mut app = Companion::new(config, daemon_binary)?;
    enable_raw_mode()?;
    let _restore = Restore;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, crossterm::cursor::Show);
        previous(info);
    }));
    let theme = Theme::new(Depth::from_env());
    let mut graphics = super::terminal_image::GraphicsPreview::new(theme.color());
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut devices = super::terminal_devices::DevicesUi::default();
    let mut search = false;
    let mut choice = 0usize;
    let mut detail_scroll = 0u16;
    loop {
        app.tick();
        let image = app.client.as_ref().filter(|_| app.route == Route::History);
        if graphics.update(
            image.and_then(|c| c.view.selected),
            image.and_then(|c| c.view.image_preview.as_ref()),
        ) {
            // Graphics placeholders span cells hidden from ratatui's normal
            // diff; clear them before drawing another selection or route.
            terminal.clear()?;
        }
        terminal.draw(|frame| {
            render(
                frame,
                &app,
                search,
                choice,
                &theme,
                detail_scroll,
                &devices,
                &mut graphics,
            )
        })?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            break;
        }
        if app.route == Route::Devices {
            if let Some(client) = &mut app.client {
                if devices.key(key, client) {
                    continue;
                }
            }
        }
        if search {
            if let Some(client) = &mut app.client {
                let mut query = client.view.query.clone();
                match key.code {
                    KeyCode::Esc | KeyCode::Enter => search = false,
                    KeyCode::Backspace => {
                        query.pop();
                        client.dispatch(Intent::Search(query));
                    }
                    KeyCode::Char(ch)
                        if !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        query.push(ch);
                        client.dispatch(Intent::Search(query));
                    }
                    _ => {}
                }
            } else {
                search = false;
            }
            continue;
        }
        let prior_route = app.route;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Char('1') => app.navigate(Route::Overview),
            KeyCode::Char('2') => app.navigate(Route::Devices),
            KeyCode::Char('3') => app.navigate(Route::History),
            KeyCode::Char('4') => app.navigate(Route::Settings),
            KeyCode::BackTab => {
                let index = Route::ALL
                    .iter()
                    .position(|route| *route == app.route)
                    .unwrap_or(0);
                app.navigate(Route::ALL[(index + 3) % 4]);
                detail_scroll = 0;
            }
            KeyCode::Tab => {
                let index = Route::ALL
                    .iter()
                    .position(|route| *route == app.route)
                    .unwrap_or(0);
                app.navigate(Route::ALL[(index + 1) % 4]);
            }
            KeyCode::Char('/') if app.route == Route::History => search = true,
            KeyCode::Char('[') | KeyCode::Char(']') if app.route == Route::History => {
                if let Some(client) = &mut app.client {
                    let page = if key.code == KeyCode::Char('[') {
                        client.view.detail_page.saturating_sub(1)
                    } else {
                        client.view.detail_page + 1
                    };
                    client.dispatch(Intent::DetailPage(page));
                    detail_scroll = 0;
                }
            }
            KeyCode::Char('r') => {
                app.setup_intent(SetupIntent::Inspect);
                if let Some(client) = &mut app.client {
                    client.dispatch(Intent::Refresh);
                }
            }
            KeyCode::Char('p') => {
                if let Some(client) = &mut app.client {
                    client.dispatch(Intent::SetPaused(
                        client.view.connection != Connection::Paused,
                    ));
                }
            }
            KeyCode::Char('c') if app.route == Route::History => {
                if let Some(client) = &mut app.client {
                    client.dispatch(Intent::CopySelected);
                }
            }
            KeyCode::Char('n') if app.setup.state == SetupState::Fresh => {
                app.setup_intent(SetupIntent::Create)
            }
            KeyCode::Char('i') if app.route == Route::Settings => {
                app.setup_intent(SetupIntent::Install {
                    startup: app.setup.startup,
                })
            }
            KeyCode::Char('s') if app.route == Route::Settings => {
                app.setup_intent(SetupIntent::Start)
            }
            KeyCode::Char('R') if app.route == Route::Settings => {
                app.setup_intent(SetupIntent::Restart)
            }
            KeyCode::Char('x') if app.route == Route::Settings => {
                app.setup_intent(SetupIntent::Stop)
            }
            KeyCode::Char('a') if app.route == Route::Settings => {
                app.setup_intent(SetupIntent::SetStartup(!app.setup.startup))
            }
            KeyCode::Char('d') if app.route == Route::Settings => {
                if let Some(client) = &mut app.client {
                    client.dispatch(Intent::Diagnostics);
                }
            }
            KeyCode::PageDown if matches!(app.route, Route::History | Route::Devices) => {
                detail_scroll = detail_scroll.saturating_add(8)
            }
            KeyCode::PageUp if matches!(app.route, Route::History | Route::Devices) => {
                detail_scroll = detail_scroll.saturating_sub(8)
            }
            KeyCode::Up | KeyCode::Down | KeyCode::Enter => {
                detail_scroll = 0;
                if app.setup.state == SetupState::ChooseConfiguration {
                    if key.code == KeyCode::Up {
                        choice = choice.saturating_sub(1);
                    }
                    if key.code == KeyCode::Down {
                        choice = (choice + 1).min(app.setup.choices.len().saturating_sub(1));
                    }
                    if key.code == KeyCode::Enter {
                        if let Some(path) = app.setup.choices.get(choice) {
                            app.setup_intent(SetupIntent::Choose(path.clone()));
                        }
                    }
                } else if app.route == Route::History {
                    if let Some(client) = &mut app.client {
                        let current = client
                            .view
                            .history
                            .iter()
                            .position(|row| Some(row.id) == client.view.selected)
                            .unwrap_or(0);
                        let index = match key.code {
                            KeyCode::Up => current.saturating_sub(1),
                            KeyCode::Down => {
                                (current + 1).min(client.view.history.len().saturating_sub(1))
                            }
                            _ => current,
                        };
                        if let Some(row) = client.view.history.get(index) {
                            client.dispatch(Intent::Select(row.id));
                        }
                    }
                }
            }
            _ => {}
        }
        if app.route != prior_route {
            detail_scroll = 0;
        }
    }
    terminal.show_cursor()?;
    Ok(())
}
/// Overall health for the header, shown on every screen.
fn status(app: &Companion) -> (Tone, &'static str, String) {
    let Some(client) = &app.client else {
        return match app.setup.state {
            SetupState::Checking => (Tone::Quiet, "◌", "Checking setup".into()),
            SetupState::Fresh => (Tone::Warn, "●", "Not set up yet".into()),
            SetupState::ChooseConfiguration => (Tone::Warn, "●", "Choose a configuration".into()),
            SetupState::NeedsRepair => (Tone::Bad, "●", "Setup needs repair".into()),
            SetupState::Ready => (Tone::Bad, "●", Connection::Unavailable.label().into()),
        };
    };
    let view = &client.view;
    match view.connection {
        Connection::Unavailable | Connection::UpgradeRequired => {
            (Tone::Bad, "●", view.connection.label().into())
        }
        Connection::Checking => (Tone::Quiet, "◌", view.connection.label().into()),
        _ if view.stale => (Tone::Warn, "◌", "Waiting for the service".into()),
        Connection::Paused => (Tone::Warn, "‖", view.connection.label().into()),
        Connection::Ready => (Tone::Warn, "●", "Ready · no devices connected".into()),
        Connection::Connected(count) => {
            let who = match view.devices.as_slice() {
                [device] => view.device_name(device.id),
                _ => format!("{count} devices"),
            };
            (Tone::Good, "●", format!("Connected · {who}"))
        }
    }
}

fn header(frame: &mut ratatui::Frame, area: Rect, app: &Companion, theme: &Theme) {
    let (tone, icon, text) = status(app);
    let pill = Span::styled(
        format!(" {icon} {text} "),
        match tone {
            Tone::Warn | Tone::Bad => theme.badge(tone),
            _ => theme.tone(tone),
        },
    );
    let pill_width = pill.width() as u16 + 1;
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    let top =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(pill_width)]).split(rows[0]);
    let mut brand = vec![Span::styled(" ◆ ClipSync", theme.heading())];
    let tagline = "  Your clipboard, everywhere";
    if usize::from(top[0].width) > 11 + tagline.len() {
        brand.push(Span::styled(tagline, theme.muted()));
    }
    frame.render_widget(Paragraph::new(Line::from(brand)), top[0]);
    frame.render_widget(Paragraph::new(Line::from(pill)), top[1]);
    let mut tabs = vec![Span::raw(" ")];
    for (index, route) in Route::ALL.iter().enumerate() {
        if *route == app.route {
            tabs.push(Span::styled(
                format!(" {} {} ", index + 1, route.label()),
                theme.tab(true),
            ));
        } else {
            tabs.push(Span::styled(format!(" {}", index + 1), theme.key()));
            tabs.push(Span::styled(
                format!(" {} ", route.label()),
                theme.tab(false),
            ));
        }
        tabs.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(tabs)), rows[1]);
}

fn notice<'a>(theme: &Theme, tone: Tone, icon: &str, text: &str) -> Line<'a> {
    let style = theme.tone(tone);
    Line::from(vec![
        Span::styled(format!(" {icon} "), style),
        Span::styled(text.to_owned(), style.remove_modifier(Modifier::BOLD)),
    ])
}

fn notices<'a>(app: &Companion, theme: &Theme) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    if let Some(view) = app.client.as_ref().map(|client| &client.view) {
        if let Some(error) = &view.error {
            lines.push(notice(theme, Tone::Bad, "✗", error));
        } else if let Some(message) = &view.notice {
            let (tone, icon) = if message.contains("aused") {
                (Tone::Warn, "‖")
            } else if message.starts_with("The service restarted") {
                (Tone::Info, "↻")
            } else {
                (Tone::Good, "✓")
            };
            lines.push(notice(theme, tone, icon, message));
        }
    }
    if app.route == Route::Settings {
        if let Some(error) = &app.setup.service_error {
            lines.push(notice(theme, Tone::Warn, "▲", error));
        }
    }
    lines
}

fn hints(app: &Companion, search: bool) -> Vec<(&'static str, &'static str)> {
    let pause = match &app.client {
        Some(client) if client.view.connection == Connection::Paused => ("p", "resume"),
        _ => ("p", "pause"),
    };
    if search {
        return vec![
            ("type", "to filter"),
            ("Enter/Esc", "done"),
            ("Ctrl-C", "quit"),
        ];
    }
    if app.client.is_none() {
        return vec![("1-4", "sections"), ("r", "refresh"), ("q", "quit app")];
    }
    match app.route {
        Route::History => vec![
            ("↑/↓", "select"),
            ("/", "search"),
            ("c", "copy"),
            pause,
            ("1-4", "sections"),
            ("q", "quit app"),
        ],
        Route::Devices => vec![
            ("1-4", "sections"),
            ("Tab", "next"),
            ("PgUp/PgDn", "scroll"),
            pause,
            ("r", "refresh"),
            ("q", "quit app"),
        ],
        Route::Overview | Route::Settings => vec![
            ("1-4", "sections"),
            ("Tab", "next"),
            pause,
            ("r", "refresh"),
            ("q", "quit app"),
        ],
    }
}

fn rows(lines: &[Line], width: u16) -> u16 {
    let width = usize::from(width.max(1));
    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width) as u16)
        .sum()
}

fn render(
    frame: &mut ratatui::Frame,
    app: &Companion,
    search: bool,
    choice: usize,
    theme: &Theme,
    detail_scroll: u16,
    devices: &super::terminal_devices::DevicesUi,
    graphics: &mut super::terminal_image::GraphicsPreview,
) {
    let area = frame.area();
    let mut footer = notices(app, theme);
    let notice_rows = rows(&footer, area.width).min(3);
    footer.push(theme.hints(&hints(app, search), area.width));
    let sections = Layout::vertical([
        Constraint::Length(if area.height >= 24 { 3 } else { 2 }),
        Constraint::Min(4),
        Constraint::Length(notice_rows + 1),
    ])
    .split(area);
    header(frame, sections[0], app, theme);
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: false }),
        sections[2],
    );
    let body = sections[1];
    let Some(client) = &app.client else {
        setup(frame, body, app, choice, theme);
        return;
    };
    let view = &client.view;
    match app.route {
        Route::Overview => overview(frame, body, view, theme),
        Route::Devices => frame.render_widget(
            Paragraph::new(devices.lines(client, theme))
                .scroll((detail_scroll, 0))
                .wrap(Wrap { trim: false })
                .block(
                    theme
                        .block(" Devices ", false)
                        .title_bottom(
                            Line::from(vec![
                                Span::raw(" "),
                                Span::styled("PgUp/PgDn", theme.key()),
                                Span::styled(" scroll ", theme.muted()),
                            ])
                            .right_aligned(),
                        )
                        .padding(Padding::horizontal(1)),
                ),
            body,
        ),
        Route::History => super::terminal_history::Pane {
            view,
            theme,
            search,
            scroll: detail_scroll,
            color: theme.color(),
        }
        .render(frame, body, graphics),
        Route::Settings => settings(frame, body, app, view, theme),
    }
}

fn section<'a>(theme: &Theme, title: &str) -> [Line<'a>; 2] {
    [
        Line::default(),
        Line::styled(title.to_owned(), theme.heading()),
    ]
}

fn shortcut<'a>(theme: &Theme, key: &str, label: &str) -> Line<'a> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(key.to_owned(), theme.key()),
        Span::raw(format!("  {label}")),
    ])
}

fn overview(frame: &mut ratatui::Frame, area: Rect, view: &View, theme: &Theme) {
    let tone = match view.connection {
        Connection::Connected(_) if !view.stale => Tone::Good,
        Connection::Unavailable | Connection::UpgradeRequired => Tone::Bad,
        Connection::Checking => Tone::Quiet,
        _ => Tone::Warn,
    };
    let explanation = match view.connection {
        Connection::Paused => "Paused until the service restarts. History copies stay local.",
        Connection::Unavailable => {
            "ClipSync cannot reach the background service. Open Settings (4) to start or repair it."
        }
        Connection::UpgradeRequired => {
            "The background service is older than this app. Repair it from Settings (4)."
        }
        Connection::Checking => "Contacting the background service…",
        _ => "The daemon shares new copied text with authenticated connected devices.",
    };
    let icon = if view.connection == Connection::Paused {
        "‖"
    } else {
        "●"
    };
    let mut lines = vec![
        Line::default(),
        Line::from(vec![
            Span::styled(format!("{icon} "), theme.tone(tone)),
            Span::styled(view.connection.label(), theme.tone(tone)),
        ]),
        Line::from(format!("  {explanation}")),
    ];
    let count = view.devices.len();
    lines.extend(section(
        theme,
        &format!(
            "Devices · {count} connected device{}",
            if count == 1 { "" } else { "s" }
        ),
    ));
    if view.devices.is_empty() {
        lines.push(Line::styled(
            "  No devices are connected right now. New copies stay on this computer.",
            theme.muted(),
        ));
    }
    for device in &view.devices {
        lines.push(Line::from(vec![
            Span::styled("  ● ", theme.tone(Tone::Good)),
            Span::styled(view.device_name(device.id), theme.device(device.id)),
            Span::styled(format!(" · {}", device.address), theme.muted()),
        ]));
    }
    lines.extend(section(theme, "Go to"));
    lines.push(shortcut(theme, "2", "Add or manage devices"));
    lines.push(shortcut(theme, "3", "Browse clipboard history"));
    lines.push(shortcut(theme, "4", "Setup and background service"));
    lines.push(Line::default());
    lines.push(if view.stale {
        Line::styled(
            "◌ Status is stale. Waiting for the service.",
            theme.tone(Tone::Warn),
        )
    } else {
        Line::styled("Status is current.", theme.muted())
    });
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            theme
                .block(" Overview ", false)
                .padding(Padding::horizontal(1)),
        ),
        area,
    );
}

fn field<'a>(theme: &Theme, label: &str, value: Vec<Span<'a>>) -> Line<'a> {
    let mut spans = vec![Span::styled(format!("  {label:<9}"), theme.muted())];
    spans.extend(value);
    Line::from(spans)
}

fn flag<'a>(theme: &Theme, on: bool, yes: &str, no: &str) -> Vec<Span<'a>> {
    if on {
        vec![
            Span::styled("● ", theme.tone(Tone::Good)),
            Span::raw(yes.to_owned()),
        ]
    } else {
        vec![
            Span::styled("○ ", theme.muted()),
            Span::styled(no.to_owned(), theme.muted()),
        ]
    }
}

/// Keep a long path on one line by eliding leading directories.
fn fit_path(path: &str, width: usize) -> String {
    let length = path.chars().count();
    if length <= width {
        return path.to_owned();
    }
    let tail: String = path.chars().skip(length + 1 - width.max(2)).collect();
    match tail.find('/') {
        Some(slash) if slash + 1 < tail.len() => format!("…{}", &tail[slash..]),
        _ => format!("…{tail}"),
    }
}

fn settings(frame: &mut ratatui::Frame, area: Rect, app: &Companion, view: &View, theme: &Theme) {
    let setup = &app.setup;
    let room = usize::from(area.width.saturating_sub(15));
    let path = |path: Option<&std::path::Path>| {
        vec![Span::raw(fit_path(
            &path
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            room,
        ))]
    };
    let mut service = flag(theme, setup.installed, "Installed", "Not installed");
    service.push(Span::raw("   "));
    service.extend(flag(theme, setup.running, "Running", "Stopped"));
    service.push(Span::raw("   "));
    service.extend(flag(
        theme,
        setup.startup,
        "Starts at login",
        "Login startup off",
    ));
    let mut lines = vec![
        Line::styled("Setup", theme.heading()),
        Line::from(format!("  {}", setup.message)),
        field(theme, "Config", path(setup.config_path.as_deref())),
        field(theme, "Daemon", path(Some(&setup.daemon_binary))),
        field(theme, "Service", service),
    ];
    lines.extend(section(theme, "Actions"));
    let key = |text: &'static str| Span::styled(text, theme.key());
    lines.push(Line::from(vec![
        Span::raw("  "),
        key("i"),
        Span::raw(" install/repair   "),
        key("s"),
        Span::raw(" start   "),
        key("x"),
        Span::raw(" stop   "),
        key("R"),
        Span::raw(" restart   "),
        key("a"),
        Span::raw(" login startup"),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        key("d"),
        Span::raw(" show redacted diagnostics"),
    ]));
    lines.push(Line::styled(
        "  Closing this app leaves the service running.",
        theme.muted(),
    ));
    if let Some(report) = &view.diagnostics {
        lines.extend(section(theme, "Diagnostics"));
        lines.extend(report.lines().map(|line| Line::from(format!("  {line}"))));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            theme
                .block(" Settings ", false)
                .padding(Padding::new(1, 1, 1, 0)),
        ),
        area,
    );
}

fn setup(frame: &mut ratatui::Frame, area: Rect, app: &Companion, choice: usize, theme: &Theme) {
    let mut lines = vec![Line::from(app.setup.message.as_str()), Line::default()];
    if app.setup.state == SetupState::Fresh {
        lines.push(shortcut(theme, "n", "Set up this computer"));
    }
    for (index, path) in app.setup.choices.iter().enumerate() {
        let selected = index == choice;
        lines.push(
            Line::from(vec![
                if selected {
                    theme.selection_bar()
                } else {
                    Span::raw(" ")
                },
                Span::raw(format!(" {}", path.display())),
            ])
            .style(if selected {
                theme.selection().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            }),
        );
    }
    if !app.setup.choices.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled("↑/↓", theme.key()),
            Span::styled(" choose · ", theme.muted()),
            Span::styled("Enter", theme.key()),
            Span::styled(" use configuration", theme.muted()),
        ]));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).block(
            theme
                .block(" Setup ", true)
                .padding(Padding::new(1, 1, 1, 0)),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_keep_their_file_name() {
        let path = "/Users/me/projects/clipsync/target/debug/clipsync";
        assert_eq!(fit_path(path, 60), path);
        assert_eq!(fit_path(path, 30), "…/target/debug/clipsync");
        assert_eq!(fit_path("/abcdefghijklmnop", 5), "…mnop");
    }
}
