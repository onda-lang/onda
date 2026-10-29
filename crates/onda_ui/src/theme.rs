//! Run-view colors and dimensions, matching ui/run/run.html and onda_egui.
pub(crate) const GUTTER: f32 = 12.0;
pub(crate) const GAP: f32 = 12.0;
pub(crate) const SECTION_HEADER_HEIGHT: f32 = 40.0;
pub(crate) const SECTION_INSET: f32 = 9.0;
pub(crate) const CARD_INSET: f32 = 10.0;
pub(crate) const CONTROL_RADIUS: f32 = 5.0;
pub(crate) const CARD_RADIUS: f32 = 9.0;
pub(crate) const SECTION_RADIUS: f32 = 12.0;
pub(crate) fn rgb(hex: u32) -> [f32; 4] {
    [
        ((hex >> 16) & 255) as f32 / 255.0,
        ((hex >> 8) & 255) as f32 / 255.0,
        (hex & 255) as f32 / 255.0,
        1.0,
    ]
}
pub(crate) fn text(hex: u32) -> [u8; 3] {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}
pub(crate) fn blend(a: [f32; 4], b: [f32; 4], weight: f32) -> [f32; 4] {
    std::array::from_fn(|i| a[i] * weight + b[i] * (1.0 - weight))
}
#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub background: [f32; 4],
    pub panel: [f32; 4],
    pub card: [f32; 4],
    pub field: [f32; 4],
    pub border: [f32; 4],
    pub ink: [u8; 3],
    pub muted: [u8; 3],
    pub accent: [f32; 4],
    pub accent_strong: [f32; 4],
    pub accent_soft: [f32; 4],
    pub success: [f32; 4],
    pub danger: [f32; 4],
    pub scope: [f32; 4],
    pub piano_white: [f32; 4],
    pub piano_black: [f32; 4],
    pub piano_active: [f32; 4],
    pub piano_black_active: [f32; 4],
    pub scrollbar: [f32; 4],
    pub scrollbar_hover: [f32; 4],
}
impl Palette {
    pub fn new(light: bool) -> Self {
        let background = rgb(if light { 0xe8f1fc } else { 0x081522 });
        if light {
            Self {
                background,
                panel: rgb(0xf5f9ff),
                card: rgb(0xf4f9ff),
                field: rgb(0xffffff),
                border: rgb(0xbed1e5),
                ink: text(0x143a63),
                muted: text(0x4e6a86),
                accent: rgb(0x143a63),
                accent_strong: rgb(0x0a213b),
                accent_soft: rgb(0xdbe8f7),
                success: rgb(0x136c4c),
                danger: rgb(0xb53333),
                scope: background,
                piano_white: rgb(0xf8fbff),
                piano_black: rgb(0x143a63),
                piano_active: rgb(0x9bb8d8),
                piano_black_active: rgb(0x2f76c5),
                scrollbar: rgb(0x9bb8d8),
                scrollbar_hover: rgb(0x6f94ba),
            }
        } else {
            Self {
                background,
                panel: rgb(0x0c1e33),
                card: rgb(0x121f31),
                field: rgb(0x0d1827),
                border: rgb(0x354e6d),
                ink: text(0xd8e5f3),
                muted: text(0x92a8bf),
                accent: rgb(0xc9dbf2),
                accent_strong: rgb(0xe0ebfa),
                accent_soft: rgb(0x233d5c),
                success: rgb(0x7ad7ba),
                danger: rgb(0xff7e7e),
                scope: background,
                piano_white: rgb(0xafc2d9),
                piano_black: rgb(0x09131f),
                piano_active: rgb(0x7bade6),
                piano_black_active: rgb(0x7bade6),
                scrollbar: rgb(0x2d5278),
                scrollbar_hover: rgb(0x47739f),
            }
        }
    }
    pub fn ink_color(self) -> [f32; 4] {
        [
            self.ink[0] as f32 / 255.0,
            self.ink[1] as f32 / 255.0,
            self.ink[2] as f32 / 255.0,
            1.0,
        ]
    }
    pub fn clear(self, srgb: bool) -> wgpu::Color {
        let convert = |n: f32| {
            if !srgb {
                n as f64
            } else if n <= 0.04045 {
                (n / 12.92) as f64
            } else {
                ((n + 0.055) / 1.055).powf(2.4) as f64
            }
        };
        wgpu::Color {
            r: convert(self.background[0]),
            g: convert(self.background[1]),
            b: convert(self.background[2]),
            a: 1.0,
        }
    }
}
