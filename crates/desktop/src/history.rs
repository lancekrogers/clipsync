//! Real history, read and copied by the daemon through the shared controller.
use crate::theme;
use clipsync::companion::client::{Controller, Intent, View};
use eframe::egui::{self, RichText};

pub fn show(ui: &mut egui::Ui, controller: &mut Controller) {
    let view = &controller.view;
    let mut actions = Vec::new();
    let mut query = view.query.clone();
    egui::Frame::new()
        .fill(theme::SURFACE)
        .corner_radius(12)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let reserve = if query.is_empty() { 0.0 } else { 66.0 };
                let response = ui.add(
                    egui::TextEdit::singleline(&mut query)
                        .id_salt("history_search")
                        .hint_text("Search your copied text…")
                        .frame(egui::Frame::NONE)
                        .desired_width((ui.available_width() - reserve).max(100.0)),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::Role::TextInput,
                        true,
                        "Search clipboard history",
                    )
                });
                if !query.is_empty() && ui.small_button("Clear").clicked() {
                    query.clear();
                }
            });
        });
    if query != view.query {
        actions.push(Intent::Search(query));
    }
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.weak(format!(
            "{} saved {}",
            view.history.len(),
            if view.history.len() == 1 {
                "copy"
            } else {
                "copies"
            }
        ));
        if view.history_loading {
            ui.spinner();
        }
    });
    ui.add_space(10.0);
    if view.history.is_empty() {
        theme::card(ui, |ui| {
            ui.label(
                RichText::new(if view.query.is_empty() {
                    "Your recent copies appear here"
                } else {
                    "No matching copies"
                })
                .size(22.0),
            );
            ui.weak(if view.query.is_empty() {
                "Copy some text while ClipSync is running, then return here to find it again."
            } else {
                "Try a word from the copied text, or clear your search."
            });
            if !view.query.is_empty() && ui.button("Clear search").clicked() {
                actions.push(Intent::Search(String::new()));
            }
        });
    } else if ui.available_width() >= 680.0 {
        let list_width = ui.available_width() * 0.47;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(list_width, 440.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| list(ui, view, &mut actions),
            );
            ui.add_space(8.0);
            ui.vertical(|ui| detail(ui, view, &mut actions));
        });
    } else {
        detail(ui, view, &mut actions);
        ui.add_space(18.0);
        list(ui, view, &mut actions);
    }
    for action in actions {
        controller.dispatch(action);
    }
}
fn list(ui: &mut egui::Ui, view: &View, actions: &mut Vec<Intent>) {
    egui::ScrollArea::vertical()
        .id_salt("history_list")
        .max_height(520.0)
        .show(ui, |ui| {
            for (index, entry) in view.history.iter().enumerate() {
                let selected = view.selected == Some(entry.id);
                let label = format!("{}\n{}\n{}", entry.timestamp, entry.preview, entry.source);
                let response = ui.add_sized(
                    [ui.available_width(), 108.0],
                    egui::Button::new(RichText::new(label).size(16.0).color(if selected {
                        theme::TEXT
                    } else {
                        theme::MUTED
                    }))
                    .wrap()
                    .right_text("")
                    .fill(if selected {
                        theme::SELECTED
                    } else {
                        theme::SURFACE
                    })
                    .stroke(egui::Stroke::NONE),
                );
                if response.clicked() {
                    actions.push(Intent::Select(entry.id));
                }
                if response.has_focus() {
                    let target = if ui.input(|input| input.key_pressed(egui::Key::ArrowDown)) {
                        Some((index + 1).min(view.history.len() - 1))
                    } else if ui.input(|input| input.key_pressed(egui::Key::ArrowUp)) {
                        Some(index.saturating_sub(1))
                    } else {
                        None
                    };
                    if let Some(target) = target {
                        actions.push(Intent::Select(view.history[target].id));
                    }
                }
                ui.add_space(6.0);
            }
        });
}
fn detail(ui: &mut egui::Ui, view: &View, actions: &mut Vec<Intent>) {
    theme::card(ui, |ui| {
        ui.label(RichText::new("Selected copy").size(20.0).strong());
        let selected = view
            .history
            .iter()
            .find(|row| Some(row.id) == view.selected);
        if let Some(entry) = selected {
            ui.weak(format!("{} · {}", entry.source, entry.timestamp));
            ui.add_space(16.0);
            if view.detail_loading {
                ui.spinner();
            } else if let Some(text) = view.visible_detail() {
                egui::ScrollArea::vertical()
                    .id_salt("copy_detail")
                    .max_height(250.0)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(text).size(17.0)).wrap());
                    });
            }
            if view.detail_pages() > 1 {
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(view.detail_page > 0, egui::Button::new("Previous page"))
                        .clicked()
                    {
                        actions.push(Intent::DetailPage(view.detail_page.saturating_sub(1)));
                    }
                    ui.weak(format!(
                        "Page {} of {}",
                        view.detail_page + 1,
                        view.detail_pages()
                    ));
                    if ui
                        .add_enabled(
                            view.detail_page + 1 < view.detail_pages(),
                            egui::Button::new("Next page"),
                        )
                        .clicked()
                    {
                        actions.push(Intent::DetailPage(view.detail_page + 1));
                    }
                });
            }
            ui.add_space(16.0);
            ui.weak(format!("{} characters · Plain text", entry.characters));
            if ui
                .add_enabled(
                    view.can_act() && view.detail.is_some() && !view.detail_loading,
                    egui::Button::new(RichText::new("Copy text").color(theme::BG))
                        .fill(theme::BLUE)
                        .min_size(egui::vec2(140.0, 44.0)),
                )
                .clicked()
            {
                actions.push(Intent::CopySelected);
            }
            ui.weak("Copy puts the original text on this computer’s clipboard.");
        } else {
            ui.weak("Choose a saved copy to read it and copy it again.");
        }
    });
}
