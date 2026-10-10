//! Number editing with host-owned drafts and horizontal drag accumulation.

use eframe::egui;
use onda_run::{ParamDomain, ParamScalarType, ParamScale};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum NumberValue {
    Float(f64),
    Integer(i64),
}

impl NumberValue {
    pub(crate) fn as_f64(self) -> f64 {
        match self {
            Self::Float(value) => value,
            Self::Integer(value) => value as f64,
        }
    }

    pub(crate) fn as_i64(self) -> i64 {
        match self {
            Self::Float(value) => value.round() as i64,
            Self::Integer(value) => value,
        }
    }

    fn text(self, max_decimals: Option<usize>) -> String {
        match self {
            Self::Float(value) => max_decimals.map_or_else(
                || value.to_string(),
                |decimals| egui::emath::format_with_decimals_in_range(value, 0..=decimals),
            ),
            Self::Integer(value) => value.to_string(),
        }
    }

    fn edit_text(self) -> String {
        let Self::Float(value) = self else {
            return self.text(None);
        };
        if value == 0.0 {
            return "0".into();
        }
        let text = if value.abs() < 0.0001 || value.abs() >= 1.0e9 {
            format!("{value:.5e}")
        } else {
            format!("{value:.5}")
        };
        let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, ""));
        let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
        if exponent.is_empty() {
            mantissa.to_owned()
        } else {
            format!("{mantissa}e{exponent}")
        }
    }

    fn parse(self, text: &str) -> Option<Self> {
        match self {
            Self::Float(_) => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())
                .map(Self::Float),
            Self::Integer(_) => text.trim().parse().ok().map(Self::Integer),
        }
    }

    fn offset(self, delta: f64) -> Self {
        match self {
            Self::Float(value) => Self::Float(value + delta),
            Self::Integer(value) => Self::Integer(value.saturating_add(delta as i64)),
        }
    }

    fn clamp(self, minimum: Self, maximum: Self) -> Self {
        match (self, minimum, maximum) {
            (Self::Float(value), Self::Float(min), Self::Float(max)) => {
                Self::Float(value.clamp(min, max))
            }
            (Self::Integer(value), Self::Integer(min), Self::Integer(max)) => {
                Self::Integer(value.clamp(min, max))
            }
            _ => unreachable!("number bounds must have the value's type"),
        }
    }
}

#[derive(Clone)]
struct NumberDraft {
    text: String,
    original: NumberValue,
    changed: bool,
}

#[derive(Clone, Copy, PartialEq)]
struct DragRange {
    domain: ParamDomain<'static>,
    points: f64,
}

impl DragRange {
    fn position(self, value: f64) -> f64 {
        self.domain.plain_to_normalized(value) * self.points
    }

    fn value(self, position: f64) -> f64 {
        self.domain.normalized_to_plain(position / self.points)
    }
}

#[derive(Clone, Copy, PartialEq)]
enum DragSpeed {
    Absolute(f64),
    Range(DragRange),
}

#[derive(Clone)]
struct NumberDrag {
    value: NumberValue,
    start_x: f32,
    previous_x: f32,
    remainder: f64,
    range_position: f64,
    dragging: bool,
}

impl NumberDrag {
    fn move_to(
        &mut self,
        x: f32,
        current_value: NumberValue,
        speed: DragSpeed,
        precision: f64,
        minimum: NumberValue,
        maximum: NumberValue,
    ) {
        if !self.dragging {
            if (x - self.start_x).abs() < 3.0 {
                return;
            }
            // Take ownership only once dragging starts, after any host updates.
            self.value = current_value;
            if let DragSpeed::Range(range) = speed {
                self.range_position = range.position(current_value.as_f64());
            }
            self.dragging = true;
        }
        let movement = f64::from(x - self.previous_x) * precision;
        self.previous_x = x;
        let delta = match speed {
            DragSpeed::Absolute(speed) => movement * speed,
            DragSpeed::Range(range) => {
                // Accumulate in points so even subnormal ranges retain small movements.
                self.range_position = (self.range_position + movement).clamp(0.0, range.points);
                self.value = NumberValue::Float(range.value(self.range_position));
                return;
            }
        };
        self.value = match (self.value, minimum, maximum) {
            (NumberValue::Float(value), NumberValue::Float(min), NumberValue::Float(max)) => {
                NumberValue::Float((value + delta).clamp(min, max))
            }
            (NumberValue::Integer(value), NumberValue::Integer(min), NumberValue::Integer(max)) => {
                let amount = self.remainder + delta;
                // Match JavaScript's Math.round, including negative half steps.
                let floor = amount.floor();
                let units = if amount - floor >= 0.5 {
                    floor + 1.0
                } else {
                    floor
                };
                let next = i128::from(value).saturating_add(units as i128);
                let clamped = next.clamp(i128::from(min), i128::from(max));
                let remainder = amount - units;
                // Fractional overshoot must not delay reversal at a bound.
                let outward = (clamped == i128::from(min) && remainder < 0.0)
                    || (clamped == i128::from(max) && remainder > 0.0);
                self.remainder = if next == clamped && !outward {
                    remainder
                } else {
                    0.0
                };
                NumberValue::Integer(clamped as i64)
            }
            _ => unreachable!("number bounds must have the value's type"),
        };
    }
}

