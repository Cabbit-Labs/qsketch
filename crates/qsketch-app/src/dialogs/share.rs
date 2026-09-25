//! File ▸ Share via Leyline: pick a conversation, then either share the
//! current canvas with it or open its shared canvas as a new tab.

use egui::{Color32, Context, RichText};

use super::modal;
use crate::share;
use crate::state::AppState;

#[derive(Default)]
pub struct ShareDialog {
    pub selected: Option<String>,
}

pub fn open(state: &mut AppState, ctx: &Context) {
    state.share_link(ctx);
    // Refresh the list every time the dialog opens: a new contact or group may
    // have appeared in Leyline since.
    if let Some(l) = state.share.as_ref() {
        l.send(share::link::Outbound::List {});
    }
    state.dialogs.share = Some(ShareDialog::default());
}

pub fn show(ctx: &Context, state: &mut AppState) {
    if state.dialogs.share.is_none() {
        return;
    }
    let p = state.settings.ui.palette();
    let (connected, me, convs, err) = match state.share.as_ref() {
        Some(l) => (l.connected, l.me.clone(), l.conversations.clone(), l.last_error.clone()),
        None => (false, None, Vec::new(), None),
    };
    let active = state.active_doc;
    let active_share = state.active().and_then(|d| d.share.as_ref()).map(|s| {
        (s.conv.name.clone(), share::status_color(state, s).unwrap_or(Color32::from_gray(140)), s.present().len())
    });
    let active_title = state.active().map(|d| d.doc.title.clone());
    let mut selected = state.dialogs.share.as_ref().and_then(|d| d.selected.clone());

    enum Act {
        None,
        Share(share::link::Conversation),
        Join(share::link::Conversation),
        Stop,
    }
    let mut act = Act::None;
    let ((), closed) = modal(ctx, "share", "Share via Leyline", 420.0, |ui| {
        match (&connected, &me) {
            (true, Some((_, name))) => {
                ui.label(RichText::new(format!("Connected to Leyline as {name}")).color(p.text_dim));
            }
            (true, None) => {
                ui.label(RichText::new("Connected to Leyline…").color(p.text_dim));
            }
            _ => {
                ui.label(RichText::new("Looking for Leyline…").color(p.text_dim));
                ui.label(
                    RichText::new("Leyline must be running on this computer (0.7.41 or newer).")
                        .small()
                        .color(p.text_dim),
                );
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            }
        }
        if let Some(e) = err {
            ui.colored_label(p.danger, e);
        }
        ui.add_space(8.0);

        if let Some((name, color, others)) = &active_share {
            ui.horizontal(|ui| {
                ui.label(share::dot(*color, 9.0));
                ui.label(format!("This canvas is shared with {name}"));
            });
            ui.label(
                RichText::new(match others {
                    0 => "Nobody else is drawing right now.".to_string(),
                    1 => "1 other person is drawing.".to_string(),
                    n => format!("{n} other people are drawing."),
                })
                .small()
                .color(p.text_dim),
            );
            ui.add_space(6.0);
            if ui.button("Stop sharing").clicked() {
                act = Act::Stop;
            }
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(4.0);
        }

        ui.label("Conversations");
        egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            if convs.is_empty() {
                ui.label(RichText::new(if connected { "(no contacts or groups yet)" } else { "…" }).color(p.text_dim));
            }
            for c in &convs {
                let dot = share::conv_color(&c.id);
                let is_sel = selected.as_deref() == Some(&c.id);
                let label = format!("{}{}", c.name, if c.group { "  (group)" } else { "" });
                ui.horizontal(|ui| {
                    ui.label(share::dot(dot, 9.0));
                    if ui.selectable_label(is_sel, label).clicked() {
                        selected = Some(c.id.clone());
                    }
                });
            }
        });
        ui.add_space(8.0);
        let chosen: Option<share::link::Conversation> =
            selected.as_ref().and_then(|id| convs.iter().find(|c| &c.id == id).cloned());
        ui.label(
            RichText::new(
                "Share sends the current canvas to the conversation and keeps it in sync while you draw. \
                 Join opens the conversation's canvas as a new tab.",
            )
            .small()
            .color(p.text_dim),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let can_share = connected && chosen.is_some() && active.is_some() && active_share.is_none();
            let share_label = match &active_title {
                Some(t) => format!("Share \"{t}\""),
                None => "Share current canvas".to_string(),
            };
            if ui.add_enabled(can_share, egui::Button::new(share_label)).clicked() {
                act = Act::Share(chosen.clone().unwrap());
            }
            if ui.add_enabled(connected && chosen.is_some(), egui::Button::new("Join shared canvas")).clicked() {
                act = Act::Join(chosen.clone().unwrap());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Close").clicked() {
                    ui.ctx().memory_mut(|m| m.data.insert_temp(egui::Id::new("share_close"), true));
                }
            });
        });
    });
    let close_btn = ctx.memory_mut(|m| m.data.remove_temp::<bool>(egui::Id::new("share_close"))).unwrap_or(false);
    if let Some(d) = state.dialogs.share.as_mut() {
        d.selected = selected;
    }
    match act {
        Act::None => {}
        Act::Share(c) => {
            if let Some(id) = active {
                share::start_sharing(state, ctx, id, c);
            }
            state.dialogs.share = None;
        }
        Act::Join(c) => {
            share::join(state, ctx, c);
            state.dialogs.share = None;
        }
        Act::Stop => {
            if let Some(id) = active {
                share::stop(state, id);
            }
        }
    }
    if closed || close_btn {
        state.dialogs.share = None;
    }
}
