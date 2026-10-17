use serde_json::{json, Value};

/// Chooses value updates only after publishing the matching schema and host state.
#[derive(Default)]
pub(crate) struct PanelState {
    previous: Option<Value>,
}

impl PanelState {
    pub(crate) fn message(&mut self, state: Value, reset_view_state: bool) -> Value {
        let values_only = !reset_view_state
            && self
                .previous
                .as_ref()
                .is_some_and(|previous| same_host_state(previous, &state));
        let message = if values_only {
            let mut values = serde_json::Map::new();
            for bank in ["params", "inputControls"] {
                if let Some(params) = state.get(bank).and_then(Value::as_array) {
                    values.insert(
                        bank.to_owned(),
                        params
                            .iter()
                            .map(|param| json!({ "name": param["name"], "value": param["value"] }))
                            .collect(),
                    );
                }
            }
            json!({ "type": "parameterValues", "state": values })
        } else {
            let mut published = state.clone();
            if reset_view_state {
                published["viewState"] = json!({ "reset": true });
            }
            json!({ "type": "state", "state": published })
        };
        self.previous = Some(state);
        message
    }
}

fn same_host_state(previous: &Value, next: &Value) -> bool {
    let (Some(previous), Some(next)) = (previous.as_object(), next.as_object()) else {
        return false;
    };
    previous.len() == next.len()
        && previous.iter().all(|(key, value)| match key.as_str() {
            "params" | "inputControls" => next
                .get(key)
                .is_some_and(|next| same_parameter_schema(value, next)),
            _ => next.get(key) == Some(value),
        })
}

fn same_parameter_schema(previous: &Value, next: &Value) -> bool {
    let (Some(previous), Some(next)) = (previous.as_array(), next.as_array()) else {
        return false;
    };
    previous.len() == next.len()
        && previous.iter().zip(next).all(|(previous, next)| {
            let (Some(previous), Some(next)) = (previous.as_object(), next.as_object()) else {
                return false;
            };
            previous
                .iter()
                .filter(|(key, _)| key.as_str() != "value")
                .eq(next.iter().filter(|(key, _)| key.as_str() != "value"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Value {
        json!({
            "path": "sampler.onda", "running": true, "error": null,
            "buffers": [{ "name": "clip", "loadedPath": "clip.wav" }],
            "params": [{ "name": "gain", "type": "f32", "value": 0.5, "default": 1.0 }],
            "inputControls": [{ "name": "gate", "type": "bool", "value": false, "enabled": true }],
        })
    }

    #[test]
    fn edits_resets_and_acknowledgements_publish_only_values() {
        let mut panel = PanelState::default();
        let mut state = state();
        assert_eq!(panel.message(state.clone(), false)["type"], "state");
        for value in [json!(0.75), json!(1.0), json!(1.0)] {
            state["params"][0]["value"] = value.clone();
            state["inputControls"][0]["value"] = json!(true);
            assert_eq!(
                panel.message(state.clone(), false),
                json!({ "type": "parameterValues", "state": {
                    "params": [{ "name": "gain", "value": value }],
                    "inputControls": [{ "name": "gate", "value": true }],
                } }),
            );
        }
    }

    #[test]
    fn schema_host_and_view_changes_publish_full_state() {
        for (pointer, value) in [
            ("/params/0/default", json!(0.25)),
            ("/params/0/name", json!("level")),
            ("/inputControls/0/enabled", json!(false)),
            ("/buffers/0/loadedPath", json!(null)),
            ("/running", json!(false)),
            ("/error", json!("failed")),
            ("/path", json!("other.onda")),
            ("/params", json!([])),
        ] {
            let mut panel = PanelState::default();
            panel.message(state(), false);
            let mut changed = state();
            *changed.pointer_mut(pointer).unwrap() = value;
            assert_eq!(panel.message(changed.clone(), false)["state"], changed);
        }
        let mut panel = PanelState::default();
        panel.message(state(), false);
        let reset = panel.message(state(), true);
        assert_eq!(reset["type"], "state");
        assert_eq!(reset["state"]["viewState"], json!({ "reset": true }));
        assert_eq!(panel.message(state(), false)["type"], "parameterValues");
    }
}
