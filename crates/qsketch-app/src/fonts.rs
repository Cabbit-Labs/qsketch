//! Fonts for the Text tool: egui's bundled faces plus the system's installed
//! fonts and any TTF/OTF files in `<config>/fonts/`. Discovery is lazy (the
//! system scan can take a moment) and parsed faces are cached per family/style.

use ab_glyph::{FontArc, FontVec};
use fontdb::{Database, Family, Query, Source, Stretch, Style, Weight, ID};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::settings::Settings;

/// Family shown first and used when a saved family is no longer installed.
pub const DEFAULT_FAMILY: &str = "Ubuntu";

pub struct FontLibrary {
    db: Database,
    /// Sorted family names.
    families: Vec<String>,
    cache: HashMap<(String, bool, bool), Option<FontArc>>,
    loaded: bool,
}

impl Default for FontLibrary {
    fn default() -> Self {
        Self { db: Database::new(), families: Vec::new(), cache: HashMap::new(), loaded: false }
    }
}

impl FontLibrary {
    pub fn dir() -> Option<PathBuf> {
        Settings::config_dir().map(|d| d.join("fonts"))
    }

    /// Load everything on first use.
    pub fn ensure_loaded(&mut self) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let t = std::time::Instant::now();
        self.db.load_font_data(epaint_default_fonts::UBUNTU_LIGHT.to_vec());
        self.db.load_font_data(epaint_default_fonts::HACK_REGULAR.to_vec());
        self.db.load_system_fonts();
        if let Some(dir) = Self::dir() {
            if dir.is_dir() {
                self.db.load_fonts_dir(&dir);
            }
        }
        self.rebuild_families();
        log::info!("fonts: {} families, {} faces in {:?}", self.families.len(), self.db.len(), t.elapsed());
    }

    /// Re-scan the user font directory (after dropping files into it).
    pub fn reload(&mut self) {
        self.db = Database::new();
        self.cache.clear();
        self.loaded = false;
        self.ensure_loaded();
    }

    fn rebuild_families(&mut self) {
        let mut names: Vec<String> = self
            .db
            .faces()
            .filter_map(|f| f.families.first().map(|(n, _)| n.clone()))
            .filter(|n| !n.is_empty() && !n.starts_with('.'))
            .collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        self.families = names;
    }

    pub fn families(&self) -> &[String] {
        &self.families
    }

    pub fn has_family(&self, name: &str) -> bool {
        self.families.iter().any(|f| f == name)
    }

    /// The face id for a family + style, falling back to the nearest match.
    fn face_id(&self, family: &str, bold: bool, italic: bool) -> Option<ID> {
        let q = Query {
            families: &[Family::Name(family)],
            weight: if bold { Weight::BOLD } else { Weight::NORMAL },
            stretch: Stretch::Normal,
            style: if italic { Style::Italic } else { Style::Normal },
        };
        self.db.query(&q)
    }

    /// Whether the family ships a real face for this style (else faux styling applies).
    pub fn has_style(&self, family: &str, bold: bool, italic: bool) -> bool {
        let Some(id) = self.face_id(family, bold, italic) else { return false };
        let Some(info) = self.db.face(id) else { return false };
        let w_ok = !bold || info.weight >= Weight::SEMIBOLD;
        let i_ok = !italic || matches!(info.style, Style::Italic | Style::Oblique);
        w_ok && i_ok
    }

    /// Parsed font for a family + style (cached). Falls back to the default family.
    pub fn font(&mut self, family: &str, bold: bool, italic: bool) -> Option<FontArc> {
        self.ensure_loaded();
        let key = (family.to_string(), bold, italic);
        if let Some(f) = self.cache.get(&key) {
            return f.clone();
        }
        let id = self.face_id(family, bold, italic).or_else(|| self.face_id(DEFAULT_FAMILY, bold, italic));
        let font = id.and_then(|id| self.parse(id));
        self.cache.insert(key, font.clone());
        font
    }

    fn parse(&self, id: ID) -> Option<FontArc> {
        let info = self.db.face(id)?;
        let index = info.index;
        let bytes = match &info.source {
            Source::Binary(b) => (**b).as_ref().to_vec(),
            Source::File(p) | Source::SharedFile(p, _) => std::fs::read(p).ok()?,
        };
        FontVec::try_from_vec_and_index(bytes, index).ok().map(FontArc::new)
    }
}
