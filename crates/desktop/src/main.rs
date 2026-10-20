//! Native shell; application behavior lives in clipsync::companion.
mod devices;
mod history;
mod theme;
use clap::Parser;
use clipsync::companion::{
    app::{Companion, SetupIntent, SetupState},
    client::Intent,
    model::{Connection, Route},
};
use eframe::egui;
use std::{path::PathBuf, time::Duration};
use tray_icon::{
    TrayIcon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem},
};

#[derive(Parser)]
struct Options {
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    daemon_binary: Option<PathBuf>,
    #[arg(long)]
    no_tray: bool,
}
struct App {
    companion: Companion,
    tray: Option<TrayIcon>,
    menu: Option<(String, String, String)>,
    menu_events: Option<std::sync::mpsc::Receiver<MenuEvent>>,
    no_tray: bool,
    initialized: bool,
    quitting: bool,
}
impl App {
    fn tray(&mut self, ctx: &egui::Context) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        if self.no_tray {
            return;
        }
        let menu = Menu::new();
        let open = MenuItem::new("Open ClipSync", true, None);
        let pause = MenuItem::new("Pause / resume syncing", true, None);
        let quit = MenuItem::new("Quit ClipSync app", true, None);
        if menu.append_items(&[&open, &pause, &quit]).is_err() {
            return;
        }
        let ids = (
            open.id().0.clone(),
            pause.id().0.clone(),
            quit.id().0.clone(),
        );
        let mut rgba = vec![0; 32 * 32 * 4];
        for y in 3..29 {
            for x in 6..26 {
                if ((x == 6 || x == 7 || x == 24 || x == 25) && y >= 7)
                    || ((y == 7 || y == 8 || y == 27 || y == 28) && !(12..20).contains(&x))
                    || ((3..8).contains(&y) && (12..20).contains(&x))
                    || ((y == 14 || y == 19 || y == 24) && (11..21).contains(&x))
                {
                    let index = (y * 32 + x) * 4;
                    rgba[index..index + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
        let Ok(icon) = tray_icon::Icon::from_rgba(rgba, 32, 32) else {
            return;
        };
        let builder = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ClipSync");
        #[cfg(target_os = "macos")]
        let builder = builder.with_icon_templated(icon);
        #[cfg(not(target_os = "macos"))]
        let builder = builder.with_icon(icon);
        if let Ok(tray) = builder.build() {
            let (sender, receiver) = std::sync::mpsc::sync_channel(16);
            let wake = ctx.clone();
            MenuEvent::set_event_handler(Some(move |event| {
                let _ = sender.try_send(event);
                wake.request_repaint();
            }));
            self.menu_events = Some(receiver);
            self.tray = Some(tray);
            self.menu = Some(ids);
        }
    }
    fn setup(&mut self, ui: &mut egui::Ui) {
        let view = self.companion.setup.clone();
        theme::card(ui, |ui| {
            ui.heading(match view.state {
                SetupState::Fresh => "Welcome to ClipSync",
                SetupState::ChooseConfiguration => "Choose your setup",
                SetupState::NeedsRepair => "Your setup needs attention",
                _ => "Your ClipSync setup",
            });
            ui.label(&view.message);
            if let Some(path) = &view.config_path {
                ui.weak(path.display().to_string());
            }
            for warning in &view.warnings {
                ui.weak(warning);
            }
            ui.add_space(16.0);
            ui.add_enabled_ui(!view.pending, |ui| {
                if view.state == SetupState::Fresh
                    && theme::primary(ui, "Set up this computer").clicked()
                {
                    self.companion.setup_intent(SetupIntent::Create);
                }
                for path in &view.choices {
                    if ui.button(path.display().to_string()).clicked() {
                        self.companion
                            .setup_intent(SetupIntent::Choose(path.clone()));
                    }
                }
                if ui.button("Check again").clicked() {
                    self.companion.setup_intent(SetupIntent::Inspect);
                }
            });
            if view.pending {
                ui.spinner();
            }
        });
    }
    fn overview(&mut self, ui: &mut egui::Ui) {
        if self.companion.client.is_none() {
            self.setup(ui);
            return;
        }
        let client = self.companion.client.as_mut().unwrap();
        theme::card(ui, |ui| {
            ui.label(
                egui::RichText::new(client.view.connection.label())
                    .size(30.0)
                    .color(if client.view.can_act() {
                        theme::TEAL
                    } else {
                        theme::MUTED
                    }),
            );
            ui.add_space(8.0);
            ui.label(match client.view.connection {
                Connection::Connected(_) => "New copied text is shared with your connected devices.",
                Connection::Paused => "Paused until the service restarts. Explicit copies from history stay on this computer.",
                Connection::Ready => "The service is running. Connect an authorized device to start sharing copied text.",
                Connection::UpgradeRequired => "Update the ClipSync service to use the companion’s history and pause controls.",
                Connection::Unavailable => "The selected service could not be reached. Check its setup in Settings.",
                Connection::Checking => "Contacting your clipboard service…",
            });
            if client.view.stale {
                ui.weak("Waiting for current service status.");
            }
            ui.add_space(20.0);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        client.view.can_act(),
                        egui::Button::new(client.view.pause_caption()),
                    )
                    .clicked()
                {
                    client.dispatch(Intent::SetPaused(
                        client.view.connection != Connection::Paused,
                    ));
                }
                if ui
                    .add_enabled(
                        client.view.can_act() && client.view.connection != Connection::Paused,
                        egui::Button::new("Sync current copy"),
                    )
                    .clicked()
                {
                    client.dispatch(Intent::SyncNow);
                }
                if ui.button("Refresh").clicked() {
                    client.dispatch(Intent::Refresh);
                }
            });
        });
        if client.view.connection == Connection::Unavailable
            && theme::primary(ui, "Open service settings").clicked()
        {
            self.companion.navigate(Route::Settings);
            return;
        }
        ui.add_space(18.0);
        theme::card(ui, |ui| {
            ui.label(egui::RichText::new("Find something you copied earlier").size(22.0));
            ui.weak("Browse your recent text, links and notes, then copy them again.");
            if theme::primary(ui, "Open clipboard history").clicked() {
                self.companion.navigate(Route::History);
            }
        });
    }
    fn devices(&mut self, ui: &mut egui::Ui) {
        if let Some(client) = &mut self.companion.client {
            devices::show(ui, client);
        } else {
            self.setup(ui);
        }
    }
    fn settings(&mut self, ui: &mut egui::Ui) {
        self.setup(ui);
        ui.add_space(18.0);
        let setup = self.companion.setup.clone();
        theme::card(ui, |ui| {
            ui.heading("Background service");
            ui.weak("Closing this app leaves the clipboard service running.");
            ui.label(format!("Daemon: {}", setup.daemon_binary.display()));
            ui.add_enabled_ui(!setup.pending && setup.state == SetupState::Ready, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .button(if setup.installed {
                            "Repair service"
                        } else {
                            "Install service"
                        })
                        .clicked()
                    {
                        self.companion.setup_intent(SetupIntent::Install {
                            startup: setup.startup,
                        });
                    }
                    if ui
                        .add_enabled(setup.installed, egui::Button::new("Start service"))
                        .clicked()
                    {
                        self.companion.setup_intent(SetupIntent::Start);
                    }
                    if ui
                        .add_enabled(setup.installed, egui::Button::new("Stop service"))
                        .clicked()
                    {
                        self.companion.setup_intent(SetupIntent::Stop);
                    }
                    if ui
                        .add_enabled(setup.installed, egui::Button::new("Restart service"))
                        .clicked()
                    {
                        self.companion.setup_intent(SetupIntent::Restart);
                    }
                });
                let mut startup = setup.startup;
                if ui
                    .add_enabled(
                        setup.installed,
                        egui::Checkbox::new(&mut startup, "Start at login"),
                    )
                    .changed()
                {
                    self.companion
                        .setup_intent(SetupIntent::SetStartup(startup));
                }
            });
        });
        ui.add_space(18.0);
        if let Some(client) = &mut self.companion.client {
            theme::card(ui, |ui| {
                ui.heading("Diagnostics");
                ui.weak("Review service details without clipboard contents or private keys.");
                if ui
                    .add_enabled(client.view.can_act(), egui::Button::new("Show diagnostics"))
                    .clicked()
                {
                    client.dispatch(Intent::Diagnostics);
                }
                if let Some(report) = &client.view.diagnostics {
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.monospace(report);
                        });
                }
            });
        }
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.companion.tick();
        self.tray(ctx);
        // logic also runs while hidden; menu events explicitly wake the event loop.
        while let Some(event) = self
            .menu_events
            .as_ref()
            .and_then(|events| events.try_recv().ok())
        {
            if let Some((open, pause, quit)) = &self.menu {
                if event.id.0 == *open {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                if event.id.0 == *pause
                    && let Some(client) = &mut self.companion.client
                {
                    client.dispatch(Intent::SetPaused(
                        client.view.connection != Connection::Paused,
                    ));
                }
                if event.id.0 == *quit {
                    self.quitting = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(target_os = "macos")]
        if !self.quitting
            && ctx.input(|input| input.viewport().close_requested())
            && self.tray.is_some()
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        egui::Panel::top("header")
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin {
                        left: 32,
                        right: 32,
                        top: 42,
                        bottom: 24,
                    }),
            )
            .show(ui, |ui| {
                let title = ui
                    .horizontal_wrapped(|ui| {
                        ui.label(egui::RichText::new("ClipSync").size(32.0).strong());
                        ui.add_space(16.0);
                        ui.weak("Your clipboard, everywhere.");
                    })
                    .response;
                if ui
                    .interact(
                        title.rect,
                        ui.scope_id().with("window_drag"),
                        egui::Sense::drag(),
                    )
                    .drag_started()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });
        egui::Panel::left("navigation")
            .min_size(204.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(24))
            .show(ui, |ui| {
                for route in Route::ALL {
                    if ui
                        .add_sized(
                            [168.0, 48.0],
                            egui::Button::new(route.label())
                                .selected(self.companion.route == route),
                        )
                        .clicked()
                    {
                        self.companion.navigate(route);
                    }
                }
            });
        egui::CentralPanel::default().frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin { left:16, right:32, top:10, bottom:24 })).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("page_scroll", self.companion.route.label())).show(ui, |ui| {
                ui.heading(if self.companion.route == Route::History { "Clipboard history" } else { self.companion.route.label() });
                ui.add_space(14.0);
                if self.companion.route == Route::Settings && let Some(error) = &self.companion.setup.service_error { ui.colored_label(theme::BLUE, error); }
                if let Some(client) = &mut self.companion.client {
                    if let Some(error) = &client.view.error { ui.colored_label(theme::BLUE, error); }
                    if let Some(notice) = &client.view.notice { ui.colored_label(theme::TEAL, notice); }
                    if (client.view.error.is_some() || client.view.notice.is_some()) && ui.small_button("Dismiss").clicked() { client.dispatch(Intent::ClearNotice); }
                }
                ui.add_space(10.0);
                match self.companion.route {
                    Route::Overview => self.overview(ui), Route::Devices => self.devices(ui), Route::Settings => self.settings(ui),
                    Route::History => { if let Some(client) = &mut self.companion.client { ui.weak("Find a recent copy. Select it to read the full text and copy it again."); ui.add_space(18.0); history::show(ui, client); } else { self.setup(ui); } }
                }
            });
        });
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}
fn main() -> eframe::Result {
    let options = Options::parse();
    #[cfg(not(feature = "ui-test"))]
    if std::env::var_os("EGUI_INSPECTION").is_some() {
        eprintln!("UI inspection requires an isolated ui-test build");
    }
    let companion = Companion::new(options.config, options.daemon_binary)
        .map_err(|error| eframe::Error::AppCreation(error.into()))?;
    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1080.0, 800.0])
        .with_min_inner_size([800.0, 640.0]);
    #[cfg(target_os = "macos")]
    let viewport = viewport
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_titlebar_buttons_shown(true);
    eframe::run_native(
        "ClipSync",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |creation| {
            theme::install(&creation.egui_ctx);
            Ok(Box::new(App {
                companion,
                tray: None,
                menu: None,
                menu_events: None,
                no_tray: options.no_tray,
                initialized: false,
                quitting: false,
            }))
        }),
    )
}
