use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new(source: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "onda-typed-transport-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        fs::write(root.join("main.onda"), source).unwrap();
        Self(root)
    }

    fn daemon(&self, requests: &[Value]) -> Vec<Value> {
        let mut child = Command::new(env!("CARGO_BIN_EXE_onda"))
            .args(["daemon", "stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        for (id, request) in requests.iter().enumerate() {
            let mut request = request.clone();
            request["id"] = json!(id);
            request["path"] = json!(self.0.join("main.onda"));
            writeln!(input, "{request}").unwrap();
        }
        drop(input);
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let replies = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect::<Vec<Value>>();
        assert_eq!(replies.len(), requests.len());
        replies
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn daemon_metadata_and_rendering_accept_every_primitive_audio_type() {
    let fixture = Fixture::new(
        r#"
proc Main:
  ins:
    double_input: f64
    bool_input: bool
    integer_input: i32[2]
    wide_input: i64
  outs:
    single: f32
    double: f64[2]
    integer: i32
    wide: i64
    enabled: bool
  sample:
    single = 0.25
    double[0] = 0.5 + double_input
    double[1] = -0.5
    integer = 1065353216 + integer_input[0]
    wide = 7 + wide_input
    enabled = !bool_input
"#,
    );
    let replies = fixture.daemon(&[
        json!({"command":"run_metadata"}),
        json!({"command":"run_start"}),
        json!({"command":"run_render"}),
    ]);
    for reply in &replies {
        assert_eq!(reply["ok"], true, "{reply}");
    }
    assert_eq!(replies[0]["result"]["outputChannels"], 6);
    let channels = replies[2]["result"]["channels"].as_array().unwrap();
    for (channel, expected) in channels
        .iter()
        .zip([0.25, 0.5, -0.5, 1_065_353_216.0, 7.0, 1.0])
    {
        assert!(channel
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value.as_f64() == Some(expected)));
    }
}

#[test]
fn events_preserve_recursive_types_and_reject_mismatches_before_execution() {
    let fixture = Fixture::new(
        r#"
enum Mode:
  First
  Second
struct Payload:
  enabled: bool
  precise: f64
  wide: i64
  pair: (i32, f32)
delegate echoed(single: f32, double: f64, integer: i32, wide: i64, enabled: bool, fixed: i32[2], payloads: Payload[], mode: Mode)
delegate counted(value: i32)
init:
  calls = 0
event echo(single: f32, double: f64, integer: i32, wide: i64, enabled: bool, fixed: i32[2], payloads: Payload[], mode: Mode):
  calls += 1
  echoed(single, double, integer, wide, enabled, fixed, payloads, mode)
event count():
  counted(calls)
sample:
  out1 = 0.0
"#,
    );
    let payload = json!({"enabled":false,"precise":1.0000000000000002,"wide":"-9223372036854775808","pair":[-7.0,"-0"]});
    let values = json!([
        0.15,
        1.0000000000000002,
        -2147483648i64,
        "9223372036854775807",
        true,
        [7, -9],
        [payload],
        999
    ]);
    let mut requests = vec![
        json!({"command":"run_start"}),
        json!({"command":"run_trigger_event","name":"echo","values":values}),
    ];
    for (index, invalid) in [
        (0, json!("1")),
        (1, json!(false)),
        (2, json!(1.5)),
        (2, json!(2147483648i64)),
        (3, json!(9007199254740992u64)),
        (3, json!("9223372036854775808")),
        (4, json!(1)),
        (5, json!([1])),
        (
            6,
            json!([{"enabled":0,"precise":1,"wide":"1","pair":[1,0]}]),
        ),
    ] {
        let mut invalid_values = values.clone();
        invalid_values[index] = invalid;
        requests.push(json!({"command":"run_trigger_event","name":"echo","values":invalid_values}));
    }
    let mut empty = values.clone();
    empty[6] = json!([]);
    requests.push(json!({"command":"run_trigger_event","name":"echo","values":empty}));
    requests.push(json!({"command":"run_trigger_event","name":"count","values":[]}));
    let replies = fixture.daemon(&requests);
    assert_eq!(replies[0]["ok"], true, "{}", replies[0]);
    assert_eq!(replies[1]["ok"], true, "{}", replies[1]);
    let echoed = &replies[1]["result"]["delegate_occurrences"][0]["values"];
    assert_eq!(
        echoed,
        &json!({"single":0.15,"double":1.0000000000000002,"integer":-2147483648.0,
        "wide":"9223372036854775807","enabled":true,"fixed":[7.0,-9.0],"payloads":[payload],"mode":1.0})
    );
    for reply in &replies[2..replies.len() - 2] {
        assert_eq!(reply["ok"], false, "{reply}");
        if let Some(occurrences) = reply["result"]["delegate_occurrences"].as_array() {
            assert!(occurrences.is_empty());
        }
    }
    let empty = &replies[replies.len() - 2];
    assert_eq!(empty["ok"], true, "{empty}");
    assert_eq!(
        empty["result"]["delegate_occurrences"][0]["values"]["payloads"],
        json!([])
    );
    let count = replies.last().unwrap();
    assert_eq!(count["ok"], true, "{count}");
    assert_eq!(
        count["result"]["delegate_occurrences"][0]["values"]["value"].as_f64(),
        Some(2.0)
    );
}
