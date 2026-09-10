//! Visual themes, plus font setup including the Phosphor icon font.
//!
//! The default theme (`Ink`) is derived from the app icon: an indigo tile
//! with a pink pencil. Every theme is monochromatic: one accent hue on a
//! neutral chrome. `Section` still names the functional families so the
//! chrome can group controls, but all sections share the accent.

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Style, Visuals};

use crate::settings::Theme;

/// Name of the icon font family registered in egui.
pub const ICON_FONT: &str = "phosphor";
pub const ICON_FONT_FILL: &str = "phosphor-fill";

/// Functional section a piece of UI belongs to. Every section has a stable
/// color in each theme so the chrome is color-coded rather than a wall of
/// identical gray boxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// Tool picking + per-tool options.
    Tools,
    /// Brushes, brush settings, symmetry, stabilizer.
    Brush,
    /// Color picker and swatches.
    Color,
    /// Layers, history, document structure.
    Layers,
    /// Navigator, info, view.
    View,
}

#[allow(dead_code)]
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub panel_alt: Color32,
    pub widget: Color32,
    pub widget_hover: Color32,
    pub widget_active: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub accent: Color32,
    pub accent_dim: Color32,
    pub danger: Color32,
    pub dark: bool,
}

impl Palette {
    /// App-icon theme: indigo chrome, pink accent, ribbon section colors.
    pub fn ink() -> Self {
        Self {
            bg: Color32::from_rgb(20, 24, 42),
            panel: Color32::from_rgb(30, 36, 64),
            panel_alt: Color32::from_rgb(25, 30, 54),
            widget: Color32::from_rgb(44, 52, 88),
            widget_hover: Color32::from_rgb(56, 66, 108),
            widget_active: Color32::from_rgb(70, 82, 130),
            border: Color32::from_rgb(14, 17, 32),
            text: Color32::from_rgb(230, 233, 245),
            text_dim: Color32::from_rgb(150, 158, 190),
            accent: Color32::from_rgb(255, 61, 138),
            accent_dim: Color32::from_rgb(176, 42, 96),
            danger: Color32::from_rgb(255, 92, 92),
            dark: true,
        }
    }

    /// The previous neutral dark theme with a teal accent.
    pub fn graphite() -> Self {
        Self {
            bg: Color32::from_rgb(30, 31, 36),
            panel: Color32::from_rgb(40, 41, 47),
            panel_alt: Color32::from_rgb(34, 35, 41),
            widget: Color32::from_rgb(52, 54, 62),
            widget_hover: Color32::from_rgb(64, 66, 76),
            widget_active: Color32::from_rgb(78, 80, 92),
            border: Color32::from_rgb(22, 22, 26),
            text: Color32::from_rgb(222, 224, 230),
            text_dim: Color32::from_rgb(150, 153, 163),
            accent: Color32::from_rgb(23, 227, 180),
            accent_dim: Color32::from_rgb(19, 150, 122),
            danger: Color32::from_rgb(235, 87, 87),
            dark: true,
        }
    }

    pub fn light() -> Self {
        Self {
            bg: Color32::from_rgb(236, 237, 240),
            panel: Color32::from_rgb(246, 246, 248),
            panel_alt: Color32::from_rgb(230, 231, 235),
            widget: Color32::from_rgb(220, 221, 226),
            widget_hover: Color32::from_rgb(205, 207, 214),
            widget_active: Color32::from_rgb(188, 190, 200),
            border: Color32::from_rgb(190, 192, 200),
            text: Color32::from_rgb(30, 31, 36),
            text_dim: Color32::from_rgb(105, 108, 118),
            accent: Color32::from_rgb(220, 40, 110),
            accent_dim: Color32::from_rgb(170, 30, 85),
            danger: Color32::from_rgb(200, 50, 50),
            dark: false,
        }
    }

    /// Warm paper tones for long sessions in a bright room.
    pub fn sepia() -> Self {
        Self {
            bg: Color32::from_rgb(226, 216, 198),
            panel: Color32::from_rgb(243, 236, 222),
            panel_alt: Color32::from_rgb(232, 223, 205),
            widget: Color32::from_rgb(220, 208, 186),
            widget_hover: Color32::from_rgb(206, 192, 166),
            widget_active: Color32::from_rgb(190, 174, 146),
            border: Color32::from_rgb(180, 166, 140),
            text: Color32::from_rgb(52, 40, 28),
            text_dim: Color32::from_rgb(120, 104, 84),
            accent: Color32::from_rgb(196, 62, 90),
            accent_dim: Color32::from_rgb(150, 44, 68),
            danger: Color32::from_rgb(190, 50, 40),
            dark: false,
        }
    }

    pub fn for_theme(t: Theme) -> Self {
        match t {
            Theme::Ink => Self::ink(),
            Theme::Graphite => Self::graphite(),
            Theme::Light => Self::light(),
            Theme::Sepia => Self::sepia(),
            // Callers with settings use `UiSettings::palette`; this is the fallback base.
            Theme::Custom => Self::ink(),
        }
    }

    /// Edge color for a group of controls. Monochrome: always the accent.
    pub fn section(&self, _s: Section) -> Color32 {
        self.accent
    }

    /// Neutral background for chips / grouped controls.
    pub fn section_tint(&self, _s: Section) -> Color32 {
        self.panel_alt
    }
}

