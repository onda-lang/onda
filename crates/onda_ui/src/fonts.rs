//! Deterministic primary fonts, with script fallbacks loaded only when needed.
#[cfg(not(target_arch = "wasm32"))]
use glyphon::FontSystem;
use glyphon::fontdb;
use std::sync::{Arc, LazyLock};

const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono.ttf");
pub(crate) struct Fallback {
    pub name: &'static str,
    ranges: &'static [(u32, u32)],
    #[cfg(not(target_arch = "wasm32"))]
    data: &'static [u8],
}
include!(concat!(env!("OUT_DIR"), "/fallbacks.rs"));
impl Fallback {
    pub fn covers(&self, c: char) -> bool {
        let point = c as u32;
        let i = self.ranges.partition_point(|r| r.1 < point);
        self.ranges.get(i).is_some_and(|r| r.0 <= point)
    }
}

pub(crate) fn database() -> fontdb::Database {
    let mut db = fontdb::Database::new();
    for bytes in [INTER, MONO] {
        db.load_font_source(fontdb::Source::Binary(Arc::new(bytes)));
    }
    db.set_sans_serif_family("Inter");
    db.set_monospace_family("JetBrains Mono");
    db
}

pub(crate) fn primary_covers(c: char) -> bool {
    static FACES: LazyLock<[ttf_parser::Face<'static>; 2]> = LazyLock::new(|| {
        [INTER, MONO].map(|bytes| ttf_parser::Face::parse(bytes, 0).expect("valid bundled font"))
    });
    c.is_ascii() || c.is_control() || FACES.iter().any(|face| face.glyph_index(c).is_some())
}

pub(crate) fn heading_width(text: &str, size: f32) -> f32 {
    static FACE: LazyLock<ttf_parser::Face<'static>> = LazyLock::new(|| {
        let mut face = ttf_parser::Face::parse(INTER, 0).expect("valid bundled font");
        face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), 600.0);
        face
    });
    text.chars()
        .filter_map(|c| FACE.glyph_index(c))
        .filter_map(|glyph| FACE.glyph_hor_advance(glyph))
        .map(f32::from)
        .sum::<f32>()
        * size
        / f32::from(FACE.units_per_em())
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_fallback(fonts: &mut FontSystem, name: &str) {
    if let Some(font) = FALLBACKS.iter().find(|f| f.name == name) {
        fonts
            .db_mut()
            .load_font_source(fontdb::Source::Binary(Arc::new(font.data)));
    }
}
