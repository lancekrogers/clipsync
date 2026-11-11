//! Pairing stays in the daemon; this view shows the code or collects it.
use crate::theme;
use clipsync::companion::{
    client::{Controller, Intent},
    model::{countdown, enrollment_state_label, friendly_device_name},
};
use eframe::egui;

pub fn show(ui: &mut egui::Ui, client: &mut Controller) {
    let view = &client.view;
    let mut actions = Vec::new();
    theme::card(ui, |ui| {
        ui.heading("Pair another computer");
        ui.weak("On one computer choose Allow a new device. On the other, pick it below and type the code it shows.");
        if !view.enrollment_supported {
            ui.label("Update the background service to add devices here.");
            return;
        }
        if !view.pairing_supported {
            ui.label("Update the background service to pair with a code.");
            return;
        }
        let snapshot = view.enrollment.as_ref();
        ui.add_enabled_ui(view.can_act(), |ui| {
            match snapshot.and_then(|s| s.code.as_ref()) {
                Some(code) => {
                    ui.label("Pairing code");
                    ui.label(
                        egui::RichText::new(code.code.grouped())
                            .monospace()
                            .size(44.0)
                            .strong()
                            .color(theme::TEAL),
                    );
                    ui.label(format!(
                        "Type it on the other computer · {}",
                        countdown(code.remaining_secs)
                    ));
                    ui.weak(format!(
                        "Each code works once. {} left in this window.",
                        code.attempts_left
                    ));
                    if let Some(address) = snapshot.and_then(|s| {
                        s.listen_hints
                            .first()
                            .cloned()
                            .or_else(|| s.listen_addr.clone())
                    }) {
                        ui.weak(format!("If it is not listed there, use address {address}"));
                    }
                    if ui.button("Stop allowing").clicked() {
                        actions.push(Intent::CancelEnrollment(None));
                    }
                }
                None if snapshot.is_some_and(|s| s.listening) => {
                    ui.colored_label(theme::TEAL, "Checking a code someone typed…");
                }
                None => {
                    if theme::primary(ui, "Allow a new device").clicked() {
                        actions.push(Intent::OpenEnrollment);
                    }
                }
            }
            ui.add_space(14.0);
            ui.label("Code shown on the other computer");
            let mut code = view.pairing_code.as_str().to_owned();
            let field = ui.add(
                egui::TextEdit::singleline(&mut code)
                    .hint_text("6 digits")
                    .char_limit(7)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(120.0),
            );
            if field.changed() {
                actions.push(Intent::PairingCode(code));
            }
            let ready = view.pairing_code.is_complete();
            ui.horizontal_wrapped(|ui| {
                if ui.button("Find nearby devices").clicked() {
                    actions.push(Intent::DiscoverEnrollment);
                }
            });
            for candidate in snapshot
                .map(|s| s.candidates.as_slice())
                .unwrap_or_default()
            {
                let name = friendly_device_name(&candidate.display_name)
                    .unwrap_or_else(|| candidate.address.clone());
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(format!("Pair with {name} · {}", candidate.address)),
                    )
                    .clicked()
                {
                    actions.push(Intent::Pair {
                        address: candidate.address.clone(),
                        expected_fingerprint: candidate.advertised_fingerprint.clone(),
                        expected_node_id: candidate.advertised_node_id,
                    });
                }
            }
            ui.add_space(10.0);
            ui.label("Or enter the other computer’s pairing address");
            let mut address = view.device_address.clone();
            let edit = ui.add(
                egui::TextEdit::singleline(&mut address)
                    .hint_text("192.168.1.20:8485")
                    .desired_width(f32::INFINITY),
            );
            if edit.changed() {
                actions.push(Intent::DeviceAddress(address.clone()));
            }
            if ui
                .add_enabled(
                    ready && !address.trim().is_empty(),
                    egui::Button::new("Pair with address"),
                )
                .clicked()
            {
                actions.push(Intent::Pair {
                    address: address.trim().to_owned(),
                    expected_fingerprint: None,
                    expected_node_id: None,
                });
            }
        });
    });
    if let Some(snapshot) = &view.enrollment {
        for session in &snapshot.sessions {
            ui.add_space(16.0);
            theme::card(ui, |ui| {
                let who = session
                    .peer_display_name
                    .as_deref()
                    .and_then(friendly_device_name)
                    .or_else(|| session.dialed_addr.clone())
                    .unwrap_or_else(|| "Another computer".into());
                ui.heading(who);
                ui.label(enrollment_state_label(&session.state));
                if let Some(error) = &session.error {
                    ui.label(error);
                }
                let live = !matches!(
                    session.state.as_str(),
                    "Succeeded" | "Cancelled" | "Expired" | "Failed"
                );
                if live
                    && ui
                        .add_enabled(view.can_act(), egui::Button::new("Cancel pairing"))
                        .clicked()
                {
                    actions.push(Intent::CancelEnrollment(Some(session.session_id)));
                }
            });
        }
        if let Some(outcome) = &snapshot.outcome {
            ui.label(outcome.detail.as_deref().unwrap_or_default());
        }
    }
    ui.add_space(20.0);
    ui.heading("Connected now");
    if view.devices.is_empty() {
        ui.weak("No authenticated devices are connected.");
    }
    for device in &view.devices {
        ui.label(format!("{} · {}", device.name, device.address));
    }
    ui.add_space(20.0);
    ui.heading("Authorized devices");
    if view.trusted_devices.is_empty() {
        ui.weak("Devices you pair will appear here, including when they are offline.");
    }
    for device in &view.trusted_devices {
        theme::card(ui, |ui| {
            ui.label(device.comment.as_deref().unwrap_or("Authorized device"));
            ui.monospace(&device.fingerprint);
            ui.add_enabled_ui(view.can_act(), |ui| {
                ui.menu_button("Remove authorization…", |ui| {
                    ui.label("This device will need to pair again.");
                    if ui.button("Remove this device").clicked() {
                        actions.push(Intent::RevokeDevice(device.fingerprint.clone()));
                        ui.close();
                    }
                });
            });
        });
    }
    for action in actions {
        client.dispatch(action);
    }
}
