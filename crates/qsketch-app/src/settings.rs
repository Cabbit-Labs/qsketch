//! Persistent user settings (TOML in the platform config directory).

use std::collections::HashMap;
use std::path::PathBuf;

use qsketch_core::{BrushSettings, Rgba8};
use serde::{Deserialize, Serialize};

/// Directory component of the per-user config path. Kept with the original
/// capitalization so settings written by older versions are still found
/// (`%APPDATA%\cabbit-labs\qSketch`, `dev.cabbit-labs.qSketch`; Linux
/// lowercases it anyway).
pub const CONFIG_DIR_NAME: &str = "qSketch";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Theme {
    /// Indigo + pink, derived from the app icon.
    #[default]
    Ink,
    /// Neutral dark gray with a teal accent (the pre-0.6 look).
    /// "Midnight" (removed in 0.9.3) folds into this one.
    #[serde(alias = "Dark", alias = "Midnight")]
    Graphite,
    Light,
    /// Warm paper tones.
    Sepia,
    /// User-configured colors (`UiSettings::custom_palette`).
    Custom,
}

impl Theme {
    pub const ALL: [Theme; 5] = [Theme::Ink, Theme::Graphite, Theme::Light, Theme::Sepia, Theme::Custom];

    pub fn label(self) -> &'static str {
        match self {
            Theme::Ink => "Ink",
            Theme::Graphite => "Graphite",
            Theme::Light => "Light",
            Theme::Sepia => "Sepia",
            Theme::Custom => "Custom",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Theme::Ink => "Indigo and pink, matching the app icon",
            Theme::Graphite => "Neutral dark gray with a teal accent",
            Theme::Light => "Cool light gray",
            Theme::Sepia => "Warm paper tones",
            Theme::Custom => "Your own colors",
        }
    }
}

/// Colors of the Custom theme: a primary (chrome) color and a secondary
/// (accent) color, stored as sRGB triples so the TOML stays hand-editable.
/// Everything else (backgrounds, widget shades, borders, text) is derived
/// from the primary's lightness.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomPalette {
    /// Panel color; the whole chrome is shaded from it.
    #[serde(alias = "panel")]
    pub primary: [u8; 3],
    /// Accent: selection, links, the active tool.
    #[serde(alias = "accent")]
    pub secondary: [u8; 3],
}

impl Default for CustomPalette {
    fn default() -> Self {
        Self::from_palette(&crate::ui::theme::Palette::ink())
    }
}

impl CustomPalette {
    pub fn from_palette(p: &crate::ui::theme::Palette) -> Self {
        let c = |c: egui::Color32| [c.r(), c.g(), c.b()];
        Self { primary: c(p.panel), secondary: c(p.accent) }
    }

    /// Whether the chrome wants light text: whichever of white / black
    /// contrasts more with the primary color.
    pub fn is_dark(&self) -> bool {
        contrast([255; 3], self.primary) >= contrast([0; 3], self.primary)
    }

    pub fn to_palette(&self) -> crate::ui::theme::Palette {
        let dark = self.is_dark();
        let c = |v: [u8; 3]| egui::Color32::from_rgb(v[0], v[1], v[2]);
        // Positive = toward white, negative = toward black; dark themes get
        // lighter widgets on the panel, light themes darker ones.
        let sign = if dark { 1.0 } else { -1.0 };
        let shade = |t: f32| c(mix(self.primary, t));
        let text = if dark { [230, 233, 245] } else { [30, 30, 36] };
        let text_mix = |t: f32| c(mix_to(self.primary, text, t));
        // Dim text is the panel color mixed toward the text color, pushed far
        // enough to stay comfortably readable against the panel — well past
        // the 4.5:1 minimum, because this color also carries "greyed out"
        // text (hidden layers, disabled rows) that still has to be read. A
        // fixed gray fails on mid-tone chrome such as a pink theme.
        let text_dim = {
            let mut t = 0.55;
            while t < 1.0 && contrast(mix_to(self.primary, text, t), self.primary) < DIM_TEXT_CONTRAST {
                t += 0.05;
            }
            c(mix_to(self.primary, text, t))
        };
        crate::ui::theme::Palette {
            bg: shade(-0.30 * sign),
            panel: c(self.primary),
            panel_alt: shade(-0.14 * sign),
            widget: shade(0.12 * sign),
            widget_hover: shade(0.22 * sign),
            widget_active: shade(0.34 * sign),
            border: shade(if dark { -0.5 } else { -0.25 }),
            text: text_mix(1.0),
            text_dim,
            accent: c(self.secondary),
            accent_dim: c(mix(self.secondary, -0.3)),
            danger: egui::Color32::from_rgb(255, 92, 92),
            dark,
        }
    }
}

/// Contrast the dim/secondary text aims for against the panel it sits on.
/// The WCAG floor for body text is 4.5:1; dim text goes further because it
/// doubles as the "greyed out" color.
pub const DIM_TEXT_CONTRAST: f32 = 7.0;

