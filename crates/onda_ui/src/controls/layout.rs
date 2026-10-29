//! Structural control-bank geometry and focus order, retained across value updates.
use crate::{
    editing::Field,
    interaction::{FocusLayer, FocusTarget},
    model::{Control, RunUi},
    scene::Target,
};
use std::sync::Arc;

pub(crate) struct Layout {
    pub(super) items: Vec<Item>,
    pub(super) focus: Arc<[FocusTarget]>,
    pub(super) height: f32,
    pub(super) row: f32,
    pub(super) card_width: f32,
    width: f32,
    knobs: bool,
}
impl Layout {
    pub(super) fn new(ui: &RunUi, width: f32, input: bool) -> Self {
        let knobs = ui.bank_knobs(input);
        let cols = columns(width, knobs);
        let row = if knobs { 140.0 } else { 80.0 };
        let card_width = (width - (cols - 1) as f32 * 8.0) / cols as f32;
        let (items, height) = layout(ui, width, input);
        let focus = items
            .iter()
            .filter_map(|item| {
                let (target, rect) = match item {
                    Item::Group { name, y, .. } => (
                        Target::Fold(format!("bank/{input}/{name}")),
                        [0.0, *y, width, 26.0],
                    ),
                    Item::Control { index, col, y } => {
                        let c = &ui.controls[*index];
                        if !c.enabled() {
                            return None;
                        }
                        (
                            Target::Field(Field::Control(*index)),
                            field_rect(
                                c,
                                knobs,
                                [*col as f32 * (card_width + 8.0), *y, card_width, row],
                            ),
                        )
                    }
                };
                Some(FocusTarget {
                    target,
                    rect,
                    layer: FocusLayer::Document,
                })
            })
            .collect();
        Self {
            items,
            focus,
            height,
            row,
            card_width,
            width,
            knobs,
        }
    }
    pub(super) fn matches(&self, ui: &RunUi, width: f32, input: bool) -> bool {
        self.width == width && self.knobs == ui.bank_knobs(input)
    }
    pub(super) fn visible(&self, top: f32, bottom: f32) -> &[Item] {
        // Rows share a top edge, and both edges are monotonic across groups.
        let start = self
            .items
            .partition_point(|item| item.y() + item.height(self.row) < top);
        let end = self
            .items
            .partition_point(|item| item.y() < bottom)
            .max(start);
        &self.items[start..end]
    }
}
impl Item {
    fn y(&self) -> f32 {
        match self {
            Self::Group { y, .. } | Self::Control { y, .. } => *y,
        }
    }
    fn height(&self, row: f32) -> f32 {
        match self {
            Self::Group { .. } => 28.0,
            Self::Control { .. } => row,
        }
    }
}

pub(super) fn field_rect(c: &Control, knobs: bool, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
    if c.boolean() {
        return [x + (w - 24.0) * 0.5, y + h * 0.5, 24.0, 24.0];
    }
    let width = 104.0_f32.min(w - 18.0);
    if knobs {
        [x + (w - width) * 0.5, y + 101.0, width, 26.0]
    } else {
        [x + w - 9.0 - width, y + 8.0, width, 26.0]
    }
}

fn columns(width: f32, knobs: bool) -> usize {
    let minimum = if knobs {
        if width <= 358.0 {
            80.0
        } else {
            140.0
        }
    } else {
        200.0
    };
    let maximum = if knobs && width <= 358.0 { 2.0 } else { 8.0 };
    ((width + 8.0) / (minimum + 8.0))
        .floor()
        .clamp(1.0, maximum) as usize
}
pub(super) enum Item {
    Group { name: String, y: f32, folded: bool },
    Control { index: usize, col: usize, y: f32 },
}
fn layout(ui: &RunUi, width: f32, input: bool) -> (Vec<Item>, f32) {
    let knobs = ui.bank_knobs(input);
    let cols = columns(width, knobs);
    let height = if knobs { 140.0 } else { 80.0 };
    let mut items = Vec::new();
    let mut y = 0.0;
    let mut slot = 0;
    let mut groups = std::collections::HashMap::<&str, Vec<usize>>::new();
    for (index, c) in ui
        .controls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.input == input)
    {
        if let Some(name) = c.metadata["array"]["name"].as_str() {
            groups.entry(name).or_default().push(index);
        }
    }
    for (index, c) in ui
        .controls
        .iter()
        .enumerate()
        .filter(|(_, c)| c.input == input)
    {
        if let Some(name) = c.metadata["array"]["name"].as_str() {
            let Some(indices) = groups.remove(name) else {
                continue;
            };
            if slot > 0 {
                y += height + 8.0;
                slot = 0;
            }
            let folded = ui
                .presentation
                .folded
                .contains(&format!("bank/{input}/{name}"));
            items.push(Item::Group {
                name: name.into(),
                y,
                folded,
            });
            y += 30.0;
            if !folded {
                for index in indices {
                    items.push(Item::Control {
                        index,
                        col: slot,
                        y,
                    });
                    slot += 1;
                    if slot == cols {
                        y += height + 8.0;
                        slot = 0;
                    }
                }
                if slot > 0 {
                    y += height + 8.0;
                    slot = 0;
                }
            }
        } else {
            items.push(Item::Control {
                index,
                col: slot,
                y,
            });
            slot += 1;
            if slot == cols {
                y += height + 8.0;
                slot = 0;
            }
        }
    }
    let bottom = items.last().map_or(0.0, |item| match item {
        Item::Group { y, .. } => y + 26.0,
        Item::Control { y, .. } => y + height,
    });
    (items, bottom)
}
