//! qsketch's look, without the app: the themes and their palettes, the egui and dock styles, the fonts (with the Phosphor icon
//! font) and the icon codepoints. qsketch-app re-exports these as `ui::theme` and `ui::phosphor`, and qSketch's sibling apps
//! (Resin, `/code/resin`) use them for the same look. No app state lives here: a theme is chosen by its caller's settings.

pub mod icons;
pub mod theme;
