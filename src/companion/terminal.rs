//! Optional terminal renderer for the same companion controller as the GUI.
use super::{
    app::{Companion, SetupIntent, SetupState},
    client::Intent,
    model::{Connection, Route},
};
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, ListState, Paragraph, Wrap},
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
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut devices = super::terminal_devices::DevicesUi::default();
    let mut search = false;
    let mut choice = 0usize;
    let mut detail_scroll = 0u16;
    let color = std::env::var_os("NO_COLOR").is_none();
    loop {
        app.tick();
        terminal
            .draw(|frame| render(frame, &app, search, choice, color, detail_scroll, &devices))?;
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
fn block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
}
fn render(
    frame: &mut ratatui::Frame,
    app: &Companion,
    search: bool,
    choice: usize,
    color: bool,
    detail_scroll: u16,
    devices: &super::terminal_devices::DevicesUi,
) {
    let accent = if color {
        Style::default().fg(Color::Rgb(105, 191, 255))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(4),
        ])
        .split(frame.area());
    let navigation = Route::ALL
        .iter()
        .enumerate()
        .map(|(index, route)| {
            Span::styled(
                format!(" {} {} ", index + 1, route.label()),
                if *route == app.route {
                    accent.add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(navigation))
            .block(block("ClipSync · Your clipboard, everywhere")),
        sections[0],
    );
    let mut notices = vec![Line::from(if search {
        "Search: type to filter · Enter/Esc finish · Ctrl-C quit"
    } else {
        "1–4 sections · Tab next · p pause/resume · r refresh · q quit app"
    })];
    if let Some(client) = &app.client {
        if let Some(message) = client.view.error.as_ref().or(client.view.notice.as_ref()) {
            notices.push(Line::from(message.as_str()));
        }
    }
    if app.route == Route::Settings {
        if let Some(error) = &app.setup.service_error {
            notices.push(Line::from(error.as_str()));
        }
    }
    frame.render_widget(
        Paragraph::new(notices).wrap(Wrap { trim: false }),
        sections[2],
    );
    if app.client.is_none() {
        let mut lines = vec![Line::from(app.setup.message.as_str()), Line::from("")];
        if app.setup.state == SetupState::Fresh {
            lines.push(Line::from("n  Set up this computer"));
        }
        for (index, path) in app.setup.choices.iter().enumerate() {
            lines.push(Line::from(format!(
                "{} {}",
                if index == choice { ">" } else { " " },
                path.display()
            )));
        }
        if !app.setup.choices.is_empty() {
            lines.push(Line::from("↑/↓ choose · Enter use configuration"));
        }
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(block("Setup")),
            sections[1],
        );
        return;
    }
    let client = app.client.as_ref().unwrap();
    match app.route {
        Route::Overview => {
            let lines = vec![
                Line::styled(client.view.connection.label(), accent),
                Line::from(""),
                Line::from(if client.view.connection == Connection::Paused {
                    "Paused until the service restarts. History copies stay local."
                } else {
                    "The daemon shares new copied text with authenticated connected devices."
                }),
                Line::from(format!("{} connected device(s)", client.view.devices.len())),
                Line::from(""),
                Line::from("3  Browse clipboard history"),
                Line::from("4  Setup and background service"),
                Line::from(if client.view.stale {
                    "Status is stale. Waiting for the service."
                } else {
                    "Status is current."
                }),
            ];
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(block("Overview")),
                sections[1],
            );
        }
        Route::Devices => {
            frame.render_widget(
                Paragraph::new(devices.lines(client))
                    .scroll((detail_scroll, 0))
                    .wrap(Wrap { trim: false })
                    .block(block("Devices · PgUp/PgDn scroll")),
                sections[1],
            );
        }
        Route::History => {
            let columns = Layout::default()
                .direction(if sections[1].width >= 90 {
                    Direction::Horizontal
                } else {
                    Direction::Vertical
                })
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(sections[1]);
            let rows = client
                .view
                .history
                .iter()
                .map(|entry| {
                    let selected = Some(entry.id) == client.view.selected;
                    ListItem::new(vec![
                        Line::from(format!(
                            "{} {}",
                            if selected { ">" } else { " " },
                            entry.preview
                        )),
                        Line::from(format!("  {} · {}", entry.source, entry.timestamp)),
                    ])
                    .style(if selected { accent } else { Style::default() })
                })
                .collect::<Vec<_>>();
            let title = format!("History · / search: {}", client.view.query);
            let mut selection = ListState::default().with_selected(
                client
                    .view
                    .history
                    .iter()
                    .position(|row| Some(row.id) == client.view.selected),
            );
            frame.render_stateful_widget(
                List::new(rows).block(block(&title)),
                columns[0],
                &mut selection,
            );
            let detail = client.view.visible_detail().unwrap_or(if client.view.history.is_empty() { "No copies match your search. Clear the query or copy text while the service is running." } else { "↑/↓ select a saved copy. Press c to copy it again." });
            let detail_title = if client.view.detail_loading {
                "Loading selected copy…"
            } else if client.view.can_act() && client.view.detail.is_some() {
                "Selected copy · c copy · PgUp/PgDn"
            } else {
                "Selected copy · Copy unavailable · PgUp/PgDn"
            };
            let title = if client.view.detail_pages() > 1 {
                format!(
                    "{} · [ ] page {}/{}",
                    detail_title,
                    client.view.detail_page + 1,
                    client.view.detail_pages()
                )
            } else {
                detail_title.to_owned()
            };
            frame.render_widget(
                Paragraph::new(detail)
                    .scroll((detail_scroll, 0))
                    .wrap(Wrap { trim: false })
                    .block(block(&title)),
                columns[1],
            );
        }
        Route::Settings => {
            let mut lines = vec![
                Line::from(app.setup.message.as_str()),
                Line::from(format!(
                    "Config: {}",
                    app.setup
                        .config_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default()
                )),
                Line::from(format!("Daemon: {}", app.setup.daemon_binary.display())),
                Line::from(format!(
                    "Installed: {} · Start at login: {}",
                    app.setup.installed, app.setup.startup
                )),
                Line::from(""),
                Line::from("i install/repair · s start · x stop · R restart · a login startup"),
                Line::from(
                    "d show redacted diagnostics · Closing this app leaves the service running.",
                ),
            ];
            if let Some(report) = &client.view.diagnostics {
                lines.push(Line::from(report.as_str()));
            }
            frame.render_widget(
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .block(block("Settings")),
                sections[1],
            );
        }
    }
}