/// WCAG contrast ratio (1..21) between two sRGB triples.
fn contrast(a: [u8; 3], b: [u8; 3]) -> f32 {
    fn lin(c: u8) -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    let rl = |v: [u8; 3]| 0.2126 * lin(v[0]) + 0.7152 * lin(v[1]) + 0.0722 * lin(v[2]);
    let (x, y) = (rl(a), rl(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

#[cfg(test)]
mod palette_tests {
    use super::*;

    /// Old files get the new Ctrl = brush size default once, Alt stays the
    /// color picker's (a 0.37.1 file that had Alt on brush size goes back to
    /// zoom), and a user's own choice for either chord is left alone.
    #[test]
    fn ctrl_wheel_migrates_to_brush_size_and_alt_stays_the_pickers() {
        let mut c = CanvasSettings {
            wheel_ctrl: WheelAction::ScrollVertical,
            wheel_alt: WheelAction::Zoom,
            ctrl_wheel_migrated: false,
            alt_wheel_restored: false,
            ..Default::default()
        };
        c.migrate_ctrl_wheel();
        assert_eq!(c.wheel_ctrl, WheelAction::BrushSize);
        assert_eq!(c.wheel_alt, WheelAction::Zoom);
        // A file from 0.37.1, where Alt had been given brush size, goes back.
        let mut from_0371 = CanvasSettings {
            wheel_alt: WheelAction::BrushSize,
            ctrl_wheel_migrated: true,
            alt_wheel_restored: false,
            ..Default::default()
        };
        from_0371.migrate_ctrl_wheel();
        assert_eq!(from_0371.wheel_alt, WheelAction::Zoom);
        let mut own = CanvasSettings {
            wheel_ctrl: WheelAction::ScrollHorizontal,
            wheel_alt: WheelAction::Nothing,
            ctrl_wheel_migrated: false,
            alt_wheel_restored: false,
            ..Default::default()
        };
        own.migrate_ctrl_wheel();
        assert_eq!(own.wheel_ctrl, WheelAction::ScrollHorizontal);
        assert_eq!(own.wheel_alt, WheelAction::Nothing);
        own.wheel_ctrl = WheelAction::ScrollVertical;
        own.migrate_ctrl_wheel();
        assert_eq!(own.wheel_ctrl, WheelAction::ScrollVertical, "runs once");
    }

    /// Old settings keep the behavior they had, and the most specific
    /// modifier held wins.
    #[test]
    fn wheel_migrates_and_picks_the_specific_chord() {
        let m =
            |ctrl: bool, alt: bool, shift: bool| egui::Modifiers { command: ctrl, ctrl, alt, shift, mac_cmd: false };
        // The old default: wheel zooms, Shift/Ctrl pan.
        let mut c = CanvasSettings { wheel: WheelBehavior::Zoom, wheel_migrated: false, ..Default::default() };
        c.migrate_wheel();
        let w = c.wheel_map();
        assert_eq!(w.action(m(false, false, false)), WheelAction::Zoom);
        assert_eq!(w.action(m(false, false, true)), WheelAction::ScrollHorizontal);
        assert_eq!(w.action(m(true, false, false)), WheelAction::ScrollVertical);
        // Ctrl outranks the others when several are held.
        assert_eq!(w.action(m(true, true, true)), WheelAction::ScrollVertical);

        // The other old mode: wheel scrolls, Ctrl/Alt zoom.
        let mut c = CanvasSettings { wheel: WheelBehavior::Scroll, wheel_migrated: false, ..Default::default() };
        c.migrate_wheel();
        let w = c.wheel_map();
        assert_eq!(w.action(m(false, false, false)), WheelAction::ScrollVertical);
        assert_eq!(w.action(m(true, false, false)), WheelAction::Zoom);
        assert_eq!(w.action(m(false, true, false)), WheelAction::Zoom);

        // Migration runs once: a later edit is not overwritten.
        c.wheel_ctrl = WheelAction::Nothing;
        c.migrate_wheel();
        assert_eq!(c.wheel_ctrl, WheelAction::Nothing);
    }

    #[test]
    fn custom_dim_text_stays_readable() {
        for primary in [[232u8, 150, 190], [40, 44, 70], [245, 245, 240], [120, 120, 120], [200, 60, 60]] {
            let p = CustomPalette { primary, secondary: [90, 140, 220] }.to_palette();
            let dim = [p.text_dim.r(), p.text_dim.g(), p.text_dim.b()];
            let best = contrast([p.text.r(), p.text.g(), p.text.b()], primary);
            // Mid-gray chrome can't reach the target with any text; then dim
            // text must at least match the primary text.
            let want = DIM_TEXT_CONTRAST.min(best - 0.01);
            assert!(contrast(dim, primary) >= want, "{primary:?} -> {dim:?} (want {want}, best {best})");
        }
    }

    /// Every built-in theme's dim text must be readable on its own panel too,
    /// not just the generated custom ones.
    #[test]
    fn builtin_dim_text_is_readable() {
        use crate::ui::theme::Palette;
        for (name, p) in [
            ("ink", Palette::ink()),
            ("graphite", Palette::graphite()),
            ("light", Palette::light()),
            ("sepia", Palette::sepia()),
        ] {
            let rgb = |c: egui::Color32| [c.r(), c.g(), c.b()];
            let dim = contrast(rgb(p.text_dim), rgb(p.panel));
            assert!(dim >= 4.5, "{name}: dim text only {dim:.1}:1 on the panel");
        }
    }
}

/// Mix toward white (`t` > 0) or black (`t` < 0) by |t|.
fn mix(v: [u8; 3], t: f32) -> [u8; 3] {
    let target = if t >= 0.0 { [255u8; 3] } else { [0u8; 3] };
    mix_to(v, target, t.abs())
}

fn mix_to(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    let f = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    [f(0), f(1), f(2)]
}

/// Overall shape language of the chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum UiShape {
    /// Soft corners, roomy spacing (the original look).
    #[default]
    Rounded,
    /// Square corners with small chamfers, flat 1 px outlines, tighter
    /// spacing: a technical, angular look.
    Angular,
}

impl UiShape {
    pub const ALL: [UiShape; 2] = [UiShape::Rounded, UiShape::Angular];
    pub fn label(self) -> &'static str {
        match self {
            UiShape::Rounded => "Rounded",
            UiShape::Angular => "Angular",
        }
    }
}

/// Which glyphs draw the tools and panel chrome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum IconSet {
    /// Phosphor regular outlines (the default).
    #[default]
    Outline,
    /// Phosphor filled shapes.
    Filled,
    /// Alternate, more literal glyphs (cursor, broad brush, grabbing hand…).
    Classic,
    /// Alternate glyphs, filled.
    ClassicFilled,
}

