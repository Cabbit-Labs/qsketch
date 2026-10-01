//! Menu rows that line up: every row reserves the same gutter on the left
//! for a check mark, so toggling a check never shifts its label, and
//! submenus open flush against the menu they belong to.

use egui::{Atom, AtomExt, Response, Ui, Vec2};

/// Width of the check-mark column at the left of every row.
pub const GUTTER: f32 = 18.0;
const GUTTER_H: f32 = 14.0;

/// The leading column of a row: a check mark, or the same width of nothing.
pub fn gutter(checked: bool) -> Atom<'static> {
    if checked {
        egui::RichText::new(super::icons::CHECK)
            .family(super::iconset::family())
            .size(12.0)
            .atom_size(Vec2::new(GUTTER, GUTTER_H))
    } else {
        Atom::custom(egui::Id::new("menu_gutter"), Vec2::new(GUTTER, GUTTER_H))
    }
}

/// A row button with the gutter; chain `.shortcut_text(...)` as needed.
pub fn button(checked: bool, label: impl Into<String>) -> egui::Button<'static> {
    egui::Button::new((gutter(checked), label.into()))
}

/// Add a row: gutter, label and a right-aligned shortcut.
pub fn item(
    ui: &mut Ui,
    checked: bool,
    label: impl Into<String>,
    shortcut: impl Into<egui::WidgetText>,
    enabled: bool,
) -> Response {
    ui.add_enabled(enabled, button(checked, label).shortcut_text(shortcut))
}

/// Space for a row made of plain widgets (a label with a value, say) so it
/// lines up with the buttons around it.
pub fn indent(ui: &mut Ui) {
    ui.add_space(GUTTER + ui.spacing().button_padding.x);
}

/// A submenu row with the gutter. The popup opens flush against the menu's
/// edge rather than a few points away from it. Returns the row's response
/// and the submenu's, when it is open.
pub fn submenu<R>(
    ui: &mut Ui,
    checked: bool,
    label: impl Into<String>,
    contents: impl FnOnce(&mut Ui) -> R,
) -> (Response, Option<egui::InnerResponse<R>>) {
    use egui::containers::menu::{MenuState, SubMenu, SubMenuButton};
    let my_id = ui.next_auto_id();
    let open = MenuState::from_ui(ui, |state, _| state.open_item == Some(SubMenu::id_from_widget_id(my_id)));
    let inactive = ui.style().visuals.widgets.inactive;
    if open {
        ui.style_mut().visuals.widgets.inactive = ui.style().visuals.widgets.open;
    }
    let response = ui.add(button(checked, label).right_text(SubMenuButton::RIGHT_ARROW));
    ui.style_mut().visuals.widgets.inactive = inactive;
    // egui places a submenu half the menu margin plus two points to the
    // right of the row. Widen the anchor so the popup's outer edge meets
    // the menu's outer edge instead (the frames overlap by a point so the
    // two outlines read as one).
    let frame = egui::Frame::menu(ui.style());
    let egui_gap = frame.total_margin().sum().x / 2.0 + 2.0;
    let edge = frame.inner_margin.right as f32 + frame.stroke.width;
    let mut anchor = response.clone();
    anchor.rect.max.x += edge - egui_gap - 1.0;
    let popup = SubMenu::default().show(ui, &anchor, contents);
    (response, popup)
}
