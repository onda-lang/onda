//! Timed UI work is independent of the host's event loop and GPU presentation.
use crate::RunUi;
use web_time::Instant;

impl RunUi {
    /// Advance timed feedback and resolve layout without drawing.
    /// Drain commands afterward, then compare revisions to decide whether to draw.
    pub fn update(&mut self) {
        self.update_at(Instant::now());
    }

    pub(crate) fn update_at(&mut self, now: Instant) {
        self.prepare_scene_at(now);
        if self.advance_hover(now) {
            self.invalidate();
            self.prepare_scene_at(now);
        }
    }

    /// Next timed update, or `None` when the host may sleep until external input.
    /// Recompute after updating, draining commands, or changing state/input. A wakeup
    /// can produce commands without changing pixels, so it does not imply a redraw.
    pub fn wake_deadline(&self) -> Option<Instant> {
        self.repaint_deadline()
            .into_iter()
            .chain(self.view_state_deadline())
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Input;
    use serde_json::json;
    use web_time::Duration;

    #[test]
    fn one_deadline_covers_feedback_and_persistence_and_returns_to_idle() {
        let mut ui = RunUi::default();
        ui.resize(720.0, 840.0, 1.0);
        ui.message(json!({"type":"state","state":{
            "path":"schedule.onda","supportsScope":false,"supportsViewState":true,
            "params":(0..60).map(|i| json!({"name":format!("p{i}"),"type":"f64","value":0})).collect::<Vec<_>>()
        }}));
        ui.update();
        ui.take_commands();
        assert_eq!(ui.wake_deadline(), None);

        ui.input(Input::PointerMove {
            x: 100.0,
            y: 35.0,
            fine: false,
        });
        ui.update();
        let tooltip = ui.wake_deadline().expect("hover needs one timed update");
        ui.input(Input::Scroll { delta: 20.0 });
        ui.update();
        ui.take_commands();
        let save = ui.view_state_deadline().unwrap();
        assert_eq!(ui.wake_deadline(), Some(save.min(tooltip)));

        // The scroll save becomes due without input or a GPU frame.
        std::thread::sleep(
            save.saturating_duration_since(Instant::now()) + Duration::from_millis(1),
        );
        let revision = ui.revisions();
        ui.update();
        let commands = ui.take_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0]["type"], "viewState");
        assert_eq!(commands[0]["state"]["scrollTop"], 20.0);
        assert_eq!(ui.revisions(), revision, "saving does not repaint");

        ui.input(Input::FocusLost);
        ui.update();
        ui.take_commands();
        assert_eq!(ui.wake_deadline(), None);
    }
}