impl IconSet {
    pub const ALL: [IconSet; 4] = [IconSet::Outline, IconSet::Filled, IconSet::Classic, IconSet::ClassicFilled];

    pub fn label(self) -> &'static str {
        match self {
            IconSet::Outline => "Outline",
            IconSet::Filled => "Filled",
            IconSet::Classic => "Classic",
            IconSet::ClassicFilled => "Classic Filled",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            IconSet::Outline => "Thin outlined glyphs",
            IconSet::Filled => "Solid filled glyphs",
            IconSet::Classic => "Literal tool shapes, outlined",
            IconSet::ClassicFilled => "Literal tool shapes, filled",
        }
    }

    pub fn filled(self) -> bool {
        matches!(self, IconSet::Filled | IconSet::ClassicFilled)
    }

    pub fn classic(self) -> bool {
        matches!(self, IconSet::Classic | IconSet::ClassicFilled)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BrushCursor {
    #[default]
    Outline,
    Crosshair,
    Both,
    Hidden,
}

/// When the selected area is tinted on the canvas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SelectionTintMode {
    /// Only while a selection tool (or Move) is active.
    #[default]
    SelectionTools,
    Always,
}

/// The four wheel bindings, copied out of the settings so the canvas can
/// read them without holding a borrow on the whole settings struct.
#[derive(Clone, Copy, Debug)]
pub struct WheelMap {
    pub plain: WheelAction,
    pub shift: WheelAction,
    pub ctrl: WheelAction,
    pub alt: WheelAction,
}

impl WheelMap {
    /// Checked ctrl, then alt, then shift, then plain, so the most specific
    /// chord held wins.
    pub fn action(&self, m: egui::Modifiers) -> WheelAction {
        if m.command || m.ctrl || m.mac_cmd {
            self.ctrl
        } else if m.alt {
            self.alt
        } else if m.shift {
            self.shift
        } else {
            self.plain
        }
    }
}

impl CanvasSettings {
    pub fn wheel_map(&self) -> WheelMap {
        WheelMap { plain: self.wheel_plain, shift: self.wheel_shift, ctrl: self.wheel_ctrl, alt: self.wheel_alt }
    }

    /// One-time migration of the old two-mode wheel setting into the
    /// per-modifier map, so existing installs keep the behavior they had.
    pub fn migrate_wheel(&mut self) {
        if self.wheel_migrated {
            return;
        }
        self.wheel_migrated = true;
        let (plain, shift, ctrl, alt) = match self.wheel {
            WheelBehavior::Zoom => {
                (WheelAction::Zoom, WheelAction::ScrollHorizontal, WheelAction::ScrollVertical, WheelAction::Zoom)
            }
            WheelBehavior::Scroll => {
                (WheelAction::ScrollVertical, WheelAction::ScrollHorizontal, WheelAction::Zoom, WheelAction::Zoom)
            }
        };
        self.wheel_plain = plain;
        self.wheel_shift = shift;
        self.wheel_ctrl = ctrl;
        self.wheel_alt = alt;
    }

    /// Ctrl+wheel became "brush size" (0.37.1). A file that still carries the
    /// old default for it takes the new one, once; anything the user chose
    /// themselves stays. (0.37.1 briefly gave Alt+wheel the same job; Alt is
    /// the color picker's, so 0.37.2 hands an Alt set that way back to zoom.)
    pub fn migrate_ctrl_wheel(&mut self) {
        if self.alt_wheel_restored {
            return;
        }
        self.alt_wheel_restored = true;
        if !self.ctrl_wheel_migrated {
            self.ctrl_wheel_migrated = true;
            if self.wheel_ctrl == WheelAction::ScrollVertical {
                self.wheel_ctrl = WheelAction::BrushSize;
            }
        }
        if self.wheel_alt == WheelAction::BrushSize {
            self.wheel_alt = WheelAction::Zoom;
        }
    }
}

/// What one turn of the mouse wheel does on the canvas, per modifier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WheelAction {
    #[default]
    Zoom,
    ScrollVertical,
    ScrollHorizontal,
    /// Grow / shrink the current tool's brush, a notch per step.
    BrushSize,
    Nothing,
}

