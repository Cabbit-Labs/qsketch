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

    pub fn is_dark(&self) -> bool {
        luma(self.primary) < 0.5
    }

    pub fn to_palette(&self) -> crate::ui::theme::Palette {
        let dark = self.is_dark();
        let c = |v: [u8; 3]| egui::Color32::from_rgb(v[0], v[1], v[2]);
        // Positive = toward white, negative = toward black; dark themes get
        // lighter widgets on the panel, light themes darker ones.
        let sign = if dark { 1.0 } else { -1.0 };
        let shade = |t: f32| c(mix(self.primary, t));
        let text = if dark { [230, 233, 245] } else { [30, 30, 36] };
        let text_dim = if dark { [150, 158, 190] } else { [100, 100, 115] };
        let text_mix = |t: f32| c(mix_to(self.primary, text, t));
        crate::ui::theme::Palette {
            bg: shade(-0.30 * sign),
            panel: c(self.primary),
            panel_alt: shade(-0.14 * sign),
            widget: shade(0.12 * sign),
            widget_hover: shade(0.22 * sign),
            widget_active: shade(0.34 * sign),
            border: shade(if dark { -0.5 } else { -0.25 }),
            text: text_mix(1.0),
            text_dim: c(text_dim),
            accent: c(self.secondary),
            accent_dim: c(mix(self.secondary, -0.3)),
            danger: egui::Color32::from_rgb(255, 92, 92),
            dark,
        }
    }
}

/// Relative luminance (0..1) of an sRGB triple.
fn luma(v: [u8; 3]) -> f32 {
    (0.2126 * v[0] as f32 + 0.7152 * v[1] as f32 + 0.0722 * v[2] as f32) / 255.0
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
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self {
            undo_limit: 100,
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
    pub wheel: WheelBehavior,
    pub invert_wheel_zoom: bool,
    pub zoom_to_cursor: bool,
    pub brush_cursor: BrushCursor,
    pub smooth_zoom_out: bool,
    /// While `Q` is held the view follows the pointer (no click needed).
    pub quick_rotate_follow_pointer: bool,
    /// Tapping `Q` twice resets the view rotation.
    pub quick_rotate_double_tap_reset: bool,
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
            wheel: WheelBehavior::Zoom,
            invert_wheel_zoom: false,
            zoom_to_cursor: true,
            brush_cursor: BrushCursor::Outline,
            smooth_zoom_out: true,
            quick_rotate_follow_pointer: true,
            quick_rotate_double_tap_reset: true,
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
}

impl Default for MouseSettings {
    fn default() -> Self {
        Self {
            middle_drag_pans: true,
            right_click_brush_popup: true,
            pick_foreground: Some(MouseChord { button: MouseButton::Left, ..MouseChord::default() }),
            pick_background: Some(MouseChord { button: MouseButton::Right, ..MouseChord::default() }),
            quick_move: Some(MouseChord { ctrl: true, shift: false, alt: false, button: MouseButton::Left }),
        }
    }
}

impl MouseSettings {
    /// A pick chord's modifiers are held (a chord without modifiers can't be
    /// signalled ahead of the click, so it never puts the eyedropper up).
    pub fn pick_modifiers_held(&self, mods: egui::Modifiers) -> bool {
        [self.pick_foreground, self.pick_background]
            .into_iter()
            .flatten()
            .any(|c| c.has_modifiers() && c.modifiers_held(mods))
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
            use_wintab: false,
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
    pub window_maximized: bool,
}

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

    pub fn sanitize(&mut self) {
        self.general.undo_limit = self.general.undo_limit.clamp(2, 2000);
        self.ui.scale = self.ui.scale.clamp(0.5, 3.0);
        self.tablet.pressure_gamma = self.tablet.pressure_gamma.clamp(0.2, 5.0);
        self.tablet.min_pressure = self.tablet.min_pressure.clamp(0.0, 0.5);
        self.canvas.checker_size = self.canvas.checker_size.clamp(2.0, 64.0);
        self.canvas.pixel_grid_min_zoom = self.canvas.pixel_grid_min_zoom.clamp(2.0, 64.0);
        self.update.check_interval_hours = self.update.check_interval_hours.clamp(1, 24 * 30);
        self.general.autosave_interval_secs = self.general.autosave_interval_secs.clamp(15, 3600);
        self.paint.recent_colors.truncate(crate::state::COLOR_HISTORY_LEN);
        self.paint.symmetry.radial = self.paint.symmetry.radial.min(64);
        self.paint.brush.clamp();
        self.paint.pencil.clamp();
        self.paint.eraser.clamp();
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
