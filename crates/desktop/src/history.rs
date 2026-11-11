//! Real history, read and copied by the daemon through the shared controller.
use crate::theme;
use clipsync::companion::client::{Controller, Intent, View};
use clipsync::control::{ImagePreview, decode_preview_png};
use eframe::egui::{self, RichText};
use std::sync::{Arc, OnceLock};
use uuid::Uuid;

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
            let texture = view
                .image_preview
                .as_ref()
                .filter(|_| !view.detail_loading)
                .map(|preview| preview_texture(ui.ctx(), entry.id, preview));
            if view.detail_loading || texture.as_ref().is_some_and(Option::is_none) {
                ui.spinner();
            } else if let Some(Some(Some(texture))) = &texture {
                image_preview(ui, texture);
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
            let image = entry.content_type == "image/png";
            if image {
                ui.weak("Image");
            } else {
                ui.weak(format!("{} characters · Plain text", entry.characters));
            }
            if ui
                .add_enabled(
                    view.can_act() && view.detail.is_some() && !view.detail_loading,
                    egui::Button::new(
                        RichText::new(if image { "Copy image" } else { "Copy text" })
                            .color(theme::BG),
                    )
                    .fill(theme::BLUE)
                    .min_size(egui::vec2(140.0, 44.0)),
                )
                .clicked()
            {
                actions.push(Intent::CopySelected);
            }
            ui.weak(if image {
                "Copy puts the image on this computer’s clipboard."
            } else {
                "Copy puts the original text on this computer’s clipboard."
            });
        } else {
            ui.weak("Choose a saved copy to read it and copy it again.");
        }
    });
}

fn image_preview(ui: &mut egui::Ui, texture: &egui::TextureHandle) {
    let [width, height] = texture.size();
    // Never enlarge past native pixels, except the 64px fallback thumbnail.
    let scale = if width.max(height) <= 64 {
        4.0
    } else {
        1.0 / ui.pixels_per_point()
    };
    ui.add(
        egui::Image::new(texture)
            .fit_to_original_size(scale)
            .max_size(egui::vec2(ui.available_width(), 360.0))
            .corner_radius(8)
            .alt_text(format!("Copied image preview, {width} by {height} pixels")),
    );
}

#[derive(Clone)]
struct CachedPreview {
    key: (Uuid, u32, u32, usize),
    texture: Arc<OnceLock<Option<egui::TextureHandle>>>,
}

/// Decode a selection's preview off the UI thread, once per selection.
/// `None` while decoding; `Some(None)` when no preview can be shown.
fn preview_texture(
    ctx: &egui::Context,
    id: Uuid,
    preview: &ImagePreview,
) -> Option<Option<egui::TextureHandle>> {
    let key = (
        id,
        preview.width,
        preview.height,
        preview.png.as_ref().map_or(0, String::len),
    );
    let slot = egui::Id::unique("history_image_preview");
    if let Some(cached) = ctx.data(|data| data.get_temp::<CachedPreview>(slot))
        && cached.key == key
    {
        return cached.texture.get().cloned();
    }
    let texture = Arc::new(OnceLock::new());
    ctx.data_mut(|data| {
        data.insert_temp(
            slot,
            CachedPreview {
                key,
                texture: texture.clone(),
            },
        )
    });
    let (ctx, preview) = (ctx.clone(), preview.clone());
    std::thread::spawn(move || {
        let image = match decode_preview_png(&preview) {
            Some(png) => Some(egui::ColorImage::from_rgba_unmultiplied(
                [png.width as usize, png.height as usize],
                &png.rgba,
            )),
            None => preview.is_valid().then(|| {
                egui::ColorImage::from_rgb(
                    [preview.width as usize, preview.height as usize],
                    &preview.rgb,
                )
            }),
        };
        let options = egui::TextureOptions {
            mipmap_mode: Some(egui::TextureFilter::Linear),
            ..egui::TextureOptions::LINEAR
        };
        let _ = texture.set(
            image.map(|image| ctx.load_texture(format!("history-image-{id}"), image, options)),
        );
        ctx.request_repaint();
    });
    None
}
