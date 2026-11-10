//! Deterministic primary fonts, with script fallbacks loaded only when needed.
use glyphon::fontdb;
#[cfg(not(target_arch = "wasm32"))]
use glyphon::FontSystem;
use std::sync::{Arc, LazyLock};

const INTER: &[u8] = include_bytes!("../../../assets/fonts/Inter.ttf");
const MONO: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono.ttf");
pub(crate) const READING_SIZE: f32 = 13.0;

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

fn inter_face(bold: bool) -> &'static ttf_parser::Face<'static> {
    static FACES: LazyLock<[ttf_parser::Face<'static>; 2]> = LazyLock::new(|| {
        [400.0, 600.0].map(|weight| {
            let mut face = ttf_parser::Face::parse(INTER, 0).expect("valid bundled font");
            face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), weight);
            face
        })
    });
    &FACES[usize::from(bold)]
}

pub(crate) fn heading_width(text: &str, size: f32) -> f32 {
    proportional_width(text, size, true)
}

pub(crate) fn proportional_width(text: &str, size: f32, bold: bool) -> f32 {
    let face = inter_face(bold);
    text.chars()
        .filter_map(|c| face.glyph_index(c))
        .filter_map(|glyph| face.glyph_hor_advance(glyph))
        .map(f32::from)
        .sum::<f32>()
        * size
        / f32::from(face.units_per_em())
}

/// Baseline relative to the vertical centre of a single Inter line.
pub(crate) fn proportional_baseline_offset(size: f32) -> f32 {
    let face = inter_face(false);
    (f32::from(face.ascender()) + f32::from(face.descender())) * 0.5 * size
        / f32::from(face.units_per_em())
}

/// Rightmost visible ink in a centered, clipped monospace label.
pub(crate) fn centered_monospace_right(text: &str, size: f32, width: f32) -> f32 {
    static FACE: LazyLock<ttf_parser::Face<'static>> =
        LazyLock::new(|| ttf_parser::Face::parse(MONO, 0).expect("valid bundled font"));
    let scale = size / f32::from(FACE.units_per_em());
    let mut advance = 0.0_f32;
    let mut right = 0.0_f32;
    for glyph in text.chars().filter_map(|c| FACE.glyph_index(c)) {
        if let Some(bounds) = FACE.glyph_bounding_box(glyph) {
            right = right.max(advance + f32::from(bounds.x_max));
        }
        advance += f32::from(FACE.glyph_hor_advance(glyph).unwrap_or(0));
    }
    ((width - advance * scale).max(0.0) * 0.5 + right * scale).min(width)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_fallback(fonts: &mut FontSystem, name: &str) {
    if let Some(font) = FALLBACKS.iter().find(|f| f.name == name) {
        fonts
            .db_mut()
            .load_font_source(fontdb::Source::Binary(Arc::new(font.data)));
    }
}