impl WheelAction {
    pub const ALL: [WheelAction; 5] = [
        WheelAction::Zoom,
        WheelAction::ScrollVertical,
        WheelAction::ScrollHorizontal,
        WheelAction::BrushSize,
        WheelAction::Nothing,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WheelAction::Zoom => "Zoom",
            WheelAction::ScrollVertical => "Scroll up / down",
            WheelAction::ScrollHorizontal => "Scroll left / right",
            WheelAction::BrushSize => "Brush size",
            WheelAction::Nothing => "Nothing",
        }
    }
}

/// Legacy two-mode setting, kept only to migrate old settings files into
/// the per-modifier map below.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WheelBehavior {
    /// Aseprite / Krita style: wheel zooms at cursor, Shift+wheel pans.
    #[default]
    Zoom,
    /// Photoshop style: wheel scrolls, Ctrl/Alt+wheel zooms.
    Scroll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NewDocBackground {
    #[default]
    White,
    Transparent,
    BackgroundColor,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub undo_limit: usize,
    pub confirm_close: bool,
    pub max_recent: usize,
    pub recent_files: Vec<PathBuf>,
    pub new_doc_width: u32,
    pub new_doc_height: u32,
    pub new_doc_background: NewDocBackground,
    pub open_last_files_on_start: bool,
    /// Periodically snapshot unsaved documents for crash recovery.
    pub autosave: bool,
    pub autosave_interval_secs: u32,
    /// Copy the previous version aside whenever a file is overwritten.
    pub backups: bool,
    /// How many previous versions to keep per file.
    pub backup_versions: u32,
    /// Paste onto a new layer above the active one instead of onto it.
    pub paste_new_layer: bool,
    /// Delete drops the selection after clearing it, instead of leaving the
    /// marching ants around the hole it made.
    pub deselect_after_delete: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            undo_limit: 400,
            confirm_close: true,
            max_recent: 12,
            recent_files: Vec::new(),
            new_doc_width: 1920,
            new_doc_height: 1080,
            new_doc_background: NewDocBackground::White,
            open_last_files_on_start: false,
            autosave: true,
            autosave_interval_secs: 120,
            backups: true,
            backup_versions: 3,
            paste_new_layer: true,
            deselect_after_delete: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub theme: Theme,
    pub scale: f32,
    /// Snap floating panels to the window edges.
    pub snap_windows: bool,
    pub snap_distance: f32,
    pub show_tooltips: bool,
    pub compact_tool_options: bool,
    /// Use the OS title bar instead of qsketch's own compact strip (menus,
    /// update indicator and window buttons in one bar).
    pub native_frame: bool,
    /// Rounded or angular chrome (see `UiShape`).
    pub shape: UiShape,
    /// Icon and wordmark dissolve into the workspace when the app opens.
    pub startup_animation: bool,
    pub custom_palette: CustomPalette,
    pub icon_set: IconSet,
    /// Per-tool glyph overrides: tool name (`ToolKind` Debug form) → Phosphor icon name.
    pub icon_overrides: std::collections::BTreeMap<String, String>,
    /// When locked, tools cannot be dragged around the Tools strip.
    pub tools_locked: bool,
    /// User order of the Tools strip; empty = default order.
    pub tool_order: Vec<crate::tools::ToolKind>,
    /// Which slider group the Color panel shows (HSV or RGB, never both).
    pub color_sliders: ColorSliders,
    /// Grain/texture over panels and bars.
    pub texture: TextureSettings,
}

/// Which chrome texture tile to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ChromeTexture {
    None,
    #[default]
    BrushedMetal,
    Carbon,
    Paper,
    Grain,
    /// Fine grid whose lines fade in and out across the tile.
    Grid,
    Stars,
    Dots,
    Hearts,
    Argyle,
    /// A user image (tiled; converted to a neutral light/dark grain).
    Custom,
}

impl ChromeTexture {
    pub const ALL: [ChromeTexture; 11] = [
        ChromeTexture::None,
        ChromeTexture::BrushedMetal,
        ChromeTexture::Carbon,
        ChromeTexture::Paper,
        ChromeTexture::Grain,
        ChromeTexture::Grid,
        ChromeTexture::Stars,
        ChromeTexture::Dots,
        ChromeTexture::Hearts,
        ChromeTexture::Argyle,
        ChromeTexture::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChromeTexture::None => "None",
            ChromeTexture::BrushedMetal => "Brushed metal",
            ChromeTexture::Carbon => "Carbon weave",
            ChromeTexture::Paper => "Paper",
            ChromeTexture::Grain => "Fine grain",
            ChromeTexture::Grid => "Fading grid",
            ChromeTexture::Stars => "Stars",
            ChromeTexture::Dots => "Polka dots",
            ChromeTexture::Hearts => "Hearts",
            ChromeTexture::Argyle => "Argyle",
            ChromeTexture::Custom => "Custom image…",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextureSettings {
    pub kind: ChromeTexture,
    /// Overall opacity of the grain, 0..=1.
    pub strength: f32,
    /// Faint highlight along the top edge of each panel.
    pub sheen: bool,
    /// Size multiplier for the tile (1 = native pixels).
    pub scale: f32,
    /// Bilinear filtering; off = nearest-neighbor (crisp, aliased pixels).
    pub smooth: bool,
    /// Image file for `ChromeTexture::Custom`.
    pub custom_path: String,
}

impl Default for TextureSettings {
    fn default() -> Self {
        Self {
            kind: ChromeTexture::BrushedMetal,
            strength: 0.11,
            sheen: true,
            scale: 1.0,
            smooth: true,
            custom_path: String::new(),
        }
    }
}

impl TextureSettings {
    pub fn enabled(&self) -> bool {
        self.kind != ChromeTexture::None && self.strength > 0.0
    }
}

impl UiSettings {
    /// The active color palette (resolves the Custom theme).
    pub fn palette(&self) -> crate::ui::theme::Palette {
        match self.theme {
            Theme::Custom => self.custom_palette.to_palette(),
            t => crate::ui::theme::Palette::for_theme(t),
        }
    }
}

/// Slider group shown in the Color panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorSliders {
    #[default]
    Hsv,
    Rgb,
}

impl ColorSliders {
    pub const ALL: [ColorSliders; 2] = [ColorSliders::Hsv, ColorSliders::Rgb];

