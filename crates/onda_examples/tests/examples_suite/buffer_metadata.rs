use super::*;

#[test]
fn buffer_counts_distinguish_elements_samples_and_collection_entries() {
    let source = r#"
outs 11
buffers:
  bus: f32[]
  bank: f32[2] {2}

def inspect(buf: buffer<f32[]>):
  alias = buf
  return alias.len() * 100 + alias.samples() * 10 + alias.chans()

proc Inspector:
  buffers:
    clips: f32[2] {2}
  block:
    selected = clips[0]
    total = selected.len()
    count = selected.samples()
    sample:
      out1 = f32(total * 100 + count * 10 + selected.chans())

init:
  inspector = Inspector(clips = bank)
  captured = 0

events:
  capture():
    captured = bus.len() * 100 + bus.samples() * 10 + bus.chans()

sample:
  selected = bus
  channel = bus[0, :]
  out1 = f32(bus.len())
  out2 = f32(selected.samples())
  out3 = f32(inspect(selected))
  out4 = f32(bank.len())
  out5 = f32(bank[0].len())
  out6 = f32(bank[0].samples())
  out7 = f32(bank[1].len())
  out8 = f32(bank[1].samples())
  out9 = f32(channel.len())
  out10 = inspector()
  out11 = f32(captured)
"#;
    let block_size = 4;
    let (mut instance, inputs, outputs) = compile_instance(source, block_size);
    assert_eq!((inputs, outputs), (0, 11));
    let mut bank = vec![0.0_f32; 6];
    bind_buffer(
        &mut instance,
        1,
        bank.as_mut_ptr().cast(),
        3,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .unwrap();
    // Neutral unbound buffers retain one sample per declared channel.
    for (samples, channels) in [(1, 1), (3, 2), (4, 3)] {
        let mut data = vec![0.0_f32; samples * channels];
        if channels != 1 {
            bind_buffer(
                &mut instance,
                0,
                data.as_mut_ptr().cast(),
                samples,
                channels,
                48_000.0,
                PrimitiveType::F32,
            )
            .unwrap();
        }
        trigger_event_by_index_checked(
            &mut instance,
            0,
            &[],
            onda_runtime::ExecutionOutput::none(),
        )
        .unwrap();
        let mut output = vec![0.0_f32; block_size * outputs];
        process_interleaved(&mut instance, &[], &mut output, block_size).unwrap();
        let elements = samples * channels;
        let encoded = (elements * 100 + samples * 10 + channels) as f32;
        let expected = [
            elements as f32,
            samples as f32,
            encoded,
            2.0,
            6.0,
            3.0,
            2.0,
            1.0,
            samples as f32,
            632.0,
            encoded,
        ];
        for block_sample in output.chunks_exact(outputs) {
            assert_eq!(block_sample, expected);
        }
    }
}

#[test]
fn wrapping_interpolation_uses_channel_lengths_for_buffers_and_arrays() {
    let source = r#"
import std/lookup
import std/data
outs 8
buffers:
  bus: f32[2]
  mono: f32
init:
  values: f32[3] = [1.0, 2.0, 3.0]
  data: std::data<3, 2>::Data<f32>
sample:
  out1 = mono.readLW(2.5)
  out2 = bus.readLW(1, 2.5)
  out3 = mono.readCW(2.5)
  out4 = bus.readCW(1, 2.5)
  out5 = values.readLW(2.5)
  out6 = values.readCW(2.5)
  out7 = f32(data.len())
  out8 = f32(data.samples())
"#;
    let (mut instance, _, outputs) = compile_instance(source, 4);
    let mut data = vec![1.0_f32, 10.0, 2.0, 20.0, 3.0, 30.0];
    bind_buffer(
        &mut instance,
        0,
        data.as_mut_ptr().cast(),
        3,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .unwrap();
    let mut mono = vec![1.0_f32, 2.0, 3.0];
    bind_buffer(
        &mut instance,
        1,
        mono.as_mut_ptr().cast(),
        3,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .unwrap();
    let mut output = vec![0.0; 4 * outputs];
    process_interleaved(&mut instance, &[], &mut output, 4).unwrap();
    for sample in output.chunks_exact(outputs) {
        assert_eq!(sample, [2.0, 20.0, 2.0, 20.0, 2.0, 2.0, 6.0, 3.0]);
    }
}
