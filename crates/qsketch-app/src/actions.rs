//! The command registry: every user-invokable command, its label, category and
//! default shortcut, plus the remappable keymap.

use std::collections::HashMap;

use egui::{Key, Modifiers};
use serde::{Deserialize, Serialize};

use crate::tools::ToolKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    File,
    Edit,
    Image,
    Layer,
    Select,
    View,
    Filter,
    Tools,
    Brush,
    Window,
    Help,
}

impl Category {
    pub const ALL: [Category; 11] = [
        Category::File,
        Category::Edit,
        Category::Image,
        Category::Layer,
        Category::Select,
        Category::View,
        Category::Filter,
        Category::Tools,
        Category::Brush,
        Category::Window,
        Category::Help,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Category::File => "File",
            Category::Edit => "Edit",
            Category::Image => "Image",
            Category::Layer => "Layer",
            Category::Select => "Select",
            Category::View => "View",
            Category::Filter => "Filter",
            Category::Tools => "Tools",
            Category::Brush => "Brush",
            Category::Window => "Window",
            Category::Help => "Help",
        }
    }
}

macro_rules! actions {
    ($( $variant:ident => ($cat:ident, $label:expr, [$($sc:expr),*]) ),* $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum Action { $($variant),* }

        impl Action {
            pub const ALL: &'static [Action] = &[$(Action::$variant),*];

            pub fn label(self) -> &'static str {
                match self { $(Action::$variant => $label),* }
            }

            pub fn category(self) -> Category {
                match self { $(Action::$variant => Category::$cat),* }
            }

            pub fn default_shortcuts(self) -> &'static [&'static str] {
                match self { $(Action::$variant => &[$($sc),*]),* }
            }

            pub fn id(self) -> &'static str {
                match self { $(Action::$variant => stringify!($variant)),* }
            }

            pub fn from_id(s: &str) -> Option<Action> {
                match s { $(stringify!($variant) => Some(Action::$variant),)* _ => None }
            }
        }
    };
}

