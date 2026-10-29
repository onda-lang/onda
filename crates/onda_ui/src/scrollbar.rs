//! Shared scrollbar geometry for painting, track clicks, and thumb dragging.
use crate::scene::Rect;

#[derive(Clone, Copy)]
pub(crate) struct Scrollbar {
    pub track: Rect,
    pub thumb: Rect,
    max_scroll: f32,
}
impl Scrollbar {
    pub fn new(width: f32, viewport: f32, content: f32, scroll: f32) -> Option<Self> {
        let max_scroll = content - viewport;
        if max_scroll <= 0.0 {
            return None;
        }
        let track = [width - 9.0, 2.0, 7.0, (viewport - 4.0).max(1.0)];
        let height = (track[3] * viewport / content).max(24.0).min(track[3]);
        let thumb = [
            track[0],
            track[1] + (track[3] - height) * (scroll / max_scroll).clamp(0.0, 1.0),
            track[2],
            height,
        ];
        Some(Self {
            track: Rect(track),
            thumb: Rect(thumb),
            max_scroll,
        })
    }
    pub fn scroll_at(self, y: f32, grab: f32) -> f32 {
        let travel = self.track.0[3] - self.thumb.0[3];
        if travel <= 0.0 {
            return 0.0;
        }
        ((y - grab - self.track.0[1]) / travel).clamp(0.0, 1.0) * self.max_scroll
    }
}
