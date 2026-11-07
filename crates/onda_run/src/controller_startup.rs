//! Restore acknowledged controls and buffers before initializing the native run child.
use super::*;

impl PendingCommand {
    pub(super) fn restores_state(&self) -> bool {
        matches!(
            self,
            Self::RestoreControl
                | Self::BindBuffer { .. }
                | Self::ClearBuffer { .. }
                | Self::ResetParams
        )
    }
}

impl RunController {
    pub(super) fn send_restore_control(
        &mut self,
        command: &str,
        payload: &Value,
    ) -> Result<(), String> {
        let id = self
            .bridge
            .send_command(command, payload)
            .ok_or_else(|| "Run control connection failed during restoration".to_owned())?;
        self.pending_commands
            .insert(id, PendingCommand::RestoreControl);
        Ok(())
    }

    pub(super) fn begin_restoration(&mut self) -> Result<(), String> {
        self.restoring = true;
        self.send_restore_control(
            "setParamSmoothing",
            &json!({ "value": self.options.param_smoothing_seconds }),
        )?;
        for (name, value) in self.preserved_params.clone() {
            self.send_restore_control("setParam", &json!({ "name": name, "value": value }))?;
        }
        self.preserved_buffers.retain(|name, _| {
            self.state
                .buffers
                .iter()
                .any(|buffer| buffer["name"].as_str() == Some(name))
        });
        for (name, binding) in &self.preserved_buffers {
            let (command, payload, pending) = binding.replay_request(name);
            let id = self.bridge.send_command(command, &payload).ok_or_else(|| {
                "Run control connection failed during buffer restoration".to_owned()
            })?;
            self.pending_commands.insert(id, pending);
        }
        self.finish_restoration();
        Ok(())
    }

    /// Returns whether presentation can now publish the restored schema.
    pub(super) fn finish_restoration(&mut self) -> bool {
        if !self.restoring
            || self
                .pending_commands
                .values()
                .any(PendingCommand::restores_state)
        {
            return false;
        }
        self.restoring = false;
        if self.processing_requested {
            if let Err(error) = self.request_play() {
                self.fail_restoration(error);
            }
        } else {
            self.refresh_processing_state();
        }
        true
    }

    pub(super) fn request_play(&mut self) -> Result<(), String> {
        let id = self
            .bridge
            .send_command("play", &json!({}))
            .ok_or_else(|| "Run control connection failed before initialization".to_owned())?;
        self.pending_commands.insert(id, PendingCommand::Play);
        self.pending_play = Some(id);
        self.refresh_processing_state();
        Ok(())
    }

    pub(super) fn fail_restoration(&mut self, error: String) {
        self.preserve_pending_buffer_bindings();
        self.state.connected = false;
        self.stop();
        self.pending_commands.clear();
        self.state.error = Some(error);
        self.state.status = "Restoration failed".to_owned();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::{TcpListener, TcpStream};

    fn connected_controller() -> (RunController, TcpStream) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut controller = crate::tests::controller_without_child();
        controller
            .bridge
            .connect(
                listener.local_addr().unwrap().port(),
                1,
                controller.events_tx.clone(),
            )
            .unwrap();
        let (stream, _) = listener.accept().unwrap();
        controller.state.connected = true;
        (controller, stream)
    }

    #[test]
    fn restoration_waits_for_control_and_buffer_acknowledgements_before_play() {
        let (mut controller, stream) = connected_controller();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        controller.restoring = true;
        controller.state.connected = true;
        controller
            .pending_commands
            .insert(10, PendingCommand::RestoreControl);
        controller.pending_commands.insert(
            11,
            PendingCommand::BindBuffer {
                name: "clip".into(),
                path: "clip.wav".into(),
            },
        );
        controller.handle_tcp_response(r#"{"id":10,"ok":true}"#);
        assert!(controller.restoring);
        assert!(!controller
            .pending_commands
            .values()
            .any(|command| matches!(command, PendingCommand::Play)));
        let panel = host_panel_state(Some(&controller), None, &controller.options, "dark", 0.03);
        assert_eq!(panel["schemaReady"], false);
        controller.handle_tcp_response(r#"{"id":11,"ok":true}"#);
        assert!(!controller.restoring);
        assert!(!controller.state.running);
        let mut stream = BufReader::new(stream);
        // Buffer metadata refresh precedes play; its reply is not a restore operation.
        let mut request = String::new();
        stream.read_line(&mut request).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&request).unwrap()["command"],
            "getBuffers"
        );
        request.clear();
        stream.read_line(&mut request).unwrap();
        let request: Value = serde_json::from_str(&request).unwrap();
        assert_eq!(request["command"], "play");
        controller.processing_requested = false;
        controller.handle_tcp_response(&json!({"id":request["id"],"ok":true}).to_string());
        assert!(
            !controller.state.running,
            "a late play reply must respect Stop"
        );
    }

    #[test]
    fn rejected_restoration_terminates_preparation_and_requires_a_fresh_start() {
        let mut controller = crate::tests::controller_without_child();
        controller.restoring = true;
        controller.state.connected = true;
        controller
            .pending_commands
            .insert(10, PendingCommand::RestoreControl);
        controller.pending_commands.insert(
            11,
            PendingCommand::BindBuffer {
                name: "clip".into(),
                path: "clip.wav".into(),
            },
        );
        controller.handle_tcp_response(r#"{"id":10,"ok":false,"error":"invalid control"}"#);
        assert!(!controller.restoring);
        assert!(!controller.processing_requested);
        assert!(!controller.state.running && !controller.state.connected);
        assert!(controller.pending_commands.is_empty());
        assert_eq!(controller.state.error.as_deref(), Some("invalid control"));
        assert!(matches!(
            controller.source_compilation,
            SourceCompilationState::None
        ));
        assert_eq!(
            controller.preserved_buffers["clip"],
            PreservedBufferBinding::File("clip.wav".into())
        );
    }

    #[test]
    fn latest_play_result_owns_transport_intent_and_errors() {
        for latest_succeeds in [false, true] {
            for latest_replies_first in [false, true] {
                let (mut controller, _stream) = connected_controller();
                controller.request_play().unwrap();
                let older = *controller.pending_commands.keys().max().unwrap();
                controller.set_param("divisor", json!(1));
                controller.request_play().unwrap();
                let latest = *controller.pending_commands.keys().max().unwrap();
                let older_reply = json!({
                    "id": older, "ok": !latest_succeeds, "error": "older failure",
                });
                let latest_reply = json!({
                    "id": latest, "ok": latest_succeeds, "error": "latest failure",
                });
                let replies = if latest_replies_first {
                    [latest_reply, older_reply]
                } else {
                    [older_reply, latest_reply]
                };
                for reply in replies {
                    controller.handle_tcp_response(&reply.to_string());
                }
                assert_eq!(controller.processing_requested, latest_succeeds);
                assert_eq!(controller.state.running, latest_succeeds);
                assert_eq!(controller.scope_polling_active, latest_succeeds);
                assert_eq!(
                    controller.state.error.as_deref(),
                    (!latest_succeeds).then_some("latest failure"),
                );
            }
        }
    }

    #[test]
    fn stop_ignores_a_pending_play_failure() {
        let (mut controller, _stream) = connected_controller();
        controller.request_play().unwrap();
        let id = *controller.pending_commands.keys().max().unwrap();
        controller.stop();
        controller.handle_tcp_response(
            &json!({"id":id,"ok":false,"error":"initialization failed"}).to_string(),
        );
        assert!(!controller.processing_requested && !controller.state.running);
        assert_eq!(controller.state.status, "Stopped");
        assert!(controller.state.error.is_none());
    }
}