actions! {
    // File
    NewDocument => (File, "New…", ["Ctrl+N"]),
    OpenDocument => (File, "Open…", ["Ctrl+O"]),
    Save => (File, "Save", ["Ctrl+S"]),
    SaveAs => (File, "Save As…", ["Ctrl+Shift+S"]),
    ExportImage => (File, "Export Image…", ["Ctrl+Alt+Shift+S"]),
    CloseDocument => (File, "Close", ["Ctrl+W"]),
    Quit => (File, "Quit", ["Ctrl+Q"]),
    // Edit
    Undo => (Edit, "Undo", ["Ctrl+Z"]),
    Redo => (Edit, "Redo", ["Ctrl+Shift+Z", "Ctrl+Y"]),
    Cut => (Edit, "Cut", ["Ctrl+X"]),
    Copy => (Edit, "Copy", ["Ctrl+C"]),
    CopyMerged => (Edit, "Copy Merged", ["Ctrl+Shift+C"]),
    Paste => (Edit, "Paste", ["Ctrl+V"]),
    PasteInPlace => (Edit, "Paste in Place", ["Ctrl+Shift+V"]),
    Clear => (Edit, "Clear", ["Delete"]),
    FillForeground => (Edit, "Fill with Foreground", ["Alt+Backspace"]),
    FillBackground => (Edit, "Fill with Background", ["Ctrl+Backspace"]),
    Preferences => (Edit, "Preferences…", ["Ctrl+K"]),
    // Image
    ImageSize => (Image, "Image Size…", ["Ctrl+Alt+I"]),
    CanvasSize => (Image, "Canvas Size…", ["Ctrl+Alt+C"]),
    CropToSelection => (Image, "Crop to Selection", []),
    FlipHorizontal => (Image, "Flip Canvas/Selection Horizontal", ["Shift+X"]),
    FlipVertical => (Image, "Flip Canvas/Selection Vertical", []),
    Rotate90CW => (Image, "Rotate 90° Clockwise", ["Ctrl+R"]),
    Rotate90CCW => (Image, "Rotate 90° Counter-Clockwise", []),
    Rotate180 => (Image, "Rotate 180°", []),
    InvertColors => (Image, "Invert", ["Ctrl+I"]),
    Desaturate => (Image, "Desaturate", ["Ctrl+Shift+U"]),
    BrightnessContrast => (Image, "Brightness/Contrast…", []),
    HueSaturation => (Image, "Hue/Saturation…", ["Ctrl+U"]),
    // Layer
    NewLayer => (Layer, "New Layer", ["Ctrl+Shift+N"]),
    DuplicateLayer => (Layer, "Duplicate Layer", ["Ctrl+J"]),
    DeleteLayer => (Layer, "Delete Layer", []),
    MergeDown => (Layer, "Merge Down", ["Ctrl+E"]),
    MergeVisible => (Layer, "Merge Visible", ["Ctrl+Shift+E"]),
    Flatten => (Layer, "Flatten Image", []),
    LayerUp => (Layer, "Move Layer Up", ["Ctrl+]"]),
    LayerDown => (Layer, "Move Layer Down", ["Ctrl+["]),
    LayerToTop => (Layer, "Move Layer to Top", ["Ctrl+Shift+]"]),
    LayerToBottom => (Layer, "Move Layer to Bottom", ["Ctrl+Shift+["]),
    SelectLayerAbove => (Layer, "Select Layer Above", ["Alt+]"]),
    SelectLayerBelow => (Layer, "Select Layer Below", ["Alt+["]),
    ToggleLayerVisibility => (Layer, "Toggle Visibility", []),
    ToggleAlphaLock => (Layer, "Lock Transparent Pixels", ["/"]),
    ToggleLayerLock => (Layer, "Lock Layer", []),
    LayerProperties => (Layer, "Layer Properties…", []),
    FlipLayerHorizontal => (Layer, "Flip Layer Horizontal", []),
    FlipLayerVertical => (Layer, "Flip Layer Vertical", []),
    ClearLayer => (Layer, "Clear Layer", []),
    // Select
    SelectAll => (Select, "All", ["Ctrl+A"]),
    Deselect => (Select, "Deselect", ["Ctrl+D"]),
    InvertSelection => (Select, "Inverse", ["Ctrl+Shift+I"]),
    SelectLayerContent => (Select, "Select Layer Content", []),
    // View
    ZoomIn => (View, "Zoom In", ["Ctrl+=", "Ctrl++"]),
    ZoomOut => (View, "Zoom Out", ["Ctrl+-"]),
    ZoomFit => (View, "Fit on Screen", ["Ctrl+0"]),
    Zoom100 => (View, "Actual Pixels", ["Ctrl+1"]),
    Zoom200 => (View, "Zoom 200%", ["Ctrl+2"]),
    RotateViewLeft => (View, "Rotate View Left", ["Shift+,"]),
    RotateViewRight => (View, "Rotate View Right", ["Shift+."]),
    ResetView => (View, "Reset View", ["Escape"]),
    FlipViewHorizontal => (View, "Flip View Horizontal", ["Shift+F"]),
    TogglePixelGrid => (View, "Pixel Grid", ["Ctrl+'"]),
    ToggleFullscreen => (View, "Fullscreen", ["F11"]),
    TogglePanels => (View, "Hide/Show Panels", ["Tab"]),
    // Filter (the menu groups these into submenus; see app.rs)
    LastFilter => (Filter, "Last Filter", ["Ctrl+F"]),
    LastFilterDialog => (Filter, "Last Filter Settings…", ["Ctrl+Alt+F"]),
    FilterOilPaint => (Filter, "Oil Paint…", []),
    FilterGaussianBlur => (Filter, "Gaussian Blur…", []),
    FilterBoxBlur => (Filter, "Box Blur…", []),
    FilterMotionBlur => (Filter, "Motion Blur…", []),
    FilterRadialBlur => (Filter, "Radial Blur…", []),
    FilterRipple => (Filter, "Ripple…", []),
    FilterWave => (Filter, "Wave…", []),
    FilterTwirl => (Filter, "Twirl…", []),
    FilterSpherize => (Filter, "Spherize…", []),
    FilterZigZag => (Filter, "ZigZag…", []),
    FilterPolarCoordinates => (Filter, "Polar Coordinates…", []),
    FilterAddNoise => (Filter, "Add Noise…", []),
    FilterMedian => (Filter, "Median…", []),
    FilterDustAndScratches => (Filter, "Dust & Scratches…", []),
    FilterMosaic => (Filter, "Mosaic…", []),
    FilterCrystallize => (Filter, "Crystallize…", []),
    FilterFragment => (Filter, "Fragment", []),
    FilterColorHalftone => (Filter, "Color Halftone…", []),
    FilterPointillize => (Filter, "Pointillize…", []),
    FilterClouds => (Filter, "Clouds…", []),
    FilterDifferenceClouds => (Filter, "Difference Clouds…", []),
    FilterSharpen => (Filter, "Sharpen", []),
    FilterSharpenMore => (Filter, "Sharpen More", []),
    FilterUnsharpMask => (Filter, "Unsharp Mask…", []),
    FilterFindEdges => (Filter, "Find Edges", []),
    FilterEmboss => (Filter, "Emboss…", []),
    FilterSolarize => (Filter, "Solarize", []),
    FilterDiffuse => (Filter, "Diffuse…", []),
    FilterWind => (Filter, "Wind…", []),
    FilterHighPass => (Filter, "High Pass…", []),
    FilterMaximum => (Filter, "Maximum…", []),
    FilterMinimum => (Filter, "Minimum…", []),
    FilterOffset => (Filter, "Offset…", []),
    FilterChromaticAberration => (Filter, "Chromatic Aberration…", []),
    FilterDither => (Filter, "Dither…", []),
    FilterPixelSort => (Filter, "Pixel Sort…", []),
    FilterScanlines => (Filter, "Scanlines…", []),
    FilterVignette => (Filter, "Vignette…", []),
    FilterGlow => (Filter, "Glow…", []),
    FilterKaleidoscope => (Filter, "Kaleidoscope…", []),
    FilterOutline => (Filter, "Outline…", []),
    FilterGlitch => (Filter, "Glitch…", []),
    FilterPencilSketch => (Filter, "Pencil Sketch…", []),
    // Tools
    ToolMove => (Tools, "Move Tool", ["V"]),
    ToolRectSelect => (Tools, "Rectangular Marquee", ["G", "M"]),
    ToolEllipseSelect => (Tools, "Elliptical Marquee", ["Shift+M"]),
    ToolLasso => (Tools, "Lasso", ["L"]),
    ToolMagicWand => (Tools, "Magic Wand", ["W"]),
    ToolCrop => (Tools, "Crop", ["C"]),
    ToolEyedropper => (Tools, "Eyedropper", ["I"]),
    ToolBrush => (Tools, "Brush", ["B"]),
    ToolPencil => (Tools, "Pencil", ["N"]),
    ToolEraser => (Tools, "Eraser", ["E"]),
    ToolFill => (Tools, "Paint Bucket", ["F"]),
    ToolGradient => (Tools, "Gradient", ["Shift+G"]),
    ToolLine => (Tools, "Line", ["U"]),
    ToolRect => (Tools, "Rectangle", ["Shift+U"]),
    ToolEllipse => (Tools, "Ellipse", ["Alt+U"]),
    ToolContour => (Tools, "Contour", ["P"]),
    ToolText => (Tools, "Text", ["T"]),
    ToolZoom => (Tools, "Zoom", ["Z"]),
    ToolHand => (Tools, "Hand", ["H"]),
    ToolRotateView => (Tools, "Rotate View", ["R"]),
    // Brush
    BrushSizeUp => (Brush, "Increase Brush Size", ["]"]),
    BrushSizeDown => (Brush, "Decrease Brush Size", ["["]),
    BrushHardnessUp => (Brush, "Increase Hardness", ["Shift+]"]),
    BrushHardnessDown => (Brush, "Decrease Hardness", ["Shift+["]),
    SwapColors => (Brush, "Swap Foreground/Background", ["X"]),
    ToggleSymmetryHorizontal => (Brush, "Mirror Horizontal", ["Shift+H"]),
    ToggleSymmetryVertical => (Brush, "Mirror Vertical", ["Shift+V"]),
    SymmetryOff => (Brush, "Symmetry Off", []),
    SymmetrySetCenter => (Brush, "Set Symmetry Center…", []),
    SymmetryResetCenter => (Brush, "Reset Symmetry Center", []),
    DefaultColors => (Brush, "Default Colors", ["D"]),
    // Window
    ShowTools => (Window, "Tools", []),
    ShowLayers => (Window, "Layers", ["F7"]),
    ShowHistory => (Window, "History", []),
    ShowColor => (Window, "Color", ["F6"]),
    ShowSwatches => (Window, "Swatches", []),
    ShowNavigator => (Window, "Navigator", []),
    ShowBrushes => (Window, "Brushes", ["F5"]),
    ShowBrushSettings => (Window, "Brush Settings", ["F9"]),
    ShowInfo => (Window, "Info", ["F8"]),
    ResetLayout => (Window, "Reset Workspace", []),
    // Help
    KeyboardShortcuts => (Help, "Keyboard Shortcuts…", ["Ctrl+Alt+Shift+K"]),
    CheckForUpdates => (Help, "Check for Updates…", []),
    About => (Help, "About qsketch", []),
}