#[derive(Clone, Copy, PartialEq)]
struct NumberDomain {
    minimum: NumberValue,
    maximum: NumberValue,
    speed: DragSpeed,
    step: f64,
}

#[derive(Clone)]
struct NumberInputState {
    domain: NumberDomain,
    draft: Option<NumberDraft>,
    drag: Option<NumberDrag>,
    last_seen_pass: u64,
}

impl NumberInputState {
    fn new(domain: NumberDomain) -> Self {
        Self {
            domain,
            draft: None,
            drag: None,
            last_seen_pass: 0,
        }
    }

    fn begin_edit(&mut self, ctx: &egui::Context, id: egui::Id, value: NumberValue) {
        let text = value.edit_text();
        let mut text_state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        text_state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::default(),
                egui::text::CCursor::new(text.chars().count()),
            )));
        text_state.store(ctx, id);
        self.draft = Some(NumberDraft {
            text,
            original: value,
            changed: false,
        });
    }
}

pub(crate) struct NumberInput<'a> {
    id_salt: Option<egui::Id>,
    value: &'a mut NumberValue,
    reset: NumberValue,
    minimum: NumberValue,
    maximum: NumberValue,
    speed: DragSpeed,
    step: f64,
    max_decimals: usize,
    prefix: &'a str,
    suffix: &'a str,
    help: &'a str,
    update_while_editing: bool,
}

impl<'a> NumberInput<'a> {
    pub(crate) fn new(value: &'a mut NumberValue, reset: NumberValue, speed: f64) -> Self {
        let (minimum, maximum) = match value {
            NumberValue::Float(_) => (NumberValue::Float(-f64::MAX), NumberValue::Float(f64::MAX)),
            NumberValue::Integer(_) => (
                NumberValue::Integer(i64::MIN),
                NumberValue::Integer(i64::MAX),
            ),
        };
        Self {
            id_salt: None,
            value,
            reset,
            minimum,
            maximum,
            speed: DragSpeed::Absolute(speed),
            step: if matches!(minimum, NumberValue::Integer(_)) {
                1.0
            } else {
                0.001
            },
            max_decimals: 8,
            prefix: "",
            suffix: "",
            help: "",
            update_while_editing: true,
        }
    }

    pub(crate) fn id_salt(mut self, salt: impl std::hash::Hash) -> Self {
        self.id_salt = Some(egui::Id::new(salt));
        self
    }

    pub(crate) fn range(mut self, minimum: f64, maximum: f64) -> Self {
        (self.minimum, self.maximum) = match self.value {
            NumberValue::Float(_) => (NumberValue::Float(minimum), NumberValue::Float(maximum)),
            NumberValue::Integer(_) => (
                NumberValue::Integer(minimum.ceil() as i64),
                NumberValue::Integer(maximum.floor() as i64),
            ),
        };
        self
    }

    pub(crate) fn drag_range(self, minimum: f64, maximum: f64, points: f64) -> Self {
        let domain = ParamDomain::new(
            ParamScalarType::F64,
            minimum,
            maximum,
            ParamScale::Linear,
            None,
            None,
            None,
            None,
        )
        .expect("number drag ranges must have finite, ordered bounds");
        self.drag_domain(domain, points)
    }