    pub fn label(self) -> &'static str {
        match self {
            ColorSliders::Hsv => "HSV",
            ColorSliders::Rgb => "RGB",
        }
    }
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            theme: Theme::Ink,
            scale: 1.0,
            snap_windows: true,
            snap_distance: 12.0,
            show_tooltips: true,
            compact_tool_options: false,
            native_frame: false,
            shape: UiShape::default(),
            startup_animation: true,
            custom_palette: CustomPalette::default(),
            icon_set: IconSet::Outline,
            icon_overrides: Default::default(),
            tools_locked: true,
            tool_order: Vec::new(),
            color_sliders: ColorSliders::default(),
            texture: TextureSettings::default(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CanvasSettings {
    pub checker_a: [u8; 3],
    pub checker_b: [u8; 3],
    pub checker_size: f32,
    /// Color around the document; `None` follows the UI theme.
    pub outside_color: Option<[u8; 3]>,
    pub pixel_grid_min_zoom: f32,
    pub show_pixel_grid: bool,
    /// Tile grid over the canvas (View ▸ Grid), every `grid_size` pixels.
    pub show_grid: bool,
    pub grid_size: u32,
    /// Shape, marquee, crop, move and gradient drags land on grid lines.
    pub snap_to_grid: bool,
    /// Tiled preview: repeat the document around itself (0 off, 1 across,
    /// 2 down, 3 both) so seamless tiles can be judged while painting.
    pub tiled: u8,
    pub wheel: WheelBehavior,
    /// What the wheel does per modifier. Checked ctrl, then alt, then shift,
    /// then plain, so the most specific chord held wins.
    pub wheel_plain: WheelAction,
    pub wheel_shift: WheelAction,
    pub wheel_ctrl: WheelAction,
    pub wheel_alt: WheelAction,
    /// Whether the one-time migration from `wheel` has run.
    pub wheel_migrated: bool,
    #[serde(default)]
    pub ctrl_wheel_migrated: bool,
    #[serde(default)]
    pub alt_wheel_restored: bool,
    pub invert_wheel_zoom: bool,
    pub zoom_to_cursor: bool,
    pub brush_cursor: BrushCursor,
    pub smooth_zoom_out: bool,
    /// While `Q` is held the view follows the pointer (no click needed).
    pub quick_rotate_follow_pointer: bool,
    /// Tapping `Q` twice resets the view rotation.
    pub quick_rotate_double_tap_reset: bool,
    /// A fast hand-tool (Space) drag keeps gliding briefly after release.
    pub pan_inertia: bool,
    /// Dragging a selection, shape or move past the edge of the viewport
    /// scrolls the canvas in that direction.
    pub edge_autoscroll: bool,
    /// Auto-scroll speed at the viewport edge, screen points per second.
    pub edge_autoscroll_speed: f32,
    /// Tint the selected area on the canvas (besides the marching ants).
    pub selection_tint: bool,
    pub selection_tint_mode: SelectionTintMode,
    pub selection_tint_color: [u8; 3],
    /// 0..=1 overlay opacity at full selection coverage.
    pub selection_tint_opacity: f32,
    /// Briefly light up a layer's pixels on the canvas when it becomes active.
    pub flash_selected_layer: bool,
    /// Smooth (bilinear) or Pixel (nearest) resampling for the transform box;
    /// the toolbar toggle writes it back so the choice sticks.
    pub transform_filter: qsketch_core::raster::ResizeFilter,
    /// Show the zoomed loupe while picking a color.
    pub pick_loupe: bool,
    /// Loupe diameter in screen points.
    pub pick_loupe_size: f32,
    /// How many canvas pixels the loupe spans.
    pub pick_loupe_pixels: u32,
}

impl Default for CanvasSettings {
    fn default() -> Self {
        Self {
            checker_a: [204, 204, 204],
            checker_b: [255, 255, 255],
            checker_size: 8.0,
            outside_color: None,
            pixel_grid_min_zoom: 8.0,
            show_pixel_grid: false,
            show_grid: false,
            grid_size: 16,
            snap_to_grid: false,
            tiled: 0,
            transform_filter: qsketch_core::raster::ResizeFilter::Bilinear,
            wheel: WheelBehavior::Zoom,
            wheel_plain: WheelAction::Zoom,
            wheel_shift: WheelAction::ScrollHorizontal,
            wheel_ctrl: WheelAction::BrushSize,
            wheel_alt: WheelAction::Zoom,
            wheel_migrated: false,
            ctrl_wheel_migrated: false,
            alt_wheel_restored: false,
            invert_wheel_zoom: false,
            zoom_to_cursor: true,
            brush_cursor: BrushCursor::Outline,
            smooth_zoom_out: true,
            quick_rotate_follow_pointer: true,
            quick_rotate_double_tap_reset: true,
            pan_inertia: true,
            edge_autoscroll: true,
            edge_autoscroll_speed: 240.0,
            selection_tint: false,
            selection_tint_mode: SelectionTintMode::SelectionTools,
            selection_tint_color: [40, 90, 220],
            selection_tint_opacity: 0.35,
            flash_selected_layer: true,
            pick_loupe: true,
            pick_loupe_size: 86.0,
            pick_loupe_pixels: 13,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub fn from_egui(b: egui::PointerButton) -> Option<Self> {
        Some(match b {
            egui::PointerButton::Primary => Self::Left,
            egui::PointerButton::Secondary => Self::Right,
            egui::PointerButton::Middle => Self::Middle,
            _ => return None,
        })
    }
}

/// A mouse button plus the modifiers held with it: "Alt+Left click".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseChord {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub button: MouseButton,
}

impl Default for MouseChord {
    fn default() -> Self {
        Self { ctrl: false, shift: false, alt: true, button: MouseButton::Left }
    }
}

impl MouseChord {
    pub fn from_egui(mods: egui::Modifiers, button: egui::PointerButton) -> Option<Self> {
        Some(Self {
            ctrl: mods.command || mods.ctrl || mods.mac_cmd,
            shift: mods.shift,
            alt: mods.alt,
            button: MouseButton::from_egui(button)?,
        })
    }

    pub fn has_modifiers(self) -> bool {
        self.ctrl || self.shift || self.alt
    }

    /// The chord's modifiers (and only those) are currently held.
    pub fn modifiers_held(self, mods: egui::Modifiers) -> bool {
        self.ctrl == (mods.command || mods.ctrl || mods.mac_cmd) && self.shift == mods.shift && self.alt == mods.alt
    }

    pub fn matches(self, mods: egui::Modifiers, button: egui::PointerButton) -> bool {
        self.modifiers_held(mods) && MouseButton::from_egui(button) == Some(self.button)
    }

    pub fn label(self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str(if cfg!(target_os = "macos") { "Cmd+" } else { "Ctrl+" });
        }
        if self.shift {
            s.push_str("Shift+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        s.push_str(match self.button {
            MouseButton::Left => "Left click",
            MouseButton::Right => "Right click",
            MouseButton::Middle => "Middle click",
        });
        s
    }
}

/// Mouse-button behaviors on the canvas (keyboard chords live in the keymap).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MouseSettings {
    /// Middle-button drag pans the view with any tool.
    pub middle_drag_pans: bool,
    /// Right-click with a brush-based tool opens the quick brush settings popup.
    pub right_click_brush_popup: bool,
    /// Chord that picks the foreground color with any color-using tool.
    pub pick_foreground: Option<MouseChord>,
    /// Chord that picks the background color with any color-using tool.
    pub pick_background: Option<MouseChord>,
    /// Chord that drags the layer (or the selected pixels) as the Move tool
    /// with any tool active.
    pub quick_move: Option<MouseChord>,
    /// Alt with a color tool picks a color, on top of whatever the chords
    /// below say: Alt+left takes the foreground, Alt+right the background.
    /// This is the Photoshop reflex, and holding Alt shows the eyedropper.
    pub alt_click_picks: bool,
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            middle_drag_pans: true,
            // Right-click picks a color (Aseprite-style); the quick brush
            // popup moves out of its way and is off unless asked for.
            right_click_brush_popup: false,
            pick_foreground: Some(MouseChord { ctrl: false, shift: false, alt: false, button: MouseButton::Right }),
            pick_background: Some(MouseChord { button: MouseButton::Right, ..MouseChord::default() }),
            quick_move: Some(MouseChord { ctrl: true, shift: false, alt: false, button: MouseButton::Left }),
            alt_click_picks: true,
        }
    }
}