impl Action {
    /// The tool an action selects, if any.
    pub fn tool(self) -> Option<ToolKind> {
        Some(match self {
            Action::ToolMove => ToolKind::Move,
            Action::ToolRectSelect => ToolKind::RectSelect,
            Action::ToolEllipseSelect => ToolKind::EllipseSelect,
            Action::ToolLasso => ToolKind::Lasso,
            Action::ToolMagicWand => ToolKind::MagicWand,
            Action::ToolCrop => ToolKind::Crop,
            Action::ToolEyedropper => ToolKind::Eyedropper,
            Action::ToolBrush => ToolKind::Brush,
            Action::ToolPencil => ToolKind::Pencil,
            Action::ToolEraser => ToolKind::Eraser,
            Action::ToolFill => ToolKind::Fill,
            Action::ToolGradient => ToolKind::Gradient,
            Action::ToolLine => ToolKind::Line,
            Action::ToolRect => ToolKind::Rect,
            Action::ToolEllipse => ToolKind::Ellipse,
            Action::ToolContour => ToolKind::Contour,
            Action::ToolText => ToolKind::Text,
            Action::ToolZoom => ToolKind::Zoom,
            Action::ToolHand => ToolKind::Hand,
            Action::ToolRotateView => ToolKind::RotateView,
            _ => return None,
        })
    }