    pub(crate) fn drag_domain(self, domain: ParamDomain<'_>, points: f64) -> Self {
        debug_assert!(matches!(self.value, NumberValue::Float(_)));
        debug_assert!(points.is_finite() && points > 0.0);
        // Accumulate continuously; the owner applies its declared step grid.
        // Units belong to presentation and need not be retained in widget state.
        let domain = ParamDomain::new(
            ParamScalarType::F64,
            domain.minimum(),
            domain.maximum(),
            domain.scale(),
            domain.curve(),
            None,
            None,
            None,
        )
        .expect("a prepared parameter domain has a valid continuous drag mapping");
        let mut input = self.range(domain.minimum(), domain.maximum());
        input.speed = DragSpeed::Range(DragRange { domain, points });
        input
    }

    pub(crate) fn max_decimals(mut self, decimals: usize) -> Self {
        self.max_decimals = decimals;
        self
    }

    pub(crate) fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    pub(crate) fn prefix(mut self, prefix: &'a str) -> Self {
        self.prefix = prefix;
        self
    }

    pub(crate) fn suffix(mut self, suffix: &'a str) -> Self {
        self.suffix = suffix;
        self
    }

    pub(crate) fn help(mut self, help: &'a str) -> Self {
        self.help = help;
        self
    }

    pub(crate) fn update_while_editing(mut self, update: bool) -> Self {
        self.update_while_editing = update;
        self
    }

    fn drag_button(&self, ui: &mut egui::Ui, id: egui::Id) -> egui::Response {
        let text = egui::WidgetText::from(
            egui::RichText::new(format!(
                "{}{}{}",
                self.prefix,
                self.value.text(Some(self.max_decimals)),
                self.suffix
            ))
            .text_style(ui.style().drag_value_text_style.clone()),
        );
        let galley = text.into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Button,
        );
        let padding = ui.spacing().button_padding;
        let mut size = (galley.size() + 2.0 * padding).max(ui.spacing().interact_size);
        size.x = size.x.min(ui.available_width());
        let (_, rect) = ui.allocate_space(size);
        let mut response = ui.interact(rect, id, egui::Sense::click_and_drag());
        response.intrinsic_size = Some(size);
        if ui.is_rect_visible(rect) {
            let visuals = ui.style().interact(&response);
            ui.painter().rect(
                rect.expand(visuals.expansion),
                visuals.corner_radius,
                visuals.weak_bg_fill,
                visuals.bg_stroke,
                egui::StrokeKind::Inside,
            );
            let text_rect = ui
                .layout()
                .align_size_within_rect(galley.size(), rect.shrink2(padding));
            ui.painter()
                .with_clip_rect(ui.clip_rect().intersect(rect.shrink2(padding)))
                .galley(text_rect.min, galley, visuals.text_color());
        }
        response.on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal)
    }
}

