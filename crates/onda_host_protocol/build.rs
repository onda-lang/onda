use std::collections::HashSet;
use std::env;
use std::fmt::Write;
use std::fs;
use std::path::PathBuf;

use serde_json::Value;

fn main() {
    println!("cargo:rerun-if-changed=events.json");
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let source = fs::read_to_string(manifest_dir.join("events.json")).unwrap();
    let catalog: Value = serde_json::from_str(&source).unwrap();
    let events = catalog["events"]
        .as_array()
        .expect("events must be an array");
    let mut midi = Vec::new();
    let mut host_context = Vec::new();
    let mut names = HashSet::new();
    for event in events {
        let name = event["name"].as_str().expect("event name must be a string");
        assert!(names.insert(name), "duplicate host event '{name}'");
        let params = event["params"]
            .as_array()
            .expect("event params must be an array");
        for param in params {
            let pair = param.as_array().expect("event param must be a pair");
            assert_eq!(pair.len(), 2, "event param must contain a name and type");
            assert!(pair[0].is_string(), "param name must be a string");
            assert!(pair[1].is_string(), "param type must be a string");
        }
        match event["family"].as_str() {
            Some("midi") => midi.push(event),
            Some("host_context") => host_context.push(event),
            _ => panic!("host event '{name}' has an unknown family"),
        }
    }

    let mut generated = String::new();
    write_events(&mut generated, "PLUGIN_MIDI_EVENTS", "Midi", &midi);
    write_events(
        &mut generated,
        "PLUGIN_HOST_CONTEXT_EVENTS",
        "HostContext",
        &host_context,
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("events.rs");
    fs::write(output, generated).unwrap();
}

fn write_events(output: &mut String, constant: &str, family: &str, events: &[&Value]) {
    writeln!(
        output,
        "pub const {constant}: [HostEvent; {}] = [",
        events.len()
    )
    .unwrap();
    for event in events {
        let name = event["name"].as_str().expect("event name must be a string");
        let params = event["params"]
            .as_array()
            .expect("event params must be an array");
        let signature = params
            .iter()
            .map(|param| {
                let pair = param.as_array().expect("event param must be a pair");
                format!(
                    "{}: {}",
                    pair[0].as_str().expect("param name must be a string"),
                    pair[1].as_str().expect("param type must be a string")
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(output, "    HostEvent {{").unwrap();
        writeln!(output, "        name: {name:?},").unwrap();
        writeln!(output, "        signature: {signature:?},").unwrap();
        writeln!(output, "        family: HostEventFamily::{family},").unwrap();
        writeln!(output, "        params: &[").unwrap();
        for param in params {
            let pair = param.as_array().expect("event param must be a pair");
            let name = pair[0].as_str().expect("param name must be a string");
            let type_repr = pair[1].as_str().expect("param type must be a string");
            writeln!(
                output,
                "            HostEventParam {{ name: {name:?}, type_repr: {type_repr:?} }},"
            )
            .unwrap();
        }
        writeln!(output, "        ],").unwrap();
        writeln!(output, "    }},").unwrap();
    }
    writeln!(output, "];").unwrap();
}