impl MouseSettings {
    /// A pick chord's modifiers are held (a chord without modifiers can't be
    /// signalled ahead of the click, so it never puts the eyedropper up).
    pub fn pick_modifiers_held(&self, mods: egui::Modifiers) -> bool {
        if self.alt_only(mods) {
            return true;
        }
        [self.pick_foreground, self.pick_background]
            .into_iter()
            .flatten()
            .any(|c| c.has_modifiers() && c.modifiers_held(mods))
    }

    /// Alt alone is held and it means "pick a color" (see `alt_click_picks`).
    /// A quick-move chord bound to Alt keeps Alt for itself.
    fn alt_only(&self, mods: egui::Modifiers) -> bool {
        self.alt_click_picks
            && mods.alt
            && !mods.shift
            && !(mods.command || mods.ctrl || mods.mac_cmd)
            && !self.quick_move.is_some_and(|c| c.alt && !c.ctrl && !c.shift)
    }

    /// The quick-move chord's modifiers are held (a chord without modifiers
    /// can't be signalled ahead of the press).
    pub fn quick_move_modifiers_held(&self, mods: egui::Modifiers) -> bool {
        self.quick_move.is_some_and(|c| c.has_modifiers() && c.modifiers_held(mods))
    }

    /// A press with these modifiers + button starts a quick move.
    pub fn is_quick_move(&self, mods: egui::Modifiers, button: egui::PointerButton) -> bool {
        self.quick_move.is_some_and(|c| c.matches(mods, button))
    }

