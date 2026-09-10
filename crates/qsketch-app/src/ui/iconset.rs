//! Icon set resolution: which font (outline / filled) and which glyph each
//! tool uses, including per-tool user overrides. Kept in a process-wide
//! `RwLock` so `ToolKind::icon()` stays a plain method.

use std::collections::BTreeMap;
use std::sync::RwLock;

use egui::FontFamily;

use crate::settings::IconSet;
use crate::tools::ToolKind;
use crate::ui::icons;
use crate::ui::theme::{ICON_FONT, ICON_FONT_FILL};

struct Config {
    filled: bool,
    glyphs: BTreeMap<ToolKind, &'static str>,
}

static CONFIG: RwLock<Config> = RwLock::new(Config { filled: false, glyphs: BTreeMap::new() });

/// Font family for icon glyphs under the active set.
pub fn family() -> FontFamily {
    let filled = CONFIG.read().map(|c| c.filled).unwrap_or(false);
    FontFamily::Name(if filled { ICON_FONT_FILL } else { ICON_FONT }.into())
}

/// Glyph for `tool` under the active set and overrides.
pub fn tool_glyph(tool: ToolKind) -> &'static str {
    CONFIG.read().ok().and_then(|c| c.glyphs.get(&tool).copied()).unwrap_or_else(|| tool.default_icon())
}

/// Glyph a set would use for `tool` before user overrides.
pub fn set_glyph(set: IconSet, tool: ToolKind) -> &'static str {
    if !set.classic() {
        return tool.default_icon();
    }
    match tool {
        ToolKind::Move => icons::CURSOR,
        ToolKind::RectSelect => icons::SELECTION_ALL,
        ToolKind::Brush => icons::PAINT_BRUSH_BROAD,
        ToolKind::Pencil => icons::PENCIL_LINE,
        ToolKind::Eraser => icons::ERASER,
        ToolKind::Fill => icons::PAINT_BUCKET,
        ToolKind::Line => icons::LINE_SEGMENT,
        ToolKind::Rect => icons::SQUARE,
        ToolKind::Contour => icons::BEZIER_CURVE,
        ToolKind::Text => icons::TEXT_AA,
        ToolKind::Zoom => icons::MAGNIFYING_GLASS_PLUS,
        ToolKind::Hand => icons::HAND_GRABBING,
        ToolKind::Eyedropper => icons::EYEDROPPER,
        other => other.default_icon(),
    }
}

/// Look up a Phosphor glyph by its constant name (e.g. `PAINT_BRUSH`).
pub fn glyph_by_name(name: &str) -> Option<&'static str> {
    icons::ICONS.iter().find(|(n, _)| *n == name).map(|(_, g)| *g)
}

/// Name of a glyph, for the overrides editor.
pub fn name_of(glyph: &str) -> Option<&'static str> {
    icons::ICONS.iter().find(|(_, g)| *g == glyph).map(|(n, _)| *n)
}

/// Stable key for a tool in the overrides map.
pub fn tool_key(tool: ToolKind) -> String {
    format!("{tool:?}")
}

/// Recompute the active glyph table from settings.
pub fn configure(set: IconSet, overrides: &BTreeMap<String, String>) {
    let mut glyphs = BTreeMap::new();
    for t in ToolKind::ALL {
        let g = overrides.get(&tool_key(t)).and_then(|n| glyph_by_name(n)).unwrap_or_else(|| set_glyph(set, t));
        glyphs.insert(t, g);
    }
    if let Ok(mut c) = CONFIG.write() {
        c.filled = set.filled();
        c.glyphs = glyphs;
    }
}