/// Register the UI fonts. `filled` puts the filled icon font first in the
/// text fallback chain so inline glyphs (menus, labels) follow the icon set.
pub fn install_fonts(ctx: &egui::Context, filled: bool) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        ICON_FONT.into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!("../../../../assets/fonts/Phosphor.ttf"))),
    );
    fonts.font_data.insert(
        ICON_FONT_FILL.into(),
        std::sync::Arc::new(FontData::from_static(include_bytes!("../../../../assets/fonts/Phosphor-Fill.ttf"))),
    );
    // Icons as a fallback in the proportional family so labels can mix text and glyphs.
    let order: [&str; 2] = if filled { [ICON_FONT_FILL, ICON_FONT] } else { [ICON_FONT, ICON_FONT_FILL] };
    for f in order {
        fonts.families.entry(FontFamily::Proportional).or_default().push(f.into());
        fonts.families.entry(FontFamily::Monospace).or_default().push(f.into());
    }
    fonts.families.insert(FontFamily::Name(ICON_FONT.into()), vec![ICON_FONT.into()]);
    fonts.families.insert(FontFamily::Name(ICON_FONT_FILL.into()), vec![ICON_FONT_FILL.into()]);
    ctx.set_fonts(fonts);
}

pub fn apply(ctx: &egui::Context, p: &Palette, scale: f32) {
    let mut visuals = if p.dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(4);
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.panel;
    visuals.extreme_bg_color = p.panel_alt;
    visuals.faint_bg_color = p.panel_alt;
    visuals.window_stroke = Stroke::new(1.0, p.border);
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.menu_corner_radius = CornerRadius::same(6);
    visuals.selection.bg_fill = if p.dark { p.accent_dim.gamma_multiply(0.6) } else { p.accent.gamma_multiply(0.28) };
    visuals.selection.stroke = Stroke::new(1.0, p.accent);
    visuals.hyperlink_color = p.accent;
    visuals.override_text_color = None;
    visuals.widgets.noninteractive.bg_fill = p.panel;
    visuals.widgets.noninteractive.weak_bg_fill = p.panel;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, p.text_dim);
    visuals.widgets.noninteractive.corner_radius = r;
    visuals.widgets.inactive.bg_fill = p.widget;
    visuals.widgets.inactive.weak_bg_fill = p.widget;
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.inactive.corner_radius = r;
    visuals.widgets.hovered.bg_fill = p.widget_hover;
    visuals.widgets.hovered.weak_bg_fill = p.widget_hover;
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, p.border);
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.hovered.corner_radius = r;
    visuals.widgets.active.bg_fill = p.widget_active;
    visuals.widgets.active.weak_bg_fill = p.widget_active;
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, p.accent);
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, p.text);
    visuals.widgets.active.corner_radius = r;
    visuals.widgets.open.bg_fill = p.widget_active;
    visuals.widgets.open.weak_bg_fill = p.widget_active;
    visuals.widgets.open.corner_radius = r;
    visuals.slider_trailing_fill = true;
    visuals.window_shadow.spread = 4;
    visuals.popup_shadow.spread = 2;
    visuals.striped = true;
    visuals.indent_has_left_vline = false;

    let mut style = Style { visuals, ..Style::default() };
    style.spacing.item_spacing = egui::vec2(5.0, 3.0);
    style.spacing.button_padding = egui::vec2(5.0, 2.0);
    style.spacing.interact_size = egui::vec2(26.0, 18.0);
    style.spacing.slider_width = 110.0;
    style.spacing.menu_margin = egui::Margin::same(6);
    style.spacing.window_margin = egui::Margin::same(8);
    style.interaction.selectable_labels = false;
    style.interaction.tooltip_delay = 0.4;
    style.animation_time = 0.08;
    let egui_theme = if p.dark { egui::Theme::Dark } else { egui::Theme::Light };
    ctx.set_theme(egui_theme);
    ctx.set_style_of(egui_theme, style);
    ctx.set_zoom_factor(scale);
}

/// Dock-area style consistent with the palette.
pub fn dock_style(ctx: &egui::Context, p: &Palette) -> egui_dock::Style {
    let mut s = egui_dock::Style::from_egui(&ctx.global_style());
    s.tab_bar.height = 22.0;
    s.tab_bar.bg_fill = p.panel_alt;
    s.tab_bar.hline_color = p.border;
    s.tab.active.bg_fill = p.panel;
    s.tab.active.outline_color = p.border;
    s.tab.active.text_color = p.text;
    s.tab.inactive.bg_fill = p.panel_alt;
    s.tab.inactive.text_color = p.text_dim;
    s.tab.hovered.bg_fill = p.widget_hover;
    s.tab.hovered.text_color = p.text;
    s.tab.focused.bg_fill = p.panel;
    s.tab.focused.text_color = p.text;
    s.tab.tab_body.bg_fill = p.panel;
    s.tab.tab_body.stroke = Stroke::new(1.0, p.border);
    s.tab.tab_body.inner_margin = egui::Margin::same(4);
    s.separator.width = 3.0;
    s.separator.color_idle = p.bg;
    s.separator.color_hovered = p.accent_dim;
    s.separator.color_dragged = p.accent;
    s.overlay.selection_color = p.accent.gamma_multiply(0.35);
    s.overlay.button_color = p.widget;
    s.overlay.button_border_stroke = Stroke::new(1.0, p.accent);
    s.buttons.close_tab_color = p.text_dim;
    s.buttons.close_tab_active_color = p.text;
    s.buttons.add_tab_color = p.text_dim;
    s.dock_area_padding = None;
    s.main_surface_border_stroke = Stroke::NONE;
    s
}