    pub fn for_tool(tool: ToolKind) -> Action {
        match tool {
            ToolKind::Move => Action::ToolMove,
            ToolKind::RectSelect => Action::ToolRectSelect,
            ToolKind::EllipseSelect => Action::ToolEllipseSelect,
            ToolKind::Lasso => Action::ToolLasso,
            ToolKind::MagicWand => Action::ToolMagicWand,
            ToolKind::Crop => Action::ToolCrop,
            ToolKind::Eyedropper => Action::ToolEyedropper,
            ToolKind::Brush => Action::ToolBrush,
            ToolKind::Pencil => Action::ToolPencil,
            ToolKind::Eraser => Action::ToolEraser,
            ToolKind::Fill => Action::ToolFill,
            ToolKind::Gradient => Action::ToolGradient,
            ToolKind::Line => Action::ToolLine,
            ToolKind::Rect => Action::ToolRect,
            ToolKind::Ellipse => Action::ToolEllipse,
            ToolKind::Contour => Action::ToolContour,
            ToolKind::Text => Action::ToolText,
            ToolKind::Zoom => Action::ToolZoom,
            ToolKind::Hand => Action::ToolHand,
            ToolKind::RotateView => Action::ToolRotateView,
        }
    }

    /// Actions that may auto-repeat while the key is held.
    pub fn repeatable(self) -> bool {
        matches!(
            self,
            Action::Undo
                | Action::Redo
                | Action::ZoomIn
                | Action::ZoomOut
                | Action::BrushSizeUp
                | Action::BrushSizeDown
                | Action::BrushHardnessUp
                | Action::BrushHardnessDown
                | Action::RotateViewLeft
                | Action::RotateViewRight
                | Action::LayerUp
                | Action::LayerDown
                | Action::SelectLayerAbove
                | Action::SelectLayerBelow
        )
    }
}

/// A key chord.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub mods: Modifiers,
    pub key: Key,
}

impl Shortcut {
    pub fn new(mods: Modifiers, key: Key) -> Self {
        Self { mods, key }
    }

