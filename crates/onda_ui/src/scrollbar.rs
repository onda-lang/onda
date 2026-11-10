//! Shared scrollbar geometry for painting, track clicks, and thumb dragging.
use crate::{
    model::RunUi,
    scene::{Palette, Rect, Scene, Target},
};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollRegion {
    Document,
    Scope,
    Toolbar,
    Log,
    LogHorizontal,
    Popup,
    Menu,
}

#[derive(Clone, Copy)]
pub(crate) struct Scrollbar {
    pub track: Rect,
    pub thumb: Rect,
    pub scroll: f32,
    max_scroll: f32,
    horizontal: bool,
}
impl Scrollbar {
    fn new(rect: [f32; 4], content: f32, scroll: f32, horizontal: bool) -> Option<Self> {
        let [x, y, w, h] = rect;
        let axis = usize::from(!horizontal);
        let viewport = rect[axis + 2];
        let max_scroll = (content - viewport).max(0.0);
        if max_scroll == 0.0 || w <= 4.0 || h <= 4.0 {
            return None;
        }
        let track = if horizontal {
            [x + 2.0, y + h - 9.0, w - 4.0, 7.0]
        } else {
            [x + w - 9.0, y + 2.0, 7.0, h - 4.0]
        };
        let length = (track[axis + 2] * viewport / content)
            .max(24.0)
            .min(track[axis + 2]);
        let scroll = scroll.clamp(0.0, max_scroll);
        let mut thumb = track;
        thumb[axis] += (track[axis + 2] - length) * scroll / max_scroll;
        thumb[axis + 2] = length;
        Some(Self {
            track: Rect(track),
            thumb: Rect(thumb),
            scroll,
            max_scroll,
            horizontal,
        })
    }
    fn position(self, x: f32, y: f32) -> f32 {
        if self.horizontal {
            x
        } else {
            y
        }
    }
    pub fn grab(self, x: f32, y: f32) -> f32 {
        let axis = usize::from(!self.horizontal);
        if self.thumb.contains(x, y) {
            self.position(x, y) - self.thumb.0[axis]
        } else {
            self.thumb.0[axis + 2] * 0.5
        }
    }
    pub fn scroll_at(self, x: f32, y: f32, grab: f32) -> f32 {
        let axis = usize::from(!self.horizontal);
        let travel = self.track.0[axis + 2] - self.thumb.0[axis + 2];
        if travel <= 0.0 {
            return 0.0;
        }
        ((self.position(x, y) - grab - self.track.0[axis]) / travel).clamp(0.0, 1.0)
            * self.max_scroll
    }
}

impl Scene {
    pub fn scrollbar(
        &mut self,
        ui: &RunUi,
        p: Palette,
        region: ScrollRegion,
        viewport: [f32; 4],
        content: f32,
        scroll: f32,
    ) {
        let Some(bar) = Scrollbar::new(
            viewport,
            content,
            scroll,
            region == ScrollRegion::LogHorizontal,
        ) else {
            return;
        };
        let color = if self.hovered(ui, bar.track.0) || ui.interaction.dragging_scrollbar(region) {
            p.scrollbar_hover
        } else {
            p.scrollbar
        };
        self.shape(bar.thumb.0, color, 3.5);
        self.hit(bar.track.0, Target::Scrollbar(region));
        self.scrollbars.push((region, bar));
    }
    pub fn scrollbar_for(&self, region: ScrollRegion) -> Option<Scrollbar> {
        self.scrollbars
            .iter()
            .find(|(id, _)| *id == region)
            .map(|(_, bar)| *bar)
    }
}
