//! Device enrollment stays in the daemon; this view only renders verified state.
use crate::theme;
use clipsync::companion::{
    client::{Controller, Intent},
    model::enrollment_state_label,
};
use eframe::egui;

pub fn show(ui: &mut egui::Ui, client: &mut Controller) {
    let view = &client.view;
    let mut actions = Vec::new();
    theme::card(ui, |ui| {
        ui.heading("Connect another computer");
        ui.weak("Open ClipSync on both computers. Allow connections, then compare the full fingerprints on both screens.");
        if !view.enrollment_supported {
            ui.label("Update the background service to add devices here.");
            return;
        }
        ui.add_enabled_ui(view.can_act(), |ui| {
            ui.horizontal_wrapped(|ui| {
                if theme::primary(ui, "Allow a new device").clicked() {
                    actions.push(Intent::OpenEnrollment);
                }
                if ui.button("Find nearby devices").clicked() {
                    actions.push(Intent::DiscoverEnrollment);
                }
            });
            if let Some(snapshot) = &view.enrollment {
                if snapshot.listening {
                    ui.colored_label(
                        theme::TEAL,
                        "Ready for a new device · expires automatically",
                    );
                    if let Some(address) = &snapshot.listen_addr {
                        ui.label(format!("Listening on {address}"));
                    }
                }
                for candidate in &snapshot.candidates {
                    if ui
                        .button(format!(
                            "Connect to {} · {}",
                            candidate.display_name, candidate.address
                        ))
                        .clicked()
                    {
                        actions.push(Intent::DialEnrollment {
                            address: candidate.address.clone(),
                            expected_fingerprint: candidate.advertised_fingerprint.clone(),
                            expected_node_id: candidate.advertised_node_id,
                        });
                    }
                }
            }
            ui.add_space(10.0);
            ui.label("Or enter the other computer’s IP address and enrollment port");
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
                    !address.trim().is_empty(),
                    egui::Button::new("Connect to address"),
                )
                .clicked()
            {
                actions.push(Intent::DialEnrollment {
                    address,
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
                ui.heading(
                    session
                        .peer_display_name
                        .as_deref()
                        .unwrap_or("Device verification"),
                );
                let pending = matches!(
                    session.state.as_str(),
                    "AwaitingLocalConfirm" | "AwaitingPeerConfirm"
                );
                ui.label(enrollment_state_label(&session.state));
                if pending {
                    ui.weak(format!("{} seconds remaining", session.remaining_secs));
                }
                ui.label("This computer’s fingerprint");
                ui.add(
                    egui::Label::new(egui::RichText::new(&session.local_fingerprint).monospace())
                        .wrap(),
                );
                ui.label("Other computer’s fingerprint");
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            session
                                .peer_fingerprint
                                .as_deref()
                                .unwrap_or("Waiting for a secure connection…"),
                        )
                        .monospace(),
                    )
                    .wrap(),
                );
                ui.weak("Each fingerprint must match the corresponding computer on the other screen. Device names alone do not prove identity.");
                if let Some(error) = &session.error {
                    ui.label(error);
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(
                            view.can_act()
                                && pending
                                && !session.local_confirmed
                                && session.peer_fingerprint.is_some()
                                && session.remaining_secs > 0,
                            egui::Button::new("Fingerprints match — confirm"),
                        )
                        .clicked()
                    {
                        actions.push(Intent::ConfirmEnrollment(session.session_id));
                    }
                    if session.local_confirmed && pending {
                        ui.label("Confirmed here; waiting for both computers.");
                    }
                    if ui
                        .add_enabled(
                            view.can_act()
                                && !matches!(
                                    session.state.as_str(),
                                    "Succeeded" | "Cancelled" | "Expired" | "Failed"
                                ),
                            egui::Button::new("Cancel connection"),
                        )
                        .clicked()
                    {
                        actions.push(Intent::CancelEnrollment(Some(session.session_id)));
                    }
                });
            });
        }
        if let Some(outcome) = &snapshot.outcome {
            ui.label(format!(
                "{}: {}",
                outcome.status,
                outcome.detail.as_deref().unwrap_or_default()
            ));
        }
        if snapshot.listening && ui.button("Stop allowing new devices").clicked() {
            actions.push(Intent::CancelEnrollment(None));
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
        ui.weak("Devices you confirm will appear here, including when they are offline.");
    }
    for device in &view.trusted_devices {
        theme::card(ui, |ui| {
            ui.label(device.comment.as_deref().unwrap_or("Authorized device"));
            ui.monospace(&device.fingerprint);
            ui.add_enabled_ui(view.can_act(), |ui| {
                ui.menu_button("Remove authorization…", |ui| {
                    ui.label("This device will need to be verified again.");
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