    /// Parse `"Ctrl+Shift+Z"`, `"]"`, `"Alt+Backspace"`, `"F5"` etc.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut mods = Modifiers::NONE;
        // Split on '+' but allow the key itself to be '+'.
        let (mod_part, key_part) = match s.rfind('+') {
            Some(i) if i + 1 < s.len() => (&s[..i], &s[i + 1..]),
            Some(i) if i > 0 => (&s[..i - 1], "+"),
            Some(_) => ("", "+"),
            None => ("", s),
        };
        for m in mod_part.split('+').map(str::trim).filter(|m| !m.is_empty()) {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => mods.command = true,
                "shift" => mods.shift = true,
                "alt" | "option" => mods.alt = true,
                _ => return None,
            }
        }
        let key = key_from_name(key_part.trim())?;
        if is_modifier_key(key) {
            return None;
        }
        Some(Self { mods, key })
    }

    pub fn display(&self) -> String {
        let mut parts = Vec::new();
        if self.mods.command || self.mods.ctrl || self.mods.mac_cmd {
            parts.push(if cfg!(target_os = "macos") { "Cmd" } else { "Ctrl" });
        }
        if self.mods.alt {
            parts.push("Alt");
        }
        if self.mods.shift {
            parts.push("Shift");
        }
        parts.push(key_display_name(self.key));
        parts.join("+")
    }

    /// Canonical serialization (`Ctrl+Shift+Z`).
    pub fn serialize(&self) -> String {
        let mut parts = Vec::new();
        if self.mods.command || self.mods.ctrl || self.mods.mac_cmd {
            parts.push("Ctrl".to_string());
        }
        if self.mods.alt {
            parts.push("Alt".to_string());
        }
        if self.mods.shift {
            parts.push("Shift".to_string());
        }
        parts.push(key_display_name(self.key).to_string());
        parts.join("+")
    }

    pub fn matches(&self, key: Key, mods: Modifiers) -> bool {
        let cmd = |m: Modifiers| m.command || m.ctrl || m.mac_cmd;
        key == self.key && mods.alt == self.mods.alt && mods.shift == self.mods.shift && cmd(mods) == cmd(self.mods)
    }
}

/// Bare modifier keys (egui 0.36 emits them as physical `Key`s); never a
/// valid chord key on their own.
pub fn is_modifier_key(key: Key) -> bool {
    matches!(
        key,
        Key::ShiftLeft
            | Key::ShiftRight
            | Key::ControlLeft
            | Key::ControlRight
            | Key::AltLeft
            | Key::AltRight
            | Key::SuperLeft
            | Key::SuperRight
    )
}

fn key_from_name(s: &str) -> Option<Key> {
    let lower = s.to_ascii_lowercase();
    let name = match lower.as_str() {
        "esc" | "escape" => "Escape",
        "del" | "delete" => "Delete",
        "backspace" => "Backspace",
        "enter" | "return" => "Enter",
        "space" => "Space",
        "tab" => "Tab",
        "insert" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "up" | "arrowup" => "ArrowUp",
        "down" | "arrowdown" => "ArrowDown",
        "left" | "arrowleft" => "ArrowLeft",
        "right" | "arrowright" => "ArrowRight",
        "plus" => "+",
        "minus" => "-",
        "equals" | "equal" => "=",
        "comma" => ",",
        "period" => ".",
        "slash" => "/",
        "backslash" => "\\",
        "semicolon" => ";",
        "quote" => "'",
        "backtick" | "grave" => "`",
        _ => s,
    };
    Key::from_name(name).or_else(|| Key::from_name(&name.to_ascii_uppercase()))
}

fn key_display_name(key: Key) -> &'static str {
    match key {
        Key::ArrowUp => "Up",
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::Escape => "Esc",
        Key::Delete => "Delete",
        Key::Backspace => "Backspace",
        Key::Enter => "Enter",
        Key::Space => "Space",
        Key::Tab => "Tab",
        Key::Plus => "+",
        Key::Minus => "-",
        Key::Equals => "=",
        Key::Comma => ",",
        Key::Period => ".",
        Key::Slash => "/",
        Key::Backslash => "\\",
        Key::Semicolon => ";",
        Key::Quote => "'",
        Key::Backtick => "`",
        Key::OpenBracket => "[",
        Key::CloseBracket => "]",
        _ => key.symbol_or_name(),
    }
}

/// Remappable key bindings for every action.
#[derive(Clone, Debug)]
pub struct Keymap {
    map: HashMap<Action, Vec<Shortcut>>,
}

impl Default for Keymap {
    fn default() -> Self {
        let mut map = HashMap::new();
        for &a in Action::ALL {
            map.insert(a, a.default_shortcuts().iter().filter_map(|s| Shortcut::parse(s)).collect());
        }
        Self { map }
    }
}

impl Keymap {
    /// Build from the default map plus user overrides (`action id → shortcuts`).
    pub fn with_overrides(overrides: &HashMap<String, Vec<String>>) -> Self {
        let mut km = Self::default();
        for (id, scs) in overrides {
            if let Some(a) = Action::from_id(id) {
                km.map.insert(a, scs.iter().filter_map(|s| Shortcut::parse(s)).collect());
            }
        }
        km
    }

    /// Only the bindings that differ from the defaults.
    pub fn overrides(&self) -> HashMap<String, Vec<String>> {
        let defaults = Keymap::default();
        let mut out = HashMap::new();
        for &a in Action::ALL {
            if self.shortcuts(a) != defaults.shortcuts(a) {
                out.insert(a.id().to_string(), self.shortcuts(a).iter().map(|s| s.serialize()).collect());
            }
        }
        out
    }

