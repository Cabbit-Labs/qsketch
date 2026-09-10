//! The Home tab: quick actions and recent files.

use egui::{RichText, Ui};

use crate::actions::Action;
use crate::state::AppState;
use crate::ui::icons;

pub fn ui(ui: &mut Ui, state: &mut AppState) {
    egui::Frame::new().inner_margin(egui::Margin::same(32)).show(ui, |ui| {
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("qsketch").size(30.0).strong());
                ui.label(RichText::new(format!("v{}", qsketch_core::VERSION)).weak());
            });
            ui.label(RichText::new("Fast, focused sketching.").weak().size(14.0));
            ui.add_space(18.0);
            ui.horizontal_wrapped(|ui| {
                let big = |ui: &mut Ui, glyph: &str, title: &str, hint: String| -> bool {
                    let text = format!("{glyph}  {title}");
                    let r = ui.add_sized([180.0, 40.0], egui::Button::new(RichText::new(text).size(14.0)));
                    let r = if hint.is_empty() { r } else { r.on_hover_text(hint) };
                    r.clicked()
                };
                if big(ui, icons::FILE_PLUS, "New Document", state.keymap.primary_text(Action::NewDocument)) {
                    state.pending.push(Action::NewDocument);
                }
                if big(ui, icons::FOLDER_OPEN, "Open…", state.keymap.primary_text(Action::OpenDocument)) {
                    state.pending.push(Action::OpenDocument);
                }
                if big(ui, icons::GEAR, "Preferences", state.keymap.primary_text(Action::Preferences)) {
                    state.pending.push(Action::Preferences);
                }
            });
            ui.add_space(24.0);
            ui.label(RichText::new("RECENT FILES").small().weak().strong());
            ui.separator();
            let recent = state.settings.general.recent_files.clone();
            if recent.is_empty() {
                ui.label(RichText::new("Files you open or save will appear here.").weak());
            }
            let mut open: Option<std::path::PathBuf> = None;
            let mut forget: Option<std::path::PathBuf> = None;
            for p in &recent {
                ui.horizontal(|ui| {
                    let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                    let exists = p.exists();
                    let label = if exists {
                        RichText::new(format!("{}  {name}", icons::IMAGE))
                    } else {
                        RichText::new(format!("{}  {name}", icons::IMAGE)).strikethrough().weak()
                    };
                    let r = ui.add(egui::Button::new(label).frame_when_inactive(false));
                    if r.clicked() && exists {
                        open = Some(p.clone());
                    }
                    r.on_hover_text(p.display().to_string());
                    ui.label(
                        RichText::new(p.parent().map(|d| d.display().to_string()).unwrap_or_default()).weak().small(),
                    );
                    if ui.small_button(icons::X).on_hover_text("Remove from list").clicked() {
                        forget = Some(p.clone());
                    }
                });
            }
            if let Some(p) = open {
                state.open_file_requests.push(p);
            }
            if let Some(p) = forget {
                state.settings.general.recent_files.retain(|x| x != &p);
            }
            ui.add_space(24.0);
            ui.label(RichText::new("TIPS").small().weak().strong());
            ui.separator();
            let km = &state.keymap;
            let tip = |ui: &mut Ui, a: Action, text: &str| {
                ui.horizontal(|ui| {
                    crate::ui::widgets::keycap(ui, &km.primary_text(a));
                    ui.label(RichText::new(text).weak());
                });
            };
            tip(ui, Action::ToolBrush, "Brush · [ and ] change size · Shift+[ ] change hardness");
            tip(ui, Action::TogglePanels, "Hide every panel for a clean canvas");
            tip(ui, Action::ZoomFit, "Fit the document · hold Space to pan · R rotates the view");
            tip(ui, Action::KeyboardShortcuts, "Every shortcut is remappable in Preferences");
            ui.label(RichText::new("Drop image files onto the window to open them.").weak());
        });
    });
}
