use crate::{
    editing::Field,
    events::{child_path, EventField},
    interaction::{FocusLayer, FocusTarget},
    model::RunUi,
    scene::Target,
    theme::CARD_INSET,
};
use serde_json::Value;
use std::sync::Arc;

pub(super) enum WidgetKind {
    Title(String),
    Trigger(usize),
    Group {
        label: String,
        key: String,
        open: bool,
    },
    Button {
        text: String,
        target: Target,
    },
    Scalar {
        label: String,
        field: Field,
        editor: [f32; 4],
    },
}
pub(super) struct Widget {
    pub rect: [f32; 4],
    pub kind: WidgetKind,
}
pub(crate) struct Layout {
    widgets: Vec<Widget>,
    pub(super) focus: Arc<[FocusTarget]>,
    pub(super) height: f32,
    cards: Vec<[f32; 4]>,
    width: f32,
}
impl Layout {
    pub(super) fn visible_widgets(&self, top: f32, bottom: f32) -> &[Widget] {
        visible(&self.widgets, |w| w.rect, top, bottom)
    }
    pub(super) fn visible_cards(&self, top: f32, bottom: f32) -> &[[f32; 4]] {
        visible(&self.cards, |r| *r, top, bottom)
    }
    pub(super) fn matches(&self, width: f32) -> bool {
        self.width == width
    }
    pub(super) fn new(ui: &RunUi, width: f32) -> Self {
        let mut builder = Builder {
            ui,
            width,
            y: 0.0,
            event: 0,
            arg: 0,
            widgets: Vec::new(),
            focus: Vec::new(),
        };
        let mut cards = Vec::new();
        for (i, event) in ui.events.iter().enumerate() {
            builder.event = i;
            let top = builder.y;
            let trigger_width = if width < 180.0 { 28.0 } else { 80.0 };
            builder.push(
                [
                    CARD_INSET,
                    top + 8.0,
                    (width - 2.0 * CARD_INSET - trigger_width - 8.0).max(1.0),
                    28.0,
                ],
                WidgetKind::Title(event.name.clone()),
            );
            let trigger = [
                width - CARD_INSET - trigger_width,
                top + 8.0,
                trigger_width,
                28.0,
            ];
            builder.focus(trigger, Target::Trigger(i));
            builder.push(trigger, WidgetKind::Trigger(i));
            builder.y += 40.0;
            for (j, arg) in event.args.iter().enumerate() {
                builder.arg = j;
                builder.node(&arg.name, &arg.shape, &arg.value, "", 0);
            }
            builder.y += 4.0;
            cards.push([0.0, top, width, builder.y - top]);
            builder.y += 6.0;
        }
        Self {
            height: (builder.y - 6.0).max(0.0),
            widgets: builder.widgets,
            focus: builder.focus.into(),
            cards,
            width,
        }
    }
}

// Rows and non-overlapping cards are ordered by both their top and bottom edges.
fn visible<T>(items: &[T], rect: impl Fn(&T) -> [f32; 4], top: f32, bottom: f32) -> &[T] {
    let start = items.partition_point(|item| {
        let r = rect(item);
        r[1] + r[3] < top
    });
    let end = items
        .partition_point(|item| rect(item)[1] < bottom)
        .max(start);
    &items[start..end]
}

struct Builder<'a> {
    ui: &'a RunUi,
    width: f32,
    y: f32,
    event: usize,
    arg: usize,
    widgets: Vec<Widget>,
    focus: Vec<FocusTarget>,
}
impl Builder<'_> {
    fn push(&mut self, rect: [f32; 4], kind: WidgetKind) {
        self.widgets.push(Widget { rect, kind });
    }
    fn focus(&mut self, rect: [f32; 4], target: Target) {
        self.focus.push(FocusTarget {
            rect,
            target,
            layer: FocusLayer::Document,
        });
    }
    fn button(&mut self, text: String, rect: [f32; 4], target: Target) {
        self.focus(rect, target.clone());
        self.push(rect, WidgetKind::Button { text, target });
    }
    fn field(&self, path: &str, scalar: &str) -> Field {
        Field::Event(EventField {
            event: self.event,
            arg: self.arg,
            path: path.into(),
            scalar: scalar.into(),
        })
    }
    fn node(&mut self, label: &str, shape: &Value, value: &Value, path: &str, depth: usize) {
        let indent =
            (depth.min(8) as f32 * 12.0).min((self.width - 2.0 * CARD_INSET - 80.0).max(0.0));
        let x = CARD_INSET + indent;
        let width = (self.width - x - CARD_INSET).max(1.0);
        if shape["kind"] == "scalar" {
            let field = self.field(path, shape["encoding"].as_str().unwrap_or("f64"));
            let field_width = 112.0_f32.min(width * 0.5);
            let editor = [x + width - field_width, self.y + 2.0, field_width, 24.0];
            let focus = if shape["encoding"] == "bool" {
                [
                    editor[0] + (field_width - 24.0) * 0.5,
                    editor[1],
                    24.0,
                    24.0,
                ]
            } else {
                editor
            };
            self.focus(
                focus,
                if self.ui.field_enumeration(&field).is_some() {
                    Target::Choose(crate::interaction::SelectId::Field(field.clone()))
                } else {
                    Target::Field(field.clone())
                },
            );
            self.push(
                [x, self.y, width, 32.0],
                WidgetKind::Scalar {
                    label: label.into(),
                    field,
                    editor,
                },
            );
            self.y += 32.0;
            return;
        }
        let key = format!("event/{}/{}/{}", self.event, self.arg, path);
        let folded = self.ui.presentation.folded.contains(&key);
        let heading = [x, self.y, width, 28.0];
        self.focus(heading, Target::Fold(key.clone()));
        self.push(
            heading,
            WidgetKind::Group {
                label: label.into(),
                key,
                open: !folded,
            },
        );
        self.y += 34.0;
        if folded {
            return;
        }
        match shape["kind"].as_str() {
            Some("struct") => {
                if let Some(fields) = shape["fields"].as_array() {
                    for f in fields {
                        if let Some(name) = f["name"].as_str() {
                            self.node(
                                name,
                                &f["ty"],
                                &value[name],
                                &child_path(path, name),
                                depth + 1,
                            );
                        }
                    }
                }
            }
            Some("tuple") => {
                if let Some(shapes) = shape["elements"].as_array() {
                    for (i, shape) in shapes.iter().enumerate() {
                        self.node(
                            &format!("[{i}]"),
                            shape,
                            &value[i],
                            &child_path(path, &i.to_string()),
                            depth + 1,
                        );
                    }
                }
            }
            Some("array" | "slice") => {
                if let Some(values) = value.as_array() {
                    for (i, value) in values.iter().enumerate() {
                        self.node(
                            &format!("[{i}]"),
                            &shape["element"],
                            value,
                            &child_path(path, &i.to_string()),
                            depth + 1,
                        );
                    }
                }
                if shape["kind"] == "slice" {
                    let button_width = ((width - 6.0) * 0.5).clamp(1.0, 72.0);
                    for (i, (text, add)) in
                        [("Add", true), ("Remove", false)].into_iter().enumerate()
                    {
                        self.button(
                            text.into(),
                            [
                                x + i as f32 * (button_width + 6.0),
                                self.y,
                                button_width,
                                28.0,
                            ],
                            Target::Slice {
                                event: self.event,
                                arg: self.arg,
                                path: path.into(),
                                add,
                            },
                        );
                    }
                    self.y += 36.0;
                }
            }
            _ => {}
        }
    }
}