impl egui::Widget for NumberInput<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let id = self
            .id_salt
            .map_or_else(|| ui.next_auto_id(), |salt| ui.make_persistent_id(salt));
        let pass = ui.ctx().cumulative_pass_nr();
        let domain = NumberDomain {
            minimum: self.minimum,
            maximum: self.maximum,
            speed: self.speed,
            step: self.step,
        };
        let editing = ui.is_enabled()
            && ui.memory_mut(|memory| {
                memory.interested_in_focus(id, ui.layer_id());
                memory.has_focus(id)
            });
        let mut state = ui
            .data_mut(|data| data.get_temp::<NumberInputState>(id))
            .filter(|state| {
                // Reused IDs must not carry interactions across absence or domain changes.
                state.last_seen_pass.saturating_add(1) >= pass && state.domain == domain
            })
            .unwrap_or_else(|| NumberInputState::new(domain));
        state.last_seen_pass = pass;
        let old_value = *self.value;
        let (escape, enter) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Escape),
                input.key_pressed(egui::Key::Enter),
            )
        });
        if !ui.is_enabled() {
            state = NumberInputState::new(domain);
        } else if !editing {
            if let Some(draft) = state.draft.take() {
                if escape && self.update_while_editing {
                    *self.value = draft.original;
                } else if !escape && !self.update_while_editing && draft.changed {
                    if let Some(value) = self.value.parse(&draft.text) {
                        *self.value = value.clamp(self.minimum, self.maximum);
                    }
                }
            }
        }

        let mut response = if editing {
            if state.draft.is_none() {
                state.begin_edit(ui.ctx(), id, *self.value);
            }
            let draft = state.draft.as_mut().expect("initialized number draft");
            let increment = ui.input_mut(|input| {
                input.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp) as f64
                    - input.count_and_consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                        as f64
            });
            if increment != 0.0 {
                let value = if draft.changed {
                    self.value.parse(&draft.text).unwrap_or(*self.value)
                } else {
                    *self.value
                };
                draft.text = value
                    .offset(increment * self.step)
                    .clamp(self.minimum, self.maximum)
                    .text(None);
            }
            let mut response = ui.add(
                egui::TextEdit::singleline(&mut draft.text)
                    .id(id)
                    .font(ui.style().drag_value_text_style.clone())
                    .horizontal_align(ui.layout().horizontal_align())
                    .vertical_align(ui.layout().vertical_align())
                    .margin(ui.spacing().button_padding)
                    .min_size(ui.spacing().interact_size)
                    .desired_width(ui.spacing().interact_size.x),
            );
            if increment != 0.0 {
                response.mark_changed();
            }
            draft.changed |= response.changed();
            if !response.double_clicked() {
                if escape {
                    if self.update_while_editing {
                        *self.value = draft.original;
                    }
                } else if (self.update_while_editing && response.changed())
                    || (!self.update_while_editing
                        && draft.changed
                        && (response.lost_focus() || enter))
                {
                    // Reapplying an unchanged live draft would overwrite an owner reset.
                    if let Some(value) = self.value.parse(&draft.text) {
                        *self.value = value.clamp(self.minimum, self.maximum);
                    }
                }
                if response.lost_focus() || escape || enter {
                    state.draft = None;
                    response.surrender_focus();
                }
            }
            response
        } else {
            self.drag_button(ui, id)
        };

        if ui.is_enabled() && response.double_clicked_by(egui::PointerButton::Primary) {
            *self.value = self.reset.clamp(self.minimum, self.maximum);
            state = NumberInputState::new(domain);
            response.surrender_focus();
            response.mark_changed();
        } else if ui.is_enabled() && !editing {
            if response.is_pointer_button_down_on()
                && ui.input(|input| input.pointer.primary_pressed())
                && state.drag.is_none()
            {
                if let Some(origin) = ui.input(|input| input.pointer.press_origin()) {
                    state.drag = Some(NumberDrag {
                        value: *self.value,
                        start_x: origin.x,
                        previous_x: origin.x,
                        remainder: 0.0,
                        range_position: 0.0,
                        dragging: false,
                    });
                }
            }
            if let Some(drag) = &mut state.drag {
                if let Some(position) = ui.input(|input| input.pointer.interact_pos()) {
                    let precision = ui.input(|input| if input.modifiers.shift { 0.1 } else { 1.0 });
                    drag.move_to(
                        position.x,
                        *self.value,
                        self.speed,
                        precision,
                        self.minimum,
                        self.maximum,
                    );
                    if drag.dragging {
                        *self.value = drag.value;
                    }
                }
            }
            let click = response.clicked_by(egui::PointerButton::Primary)
                || (response.drag_stopped()
                    && state.drag.as_ref().is_some_and(|drag| !drag.dragging));
            if response.drag_stopped() || !ui.input(|input| input.pointer.primary_down()) {
                state.drag = None;
            }
            if click {
                response.request_focus();
                state.begin_edit(ui.ctx(), id, *self.value);
            }
        }
        if *self.value != old_value {
            response.mark_changed();
        }
        ui.data_mut(|data| {
            if state.draft.is_none() && state.drag.is_none() {
                data.remove::<NumberInputState>(id);
            } else {
                data.insert_temp(id, state);
            }
        });
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::DragValue,
                ui.is_enabled(),
                self.value.text(None),
            )
        });
        response.on_hover_ui(|ui| {
            ui.label("Drag left/right to adjust; Shift for fine adjustment; click or Tab to type; double-click to reset.");
            if !self.help.is_empty() {
                ui.label(self.help);
            }
        })
    }
}