    /// Which color a press with these modifiers + button picks, if any.
    pub fn pick_target(&self, mods: egui::Modifiers, button: egui::PointerButton) -> Option<PickTarget> {
        if self.pick_foreground.is_some_and(|c| c.matches(mods, button)) {
            Some(PickTarget::Foreground)
        } else if self.pick_background.is_some_and(|c| c.matches(mods, button)) {
            Some(PickTarget::Background)
        } else if self.alt_only(mods) {
            match button {
                egui::PointerButton::Primary => Some(PickTarget::Foreground),
                egui::PointerButton::Secondary => Some(PickTarget::Background),
                _ => None,
            }
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickTarget {
    Foreground,
    Background,
}

/// In-app self-update.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Check the manifest when the app starts (at most every `check_interval_hours`).
    pub check_on_start: bool,
    pub check_interval_hours: u32,
    /// Manifest URL; empty = update checks are disabled.
    pub manifest_url: String,
    /// Whether the one-time legacy manifest URL migration has run.
    pub manifest_url_migrated: bool,
    /// A version the user chose not to be reminded about.
    pub skipped_version: String,
    /// Unix seconds of the last automatic check.
    pub last_check: u64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_on_start: true,
            check_interval_hours: 6,
            manifest_url: String::new(),
            manifest_url_migrated: false,
            skipped_version: String::new(),
            last_check: 0,
        }
    }
}

impl UpdateSettings {
    /// The manifest URL to check, or `None` when updates are disabled.
    pub fn effective_manifest_url(&self) -> Option<String> {
        let s = self.manifest_url.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    }

