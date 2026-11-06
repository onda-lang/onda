use super::*;

fn source(routed: bool) -> String {
    let delegate = if routed {
        "  delegate fired(value: f32)\n"
    } else {
        ""
    };
    let publish = if routed { "    fired(held)\n" } else { "" };
    let handlers = if routed {
        "  when voices[0].fired(value):\n    seen = value + 1000.0\n  when voices[1].fired(value):\n    seen = value + 2000.0\n"
    } else {
        ""
    };
    format!(
        r#"
struct Message:
  value: i32 = 0
proc Voice:
  buffers:
    source: f32
{delegate}  init:
    held = 0.0
  event capture(message: Message, observed: i32):
    held = source[0] + f32(message.value) + f32(observed * 100)
{publish}  sample:
    out1 = held
proc Bank:
  buffers:
    left: f32
    right: f32
  init:
    voices: Voice[2] = [Voice(source = left), Voice(source = right)]
    messages: Message[2] = [Message(value = 3), Message(value = 5)]
    selector: i32 = 1
    message_selector: i32 = 0
    selections: i32 = 0
    seen = 0.0
  def choose() -> i32:
    selections = selections + 1
    return selector
{handlers}  sample:
    voices[choose()].capture(messages[message_selector], selections)
    out1 = voices[selector]() + seen * 10.0 + f32(selections * 10000)
    selector = 1 - selector
    message_selector = 1 - message_selector
buffers:
  left: f32
  right: f32
init:
  bank = Bank(left = left, right = right)
sample:
  out1 = bank()
"#
    )
}

fn assert_render(source: &str, expected: &[f32]) {
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let frames = expected.len();
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            frames,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: frames,
                fast_math: false,
                opt_level,
            },
        );
        let mut left = [2.0_f32];
        let mut right = [7.0_f32];
        for (name, samples) in [("left", &mut left), ("right", &mut right)] {
            let index = instance.buffer_index(name).unwrap();
            bind_buffer(
                &mut instance,
                index,
                samples.as_mut_ptr().cast(),
                1,
                1,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        let mut output = vec![0.0; frames];
        process_interleaved(&mut instance, &[], &mut output, frames).unwrap();
        assert_eq!(output, expected, "{opt_level:?}");
    }
}

#[test]
fn indexed_event_payloads_preserve_the_receiver_buffer_selection() {
    assert_render(&source(false), &[10110.0, 20207.0, 30310.0, 40407.0]);
}

#[test]
fn indexed_event_delegates_preserve_receiver_buffer_and_selector_order() {
    assert_render(&source(true), &[31210.0, 32277.0, 53410.0, 54477.0]);
}
