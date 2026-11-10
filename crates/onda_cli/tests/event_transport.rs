use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

#[test]
fn daemon_stdio_preserves_negative_zero_in_metadata_events_and_delegates() {
    let root = std::env::temp_dir().join(format!(
        "onda-event-transport-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    fs::create_dir(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let source = root.join("main.onda");
    fs::write(
        &source,
        r#"struct Payload:
  singles: f32[2]
  wide: (f64, i64)
delegate scalar(a: f32, b: f64, id: i64)
delegate nested(payload: Payload)
event scalars(a: f32 = -0.0, b: f64 = -0.0, id: i64 = 0):
  scalar(a, b, id)
event payloads(values: Payload[]):
  if values.len() > 0:
    nested(values[0])
sample:
  out1 = 0.0
"#,
    )
    .unwrap();
    let payload = json!({"singles":["-0",0.0],"wide":["-0","-9223372036854775808"]});
    let requests = [
        json!({"id":1,"command":"run_metadata","path":source}),
        json!({"id":2,"command":"run_start","path":source}),
        json!({"id":3,"command":"run_trigger_event","path":source,"name":"scalars",
            "values":["-0","-0","9223372036854775807"]}),
        json!({"id":4,"command":"run_trigger_event","path":source,"name":"scalars",
            "values":[0.0,0.0,"0"]}),
        json!({"id":5,"command":"run_trigger_event","path":source,"name":"payloads",
            "values":[[payload]]}),
    ];
    let input = requests
        .iter()
        .map(|request| format!("{request}\n"))
        .collect::<String>();
    let mut child = Command::new(env!("CARGO_BIN_EXE_onda"))
        .args(["daemon", "stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    fs::remove_dir_all(root).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replies = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(replies.len(), requests.len());
    for (index, reply) in replies.iter().enumerate() {
        assert_eq!(reply["id"], index + 1);
        assert_eq!(reply["ok"], true, "{reply}");
    }
    let args = &replies[0]["result"]["events"][0]["args"];
    assert_eq!(args[0]["default"], "-0");
    assert_eq!(args[1]["default"], "-0");
    assert_eq!(
        replies[2]["result"]["delegate_occurrences"][0]["values"],
        json!({"a":"-0","b":"-0","id":"9223372036854775807"})
    );
    let positive = &replies[3]["result"]["delegate_occurrences"][0]["values"];
    for name in ["a", "b"] {
        let value = positive[name].as_f64().unwrap();
        assert_eq!(value.to_bits(), 0.0_f64.to_bits());
    }
    assert_eq!(positive["id"], "0");
    assert_eq!(
        replies[4]["result"]["delegate_occurrences"][0]["values"]["payload"],
        payload
    );
}