    pub fn shortcuts(&self, a: Action) -> &[Shortcut] {
        self.map.get(&a).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn set(&mut self, a: Action, shortcuts: Vec<Shortcut>) {
        self.map.insert(a, shortcuts);
    }

    pub fn reset(&mut self, a: Action) {
        self.map.insert(a, a.default_shortcuts().iter().filter_map(|s| Shortcut::parse(s)).collect());
    }

    pub fn reset_all(&mut self) {
        *self = Self::default();
    }

    /// Text for menus: the first binding, if any.
    pub fn primary_text(&self, a: Action) -> String {
        self.shortcuts(a).first().map(|s| s.display()).unwrap_or_default()
    }

    /// Another action already using this chord.
    pub fn conflict(&self, sc: Shortcut, except: Action) -> Option<Action> {
        Action::ALL.iter().copied().find(|&a| a != except && self.shortcuts(a).contains(&sc))
    }

    /// Resolve a key press to an action.
    pub fn lookup(&self, key: Key, mods: Modifiers) -> Option<Action> {
        // Prefer the most specific (most modifiers) match.
        let mut best: Option<(u32, Action)> = None;
        for &a in Action::ALL {
            for sc in self.shortcuts(a) {
                if sc.matches(key, mods) {
                    let n = sc.mods.command as u32 + sc.mods.alt as u32 + sc.mods.shift as u32;
                    if best.is_none_or(|(bn, _)| n > bn) {
                        best = Some((n, a));
                    }
                }
            }
        }
        best.map(|(_, a)| a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display() {
        let s = Shortcut::parse("Ctrl+Shift+Z").unwrap();
        assert!(s.mods.command && s.mods.shift && !s.mods.alt);
        assert_eq!(s.key, Key::Z);
        assert_eq!(s.display(), "Ctrl+Shift+Z");
        assert_eq!(Shortcut::parse("]").unwrap().key, Key::CloseBracket);
        assert_eq!(Shortcut::parse("Ctrl++").unwrap().key, Key::Plus);
        assert_eq!(Shortcut::parse("Ctrl+=").unwrap().key, Key::Equals);
        assert_eq!(Shortcut::parse("F11").unwrap().key, Key::F11);
        assert_eq!(Shortcut::parse("Alt+Backspace").unwrap().key, Key::Backspace);
        assert!(Shortcut::parse("ControlLeft").is_none());
        assert!(Shortcut::parse("Ctrl+ShiftLeft").is_none());
        assert!(Shortcut::parse("Hyper+Q").is_none());
    }

    #[test]
    fn all_defaults_parse() {
        for &a in Action::ALL {
            for s in a.default_shortcuts() {
                assert!(Shortcut::parse(s).is_some(), "{s} for {a:?}");
            }
        }
    }

    #[test]
    fn lookup_prefers_specific() {
        let km = Keymap::default();
        assert_eq!(km.lookup(Key::Z, Modifiers::COMMAND), Some(Action::Undo));
        assert_eq!(km.lookup(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT), Some(Action::Redo));
        assert_eq!(km.lookup(Key::B, Modifiers::NONE), Some(Action::ToolBrush));
        assert_eq!(km.lookup(Key::B, Modifiers::ALT), None);
        assert_eq!(km.lookup(Key::G, Modifiers::NONE), Some(Action::ToolRectSelect));
        assert_eq!(km.lookup(Key::R, Modifiers::COMMAND), Some(Action::Rotate90CW));
        assert_eq!(km.lookup(Key::X, Modifiers::SHIFT), Some(Action::FlipHorizontal));
    }

    #[test]
    fn no_default_conflicts() {
        let km = Keymap::default();
        for &a in Action::ALL {
            for sc in km.shortcuts(a) {
                assert_eq!(km.conflict(*sc, a), None, "{} for {a:?}", sc.display());
            }
        }
    }

    #[test]
    fn overrides_roundtrip() {
        let mut km = Keymap::default();
        km.set(Action::Undo, vec![Shortcut::parse("Ctrl+U").unwrap()]);
        let o = km.overrides();
        assert_eq!(o.len(), 1);
        let back = Keymap::with_overrides(&o);
        assert_eq!(back.shortcuts(Action::Undo), km.shortcuts(Action::Undo));
        assert_eq!(back.conflict(Shortcut::parse("Ctrl+U").unwrap(), Action::Redo), Some(Action::Undo));
    }
}
