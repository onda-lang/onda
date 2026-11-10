use super::App;
use serde_json::Value;
impl App {
    pub(super) fn clipboard(&mut self) -> Result<&mut arboard::Clipboard, String> {
        if self.clipboard.is_none() {
            self.clipboard = Some(arboard::Clipboard::new().map_err(|e| e.to_string())?);
        }
        Ok(self.clipboard.as_mut().unwrap())
    }
    pub(super) fn commands(&mut self) {
        let commands = self.ui.take_commands();
        self.handle_commands(commands);
    }
    pub(super) fn handle_commands(&mut self, commands: Vec<Value>) {
        let mut publish = false;
        for command in commands {
            match self.command(&command) {
                Ok(changed) => publish |= changed,
                Err(error) => self.action_error(error),
            }
        }
        if publish {
            self.publish();
        }
    }
    /// Whether the command changed structural host state that needs publication.
    fn command(&mut self, v: &Value) -> Result<bool, String> {
        let name = v["name"].as_str().unwrap_or("");
        match v["type"].as_str().unwrap_or("") {
            "copyText" => {
                if let Some(text) = v["text"].as_str() {
                    self.clipboard()
                        .and_then(|clipboard| clipboard.set_text(text).map_err(|e| e.to_string()))
                        .map_err(|error| format!("Could not copy clipboard text: {error}"))?;
                }
                return Ok(false);
            }
            "viewState" => {
                if let Some(path) = v["state"]["path"].as_str() {
                    self.views.insert(path.into(), v["state"].clone());
                }
                return Ok(false);
            }
            // Layout belongs to the UI's saved per-source presentation.
            "setParamLayout" | "setInputControlLayout" => return Ok(false),
            "chooseOndaFile" => {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Onda", &["onda", "on", "ondaproject"])
                    .pick_file()
                {
                    self.open(path);
                }
                return Ok(false);
            }
            "unload" => {
                self.file_drop.cancel(&mut self.ui);
                if let Some(controller) = self.controller.take() {
                    self.options = controller.options().clone();
                }
                self.path = None;
            }
            "setRunSettings" => {
                let rate = v["sampleRateHz"]
                    .as_u64()
                    .filter(|n| (1..=768000).contains(n))
                    .ok_or("Invalid sample rate")? as u32;
                let block = v["blockSamples"]
                    .as_u64()
                    .filter(|n| (1..=65536).contains(n))
                    .ok_or("Invalid block size")? as usize;
                self.set_compile_settings(rate, block, self.options.fast_math)?;
            }
            "setFastMath" => {
                let enabled = v["enabled"].as_bool().ok_or("Invalid Fast Math setting")?;
                self.set_compile_settings(
                    self.options.sample_rate_hz,
                    self.options.block_samples,
                    enabled,
                )?;
            }
            "setTheme" => {
                let theme =
                    serde_json::from_value(v["mode"].clone()).map_err(|_| "Invalid theme")?;
                self.options.theme = Some(theme);
                if let Some(controller) = &mut self.controller {
                    controller.set_theme(theme);
                }
            }
            "setParamSmoothing" => {
                if let Some(ms) = v["milliseconds"]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0)
                {
                    if let Some(c) = &mut self.controller {
                        c.set_param_smoothing_seconds(ms / 1000.0)?;
                    }
                    self.options.param_smoothing_seconds = ms / 1000.0;
                    self.ui.message(
                        serde_json::json!({"type":"state","state":{"paramSmoothingMs":ms}}),
                    );
                }
                return Ok(false);
            }
            "setParam" => {
                if let Some(c) = &mut self.controller {
                    c.set_param(name, v["value"].clone());
                    self.ui
                        .message(serde_json::json!({"type":"parameterValues","state":{
                            "params":[{"name":name,"value":v["value"]}]
                        }}));
                    self.ui
                        .message(serde_json::json!({"type":"state","state":{"error":null}}));
                }
                return Ok(false);
            }
            "midiNote" => {
                if let Some(c) = &mut self.controller {
                    c.trigger_midi_note(
                        v["key"].as_i64().unwrap_or(0) as i32,
                        v["velocity"].as_f64().unwrap_or(0.0) as f32,
                        v["pressed"] == true,
                    );
                }
                return Ok(false);
            }
            kind => {
                if let Some(c) = &mut self.controller {
                    match kind {
                        "start" => c.start()?,
                        "stop" => {
                            c.stop();
                            self.publish_changes(onda_run::PollResult {
                                state_changed: true,
                                ..Default::default()
                            });
                            return Ok(false);
                        }
                        "resetParams" => {
                            c.reset_params();
                            let params: Vec<_> = c
                                .state()
                                .params
                                .iter()
                                .map(|p| serde_json::json!({"name":p["name"],"value":p["value"]}))
                                .collect();
                            self.ui.message(serde_json::json!({"type":"parameterValues","state":{"params":params}}));
                            self.ui.message(
                                serde_json::json!({"type":"state","state":{"error":null}}),
                            );
                            return Ok(false);
                        }
                        "resetEventArguments" => {
                            c.reset_event_arguments();
                            self.ui.message(serde_json::json!({"type":"state","state":{"events":c.state().events,"resetEventArguments":true,"error":null}}));
                            return Ok(false);
                        }
                        "clearLog" => {
                            c.clear_log();
                            self.ui.message(serde_json::json!({"type":"state","state":onda_run::log_panel_state(c.state())}));
                            return Ok(false);
                        }
                        "refreshDevices" => c.refresh_devices(),
                        "triggerEvent" => {
                            c.trigger_event(
                                name,
                                v["values"].as_array().cloned().unwrap_or_default(),
                            );
                            self.ui.message(serde_json::json!({"type":"state","state":{"error":c.state().error}}));
                            return Ok(false);
                        }
                        "setInputDevice" => c.set_input_device(v["name"].as_str())?,
                        "setOutputDevice" => c.set_output_device(v["name"].as_str())?,
                        "setMidiInputDevice" => c.set_midi_input_device(v["name"].as_str())?,
                        "clearBuffer" => c.clear_buffer(name),
                        "bindBufferFile" => {
                            if let Some(path) = v["filePath"].as_str() {
                                c.bind_buffer_file(name, path);
                            }
                        }
                        "chooseBufferFile" => {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("Onda buffer", &["wav", "ondabuffer"])
                                .pick_file()
                            {
                                c.bind_buffer_file(name, &path.to_string_lossy());
                            }
                        }
                        "saveProjectAs" => {
                            if let Some(path) = rfd::FileDialog::new()
                                .set_file_name("onda-project")
                                .save_file()
                            {
                                c.save_as_project(&path)?;
                            }
                        }
                        _ => return Ok(false),
                    }
                }
            }
        }
        Ok(true)
    }

    fn set_compile_settings(
        &mut self,
        rate: u32,
        block: usize,
        fast_math: bool,
    ) -> Result<(), String> {
        self.options.sample_rate_hz = rate;
        self.options.block_samples = block;
        self.options.fast_math = fast_math;
        if let Some(controller) = &mut self.controller {
            let result = controller.set_compile_settings(rate, block, fast_math);
            self.options = controller.options().clone();
            self.controller_woken = true;
            result?;
        }
        Ok(())
    }
}