    /// Installs from before the URL became user-configurable have an empty
    /// `manifest_url`; copy the build's legacy URL in once so they keep
    /// updating. Only called for settings files that already existed.
    fn migrate_manifest_url(&mut self) {
        if self.manifest_url_migrated {
            return;
        }
        self.manifest_url_migrated = true;
        if self.manifest_url.trim().is_empty() {
            if let Some(url) = crate::update::LEGACY_MANIFEST_URL {
                self.manifest_url = url.to_string();
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TabletSettings {
    /// Pressure curve exponent; < 1 makes light touches stronger.
    pub pressure_gamma: f32,
    /// Ignore pressure below this (dead zone), 0..=0.5.
    pub min_pressure: f32,
    /// Use the octotablet backend (Windows Ink RealTimeStylus / Wayland tablet)
    /// for tilt and eraser detection. Off = native winit pen events.
    pub use_octotablet: bool,
    /// Windows: use the WinTab driver API (works with "Use Windows Ink" off).
    /// Takes precedence over `use_octotablet` when the driver provides it.
    pub use_wintab: bool,
    /// Switch to the eraser when the stylus eraser tip is used.
    pub eraser_tip_switches_tool: bool,
    /// Treat mouse input as full pressure.
    pub mouse_pressure: f32,
}

impl Default for TabletSettings {
    fn default() -> Self {
        Self {
            pressure_gamma: 1.0,
            min_pressure: 0.0,
            use_octotablet: false,
            use_wintab: true,
            eraser_tip_switches_tool: true,
            mouse_pressure: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaintSettings {
    pub brush: BrushSettings,
    pub pencil: BrushSettings,
    pub eraser: BrushSettings,
    pub select_brush: BrushSettings,
    pub smudge: BrushSettings,
    pub clone: BrushSettings,
    pub presets: Vec<BrushSettings>,
    pub foreground: Rgba8,
    pub background: Rgba8,
    pub swatches: Vec<Rgba8>,
    pub recent_colors: Vec<Rgba8>,
    pub symmetry: crate::tools::symmetry::Symmetry,
}

impl Default for PaintSettings {
    fn default() -> Self {
        Self {
            brush: BrushSettings::preset("Hard Round"),
            pencil: BrushSettings::preset("Pixel"),
            eraser: BrushSettings { name: "Eraser".into(), ..BrushSettings::preset("Hard Round") },
            select_brush: BrushSettings {
                name: "Selection".into(),
                size: 24.0,
                hardness: 1.0,
                opacity: 1.0,
                flow: 1.0,
                ..BrushSettings::preset("Hard Round")
            },
            smudge: BrushSettings {
                name: "Smudge".into(),
                size: 30.0,
                hardness: 0.5,
                opacity: 0.7,
                flow: 1.0,
                spacing: 0.08,
                ..BrushSettings::preset("Soft Round")
            },
            clone: BrushSettings { name: "Clone".into(), size: 40.0, ..BrushSettings::preset("Soft Round") },
            presets: BrushSettings::presets(),
            foreground: Rgba8::BLACK,
            background: Rgba8::WHITE,
            swatches: default_swatches(),
            recent_colors: Vec::new(),
            symmetry: Default::default(),
        }
    }
}

pub fn default_swatches() -> Vec<Rgba8> {
    let hex = [
        "#000000", "#ffffff", "#7f7f7f", "#c3c3c3", "#880015", "#b97a57", "#ed1c24", "#ffaec9", "#ff7f27", "#ffc90e",
        "#fff200", "#efe4b0", "#22b14c", "#b5e61d", "#00a2e8", "#99d9ea", "#3f48cc", "#7092be", "#a349a4", "#c8bfe7",
        "#1e1f2a", "#2b2d42", "#17e3b4", "#8be34a", "#d9f24e", "#ff5c8a", "#5cc8ff", "#f5f5dc", "#8b4513", "#556b2f",
    ];
    hex.iter().filter_map(|h| Rgba8::from_hex(h)).collect()
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub general: GeneralSettings,
    pub ui: UiSettings,
    pub canvas: CanvasSettings,
    pub mouse: MouseSettings,
    pub tablet: TabletSettings,
    pub update: UpdateSettings,
    pub paint: PaintSettings,
    /// Keymap overrides: action id → shortcuts.
    pub shortcuts: HashMap<String, Vec<String>>,
    /// Serialized egui_dock layout.
    pub layout: Option<String>,
    /// Last windowed geometry as logical points: outer x, y and inner
    /// width, height. Given to the window before it is shown so it opens at
    /// its final size and place without a visible resize.
    pub window_rect: Option<[f32; 4]>,
    pub window_maximized: bool,
    /// Bumped when a default changes in a way that has to reach settings files
    /// written by an older version (see `migrate`). 0 = before any migration.
    pub schema: u32,
}

/// Current settings schema. 1 = right-click picks a color; 2 = smaller loupe.
pub const SCHEMA: u32 = 2;

impl Settings {
    pub fn config_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("dev", "cabbit-labs", CONFIG_DIR_NAME).map(|d| d.config_dir().to_path_buf())
    }

    pub fn path() -> Option<PathBuf> {
        Self::config_dir().map(|d| d.join("settings.toml"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Settings>(&s) {
                Ok(mut settings) => {
                    settings.sanitize();
                    settings.update.migrate_manifest_url();
                    settings.migrate();
                    settings
                }
                Err(e) => {
                    log::warn!("settings parse error ({}): {e}; using defaults", path.display());
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let Some(path) = Self::path() else { return Ok(()) };
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let s = toml::to_string_pretty(self)?;
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, s)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Carry default changes into settings files written by older versions.
    /// Only untouched bindings are moved: a chord the user chose stays put.
    fn migrate(&mut self) {
        if self.schema >= SCHEMA {
            return;
        }
        // 0 → 1: right-click picks the foreground color, which takes the button
        // from the quick brush popup.
        let old_pick_fg = MouseChord { ctrl: false, shift: false, alt: true, button: MouseButton::Left };
        if self.schema < 1 && self.mouse.pick_foreground == Some(old_pick_fg) {
            self.mouse.pick_foreground =
                Some(MouseChord { ctrl: false, shift: false, alt: false, button: MouseButton::Right });
            self.mouse.right_click_brush_popup = false;
        }
        // 1 → 2: the pick loupe shrank; carry it over unless resized by hand.
        if self.schema < 2 && (self.canvas.pick_loupe_size - 132.0).abs() < 0.5 {
            self.canvas.pick_loupe_size = 86.0;
        }
        self.schema = SCHEMA;
    }

    pub fn sanitize(&mut self) {
        self.canvas.migrate_wheel();
        self.canvas.migrate_ctrl_wheel();
        self.general.undo_limit = self.general.undo_limit.clamp(2, 2000);
        self.ui.scale = self.ui.scale.clamp(0.5, 3.0);
        self.tablet.pressure_gamma = self.tablet.pressure_gamma.clamp(0.2, 5.0);
        self.tablet.min_pressure = self.tablet.min_pressure.clamp(0.0, 0.5);
        self.canvas.checker_size = self.canvas.checker_size.clamp(2.0, 64.0);
        self.canvas.pixel_grid_min_zoom = self.canvas.pixel_grid_min_zoom.clamp(2.0, 64.0);
        self.canvas.grid_size = self.canvas.grid_size.clamp(1, 4096);
        self.update.check_interval_hours = self.update.check_interval_hours.clamp(1, 24 * 30);
        self.general.autosave_interval_secs = self.general.autosave_interval_secs.clamp(15, 3600);
        self.paint.recent_colors.truncate(crate::state::COLOR_HISTORY_LEN);
        self.paint.symmetry.radial = self.paint.symmetry.radial.min(64);
        self.paint.brush.clamp();
        self.paint.pencil.clamp();
        self.paint.eraser.clamp();
        self.paint.select_brush.clamp();
        for p in &mut self.paint.presets {
            p.clamp();
        }
        if self.paint.presets.is_empty() {
            self.paint.presets = BrushSettings::presets();
        }
        if self.paint.swatches.is_empty() {
            self.paint.swatches = default_swatches();
        }
    }

    pub fn push_recent(&mut self, path: PathBuf) {
        self.general.recent_files.retain(|p| p != &path);
        self.general.recent_files.insert(0, path);
        let max = self.general.max_recent.max(1);
        self.general.recent_files.truncate(max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_roundtrip() {
        let mut s = Settings::default();
        s.shortcuts.insert("Undo".into(), vec!["Ctrl+U".into()]);
        s.push_recent(PathBuf::from("/tmp/a.qsk"));
        let text = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back, s);
    }
}
