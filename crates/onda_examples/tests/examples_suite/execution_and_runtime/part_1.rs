use super::*;

#[test]
fn unbound_buffer_arrays_use_neutral_descriptors_and_discard_writes() {
    let source = r#"
buffers:
  bank: f32[2] {3}

outs:
  out1

sample:
  bank[1][1, 99] = 42.0
  out1 = bank[1][1, 0] + f32(bank.len()) + f32(bank[1].len()) + f32(bank[1].chans()) + bank[1].samplerate() / 48000.0
"#;
    let (mut instance, in_channels, out_channels) = compile_instance(source, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 3);
    assert_eq!(instance.buffer_array_count(), 1);
    let group = instance.buffer_array(0).expect("buffer array metadata");
    assert_eq!(group.name(), "bank");
    assert_eq!(group.first(), 0);
    assert_eq!(group.len(), 3);

    let mut output = [0.0_f32];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process unbound bank");
    assert_near(output[0], 7.0, 1e-6);
}

#[test]
fn ranged_integer_params_normalize_raw_host_values_at_process_entry() {
    let source = r#"
params:
  index: i32 = 0 {min = 0, max = 3}

outs:
  out1

sample:
  out1 = f32(index)
"#;
    let (mut instance, in_channels, out_channels) = compile_instance(source, 1);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32];
    for (raw, expected) in [(100_i32, 3.0_f32), (-100_i32, 0.0_f32)] {
        set_param_by_index(&mut instance, 0, &raw.to_ne_bytes()).expect("set raw i32 param");
        process_interleaved(&mut instance, &[], &mut output, 1).expect("process ranged param");
        assert_eq!(output, [expected]);
    }
}

#[test]
fn ranged_struct_fields_normalize_construction_and_method_assignments() {
    let source = r#"
struct Cursor:
  index: i32 = 0 {4, wrap}

  def advance(self):
    self.index += 5

init:
  cursor = Cursor(index = 6)

sample:
  out1 = f32(cursor.index)
  cursor.advance()
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 4];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process ranged struct field");
    assert_eq!(output, [2.0, 3.0, 0.0, 1.0]);
}

#[test]
fn ranged_fields_of_struct_arrays_normalize_construction_and_assignment() {
    let source = r#"
struct Cursor:
  index: i32 = 0 {4, wrap}

init:
  cursors: Cursor[1] = [Cursor(index = 6)]

sample:
  out1 = f32(cursors[0].index)
  cursors[0].index = cursors[0].index + 5
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 4];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process ranged struct-array field");
    assert_eq!(output, [2.0, 3.0, 0.0, 1.0]);
}

#[test]
fn mixed_width_stdlib_clamp_and_lerp_preserve_f64_distinctions() {
    let frames = 4;
    let src = r#"
import std/math

outs:
  out1
  out2

sample:
  x: f32 = f32(16777216.0)
  lo: f64 = f64(16777217.0)
  hi: f64 = f64(16777218.0)
  out1 = f32(clamp(x, lo, hi) - lo)
  out2 = f32(lerp(x, hi, f64(0.5)) - lo)
"#;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for frame in 0..frames {
        assert_near(output[frame * out_channels], 0.0, 1e-6);
        assert_near(output[frame * out_channels + 1], 0.0, 1e-6);
    }
}

#[test]
fn named_slice_arguments_evaluate_bounds_in_textual_source_order() {
    let frames = 4;
    let src = r#"
outs:
  out1

def mark(values: f32[], value: i32) -> i32:
  values[0] = f32(value)
  return 0

def consume(a: f32[], b: f32[]) -> f32:
  return 0.0

init:
  values: f32[1] = [0.0]

sample:
  out1 = consume(
    b = values[mark(values, 2):],
    a = values[mark(values, 1):]
  ) + values[0]
"#;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, 1.0, 1e-6);
    }
}

const STDLIB_ENV_AR_ONE_SHOT_EXAMPLE: &str = r#"
import std/env

outs { out1 }

init {
  trig = 0.0
  env = std::env::AR(attack_s = 0.00025, release_s = 0.0005, trigger = trig)
}

events {
  bang() {
    trig = 1.0
  }
}

sample {
  env.trigger = trig
  trig = 0.0
  out1 = env()
}
"#;

const STDLIB_ENV_ASR_F64_EXAMPLE: &str = r#"
import std/env

outs { out1 }

init {
  env = std::env::ASR<f64>(
    attack_s = f64(0.00025),
    sustain = f64(0.5),
    release_s = f64(0.00025),
    gate = f64(1.0)
  )
}

sample {
  out1 = f32(env())
}
"#;

const STDLIB_OSC_SQUARE_F64_EXAMPLE: &str = r#"
import std/osc

outs { out1 }

init {
  osc = std::osc::Square<f64>(freq = f64(220.0), amp = f64(0.25))
}

sample {
  out1 = f32(osc())
}
"#;

const STDLIB_OSC_KSINE_EXAMPLE: &str = r#"
import std/osc

outs { out1 }

init {
  lfo = std::osc::KSine<f64>(
    freq = f64(SR) / f64(BS * 4),
    amp = f64(0.25)
  )
}

block {
  held = f32(lfo())

  sample {
    out1 = held
  }
}
"#;

const STDLIB_OSC_SAW_AMP_EXAMPLE: &str = r#"
import std/osc

outs { out1 }

init {
  osc = std::osc::Saw(freq = 220.0, amp = 0.25)
}

sample {
  out1 = osc()
}
"#;

const STDLIB_OSC_PHASOR_DYNAMIC_PARAM_EXAMPLE: &str = r#"
import std/osc

outs { out1 }

init {
  phasor = std::osc::Phasor(freq = 0.0)
  idx: i32 = 0
}

sample {
  if (idx == 0) {
    out1 = phasor(freq = 0.0)
  } else {
    out1 = phasor(freq = SR * 0.25)
  }
  idx = idx + 1
}
"#;

const PROC_BIND_HOOK_INIT_USES_TOP_LEVEL_OVERSAMPLE_RATE_EXAMPLE: &str = r#"
proc Voice {
  params {
    freq = 48000.0 => update
  }
  init {
    cached = 0.0
  }
  def update() {
    cached = freq / SR
  }
  outs { out1 }
  sample {
    out1 = cached
  }
}

outs { out1 }

init {
  voice = Voice()
}

sample 2 {
  out1 = voice()
}
"#;

const PROC_BIND_HOOK_INIT_THROUGH_DEF_PROC_ARRAY_USES_OVERSAMPLE_RATE_EXAMPLE: &str = r#"
proc Voice {
  params {
    freq = 48000.0 => update
  }
  init {
    cached = 0.0
  }
  def update() {
    cached = freq / SR
  }
  outs { out1 }
  sample {
    out1 = cached
  }
}

outs { out1 }

def run(voices) {
  return voices[0]()
}

init {
  voices: Voice[1] = Voice()
}

sample 2 {
  out1 = run(voices)
}
"#;

const EXPLICIT_OVERSAMPLED_PROC_FROM_OVERSAMPLED_CONTEXT_REJECTED_EXAMPLE: &str = r#"
proc Child {
  outs { out1 }
  sample 2 {
    out1 = 0.0
  }
}

proc Parent {
  init {
    child = Child()
  }
  outs { out1 }
  sample {
    out1 = child()
  }
}

outs { out1 }

init {
  parent = Parent()
}

sample 4 {
  out1 = parent()
}
"#;

const STDLIB_OSC_TRIANGLE_F64_EXAMPLE: &str = r#"
import std/osc

outs { out1 }

init {
  osc = std::osc::Triangle<f64>(freq = f64(220.0), amp = f64(0.25))
}

sample {
  out1 = f32(osc())
}
"#;

const STDLIB_FILTER_ONE_POLE_MODES_EXAMPLE: &str = r#"
import std/filter

outs 2

init {
  lp = std::filter::OnePole(cutoff = 400.0, mode = std::filter::mode::ONE_POLE_LOWPASS)
  hp = std::filter::OnePole(cutoff = 400.0, mode = std::filter::mode::ONE_POLE_HIGHPASS)
}

sample {
  out1 = lp(1.0)
  out2 = hp(1.0)
}
"#;

const STDLIB_FILTER_DCBLOCK_EXAMPLE: &str = r#"
import std/filter

outs { out1 }

init {
  dc = std::filter::DCBlock()
}

sample {
  out1 = dc(1.0)
}
"#;

const STDLIB_NOISE_FAMILY_EXAMPLE: &str = r#"
import std/noise

outs 3

init {
  white = std::noise::White(amp = 0.25)
  pink = std::noise::Pink(amp = 0.25)
  brown = std::noise::Brown(amp = 0.25)
}

sample {
  out1 = white()
  out2 = pink()
  out3 = brown()
}
"#;

const STDLIB_NOISE_WHITE_F64_EXAMPLE: &str = r#"
import std/noise

outs { out1 }

init {
  noise = std::noise::White<f64>(amp = f64(0.25))
}

sample {
  out1 = f32(noise())
}
"#;

const STDLIB_LEVELS_HELPERS_EXAMPLE: &str = r#"
import std/levels

outs 8

sample {
  (lin_l, lin_r) = std::levels::pan_linear(0.0)
  (pow_l, pow_r) = std::levels::pan_3db(0.0)

  out1 = std::levels::db_to_gain(-6.0)
  out2 = std::levels::gain_to_db(0.5)
  out3 = lin_l
  out4 = lin_r
  out5 = pow_l
  out6 = pow_r
  out7 = f32(std::levels::db_to_gain(f64(-6.0)))
  out8 = f32(std::levels::gain_to_db(f64(0.5)))
}
"#;

const STDLIB_FILTER_SVF_EXTRA_MODES_EXAMPLE: &str = r#"
import std/filter
import std/osc

outs 3

init {
  src = std::osc::Saw(freq = 220.0, amp = 0.25)
  notch = std::filter::Svf(cutoff = 1200.0, q = 0.8, mode = std::filter::mode::SVF_NOTCH)
  peak = std::filter::Svf(cutoff = 1200.0, q = 0.8, mode = std::filter::mode::SVF_PEAK)
  allpass = std::filter::Svf(cutoff = 1200.0, q = 0.8, mode = std::filter::mode::SVF_ALLPASS)
}

sample {
  x = src()
  out1 = notch(x)
  out2 = peak(x)
  out3 = allpass(x)
}
"#;

const STDLIB_SMOOTHING_LAG_EXAMPLE: &str = r#"
import std/smoothing

outs { out1 }

init {
  smooth = std::smoothing::Lag(time_s = 0.001)
}

sample {
  out1 = smooth(1.0)
}
"#;

const STDLIB_SMOOTHING_SLEW_F64_EXAMPLE: &str = r#"
import std/smoothing

outs { out1 }

init {
  smooth = std::smoothing::Slew<f64>(rise_per_s = f64(4800.0), fall_per_s = f64(2400.0))
}

sample {
  out1 = f32(smooth(f64(1.0)))
}
"#;

const STDLIB_MIX_HELPERS_EXAMPLE: &str = r#"
import std/mix

outs 5

init {
  mono = std::mix::MonoToStereo()
  sum = std::mix::StereoToMono()
  blend = std::mix::ConstantSum(gain1 = 0.25, gain2 = 0.5)
  xfade = std::mix::Crossfade(mix = 0.25)
}

sample {
  mono(0.5)
  out1 = mono.out1
  out2 = mono.out2
  out3 = sum(0.5, -0.5)
  out4 = blend(1.0, 2.0)
  out5 = xfade(1.0, -1.0)
}
"#;

const STDLIB_MIX_GENERIC_CHANNELS_EXAMPLE: &str = r#"
import std/mix

outs 7

init {
  spread = std::mix::chans<4>::Broadcast()
  avg = std::mix::chans<4>::Average()
  xfade = std::mix::chans<2>::Crossfade(mix = 0.25)
}

sample {
  spread(0.25)
  xfade([1.0, -1.0], [0.0, 1.0])

  out1 = spread.out1
  out2 = spread.out2
  out3 = spread.out3
  out4 = spread.out4
  out5 = avg(1.0, 2.0, 3.0, 4.0)
  out6 = xfade.out1
  out7 = xfade.out2
}
"#;

const STDLIB_GAIN_LINEAR_DB_EXAMPLE: &str = r#"
import std/gain

outs 3

init {
  lin = std::gain::Constant(gain = 0.25)
  db = std::gain::Db(db = -6.0)
}

sample {
  out1 = lin(1.0)
  out2 = db(1.0)
  out3 = db(-1.0)
}
"#;

const STDLIB_GAIN_SMOOTHED_DB_F64_EXAMPLE: &str = r#"
import std/gain

outs { out1 }

init {
  gain = std::gain::SmoothedDb<f64>(db = f64(0.0), time_s = f64(0.001))
}

sample {
  out1 = f32(gain(f64(1.0)))
}
"#;

const STDLIB_PITCH_HELPERS_EXAMPLE: &str = r#"
import std/pitch

outs 6

sample {
  out1 = std::pitch::note_to_hz(69.0)
  out2 = std::pitch::hz_to_note(440.0)
  out3 = std::pitch::ratio_between(69.0, 81.0)
  out4 = f32(std::pitch::note_to_hz(f64(69.0)))
  out5 = f32(std::pitch::hz_to_note(f64(440.0)))
  out6 = f32(std::pitch::ratio_between(f64(69.0), f64(81.0)))
}
"#;

#[test]

fn wasm_playground_default_runs_with_bounded_output() {
    let frames = 128;

    let (mut instance, in_channels, out_channels) =
        compile_instance(WASM_PLAYGROUND_FILE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|sample| sample.abs() > 0.05),
        "expected playground default to produce audible output, got {output:?}"
    );

    assert!(
        output.iter().all(|sample| sample.abs() <= 1.1),
        "expected playground default to remain bounded, got {output:?}"
    );
}

#[test]

fn graph_nodes_remain_addressable_from_top_level_events() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_EVENT_ROUTING_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|sample| (*sample).abs() <= 1e-6));

    let idx = instance
        .event_index("set_gain")
        .expect("top-level graph event must exist");

    trigger_event_by_index(
        &mut instance,
        idx,
        &0.75_f32.to_ne_bytes(),
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("event trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|sample| (*sample - 0.75).abs() <= 1e-6));
}

#[test]

fn stdlib_env_ar_runs_as_a_one_shot_envelope() {
    let frames = 64;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_ENV_AR_ONE_SHOT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let bang_idx = instance.event_index("bang").expect("bang event must exist");

    assert_eq!(instance.event_payload_bytes(bang_idx), Some(0));

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|sample| sample.abs() <= 1e-6));

    trigger_event_by_index(
        &mut instance,
        bang_idx,
        &[],
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("bang trigger should succeed");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let peak = output
        .iter()
        .fold(0.0_f32, |acc, sample| acc.max(sample.abs()));

    assert!(
        peak >= 0.9,
        "expected AR envelope peak near 1.0, got {peak}"
    );

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().all(|sample| sample.abs() <= 1e-3),
        "expected AR envelope to finish and return to zero, got {output:?}"
    );
}

#[test]

fn stdlib_env_asr_supports_f64_and_holds_sustain() {
    let frames = 64;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_ENV_ASR_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|sample| *sample > 0.1),
        "expected ASR envelope to rise above zero, got {output:?}"
    );

    let tail = &output[frames - 8..];

    for sample in tail {
        assert_near(*sample, 0.5, 0.05);
    }
}

#[test]
fn stdlib_decay_env_publishes_finished_once_per_start() {
    let source = r#"
import std/env

init:
  env = std::env::DecayEnv(decay_s = 1.0 / SR, end_level = 0.5)
  completions: i32 = 0

event start():
  env.start()

when env.finished():
  completions += 1

sample:
  out1 = env() + f32(completions)
"#;
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let start = instance.event_index("start").expect("start event");
    trigger_event_by_index(
        &mut instance,
        start,
        &[],
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("start should succeed");

    let mut output = [0.0_f32; 1];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process completion");
    assert_near(output[0], 1.0, 1e-6);

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process idle envelope");
    assert_near(output[0], 1.0, 1e-6);
}

#[test]
fn stdlib_delay_lines_use_wrapped_cursors_and_zero_delay_is_direct() {
    let source = r#"
import std/delay

init:
  frame = 0 {8, wrap}
  direct = std::delay<8>::Linear(delay_samples = 0.0)
  delayed = std::delay<8>::Integer(delay_samples = 2)

sample:
  if frame == 0:
    impulse = 1.0
  else:
    impulse = 0.0
  out1 = direct(impulse)
  out2 = delayed(impulse)
  frame += 1
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = [0.0_f32; 8];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process delay lines");
    assert_eq!(output, [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
}

#[test]
fn stdlib_delay_line_supports_custom_read_before_write_feedback() {
    let source = r#"
import std/delay

init:
  frame = 0
  line = std::delay<16>::Line()

sample:
  if frame == 0:
    impulse = 1.0
  else:
    impulse = 0.0
  delayed = line.readL(2.0)
  line.write(impulse + delayed * 0.5)
  line.advance()
  out1 = delayed
  frame += 1
"#;
    let frames = 9;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 9];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process custom delay feedback");
    assert_eq!(output, [0.0, 0.0, 1.0, 0.0, 0.5, 0.0, 0.25, 0.0, 0.125]);
}

#[test]
fn stdlib_feedback_delay_crossfades_abrupt_time_changes() {
    let source = r#"
import std/delay

init:
  frame = 0
  delay = std::delay<32>::CrossfadeDelay(
    delay_s = 2.0 / SR,
    feedback = 0.0,
    mix = 1.0,
    transition_s = 4.0 / SR
  )

sample:
  if frame < 6:
    requested_delay = 2.0 / SR
  else:
    requested_delay = 4.0 / SR
  out1 = delay(f32(frame + 1), delay_s = requested_delay)
  frame += 1
"#;
    let frames = 11;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 11];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process crossfade feedback delay");
    let expected = [0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 5.5, 6.0, 6.5, 7.0];
    assert_eq!(output, expected);
}

#[test]
fn stdlib_feedback_delay_slews_one_read_head_for_time_changes() {
    let source = r#"
import std/delay

init:
  frame = 0
  delay = std::delay<32>::Delay(
    delay_s = 2.0 / SR,
    feedback = 0.0,
    mix = 1.0,
    transition_s = 4.0 / SR
  )

sample:
  if frame < 6:
    requested_delay = 2.0 / SR
  else:
    requested_delay = 4.0 / SR
  out1 = delay(f32(frame + 1), delay_s = requested_delay)
  frame += 1
"#;
    let frames = 11;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 11];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process Doppler feedback delay");
    assert!(output[6] > output[5] && output[6] < 5.0, "{output:?}");
    for pair in output[5..].windows(2) {
        assert!(pair[1] > pair[0], "{output:?}");
    }
}

#[test]
fn stdlib_feedback_delay_zero_transition_changes_time_immediately() {
    let source = r#"
import std/delay

init:
  frame = 0
  delay = std::delay<32>::Delay(
    delay_s = 2.0 / SR,
    feedback = 0.0,
    mix = 1.0,
    transition_s = 0.0
  )

sample:
  if frame < 6:
    requested_delay = 2.0 / SR
  else:
    requested_delay = 4.0 / SR
  out1 = delay(f32(frame + 1), delay_s = requested_delay)
  frame += 1
"#;
    let frames = 11;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 11];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process immediate feedback delay");
    assert_eq!(
        output,
        [0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 3.0, 4.0, 5.0, 6.0, 7.0]
    );
}

#[test]
fn stdlib_feedback_delays_repeat_at_the_requested_interval() {
    let source = r#"
import std/delay

init:
  frame = 0
  delay = std::delay<16>::Delay(
    delay_s = 2.0 / SR,
    feedback = 0.5,
    mix = 1.0,
    transition_s = 0.0
  )
  crossfade = std::delay<16>::CrossfadeDelay(
    delay_s = 2.0 / SR,
    feedback = 0.5,
    mix = 1.0,
    transition_s = 0.0
  )

sample:
  if frame == 0:
    impulse = 1.0
  else:
    impulse = 0.0
  out1 = delay(impulse)
  out2 = crossfade(impulse)
  frame += 1
"#;
    let frames = 9;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = [0.0_f32; 18];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process feedback delay impulse");
    assert_eq!(
        output,
        [
            0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0, 0.25, 0.25, 0.0, 0.0,
            0.125, 0.125,
        ]
    );
}

#[test]
fn stdlib_feedback_delay_does_not_restart_an_active_time_crossfade() {
    let source = r#"
import std/delay

init:
  frame = 0
  delay = std::delay<32>::CrossfadeDelay(
    delay_s = 2.0 / SR,
    feedback = 0.0,
    mix = 1.0,
    transition_s = 4.0 / SR
  )

sample:
  if frame < 6:
    requested_delay = 2.0 / SR
  else:
    requested_delay = f32(frame - 2) / SR
  out1 = delay(f32(frame + 1), delay_s = requested_delay)
  frame += 1
"#;
    let frames = 12;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 12];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("process continuously retargeted feedback delay");
    assert_eq!(
        output,
        [0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 5.5, 6.0, 6.5, 7.0, 7.0]
    );
}

#[test]
fn stdlib_dynamics_compressor_and_limiter_link_channels() {
    let source = r#"
import std/dynamics

init:
  compressor = std::dynamics::Compressor(
    threshold_db = -12.0,
    ratio = 4.0,
    attack_s = 0.0,
    release_s = 0.0,
    knee_db = 0.0
  )
  limiter = std::dynamics::Limiter(ceiling_db = -6.0, release_s = 0.0)

sample:
  compressor(1.0, 1.0)
  out1 = compressor.out1
  limiter(2.0, -1.0)
  out2 = limiter.out1
  out3 = limiter.out2
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 3);

    let mut output = [0.0_f32; 6];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process dynamics");
    for frame in output.as_chunks::<3>().0 {
        assert!(
            frame[0] > 0.3 && frame[0] < 0.4,
            "compressor output: {frame:?}"
        );
        assert!(frame[1] <= 0.502, "limiter left output: {frame:?}");
        assert_near(frame[2], frame[1] * -0.5, 1e-6);
    }
}

#[test]
fn stdlib_sample_player_duplicates_mono_and_reports_completion() {
    let source = r#"
import std/sample

buffers:
  clip: f32

init:
  player = std::sample::Player(clip = clip)
  did_finish = false

event play():
  player.play()

when player.finished():
  did_finish = true

sample:
  player()
  out1 = player.out1
  out2 = player.out2
  if did_finish:
    out3 = 1.0
  else:
    out3 = 0.0
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 3);

    let mut clip = [0.25_f32, 0.75_f32];
    bind_buffer(
        &mut instance,
        0,
        clip.as_mut_ptr().cast(),
        clip.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind sample clip");
    let play = instance.event_index("play").expect("play event");
    trigger_event_by_index(
        &mut instance,
        play,
        &[],
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("play should succeed");

    let mut output = [0.0_f32; 12];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process player");
    assert_eq!(
        output,
        [0.25, 0.25, 0.0, 0.75, 0.75, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0,]
    );
}

#[test]
fn stdlib_sample_player_stops_once_when_its_clip_is_unbound() {
    let source = r#"
import std/sample

buffers:
  clip: f32

init:
  player = std::sample<1>::Player(clip = clip, looping = true)
  finish_count = 0
  loop_count = 0

event play():
  player.play()

when player.finished():
  finish_count = finish_count + 1

when player.looped():
  loop_count = loop_count + 1

sample:
  player()
  out1 = f32(finish_count)
  out2 = f32(loop_count)
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let play = instance.event_index("play").expect("play event");
    trigger_event_by_index(
        &mut instance,
        play,
        &[],
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("play should succeed");

    let mut output = [0.0_f32; 8];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("unbound player should process");
    assert_eq!(output, [1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0]);
}

#[test]
fn stdlib_sample_player_events_normalize_against_the_bound_clip() {
    let source = r#"
import std/sample

buffers:
  clip: f32

init:
  player = std::sample<1>::Player(clip = clip, looping = true)

events:
  play(frame: f32):
    player.play(frame)

  seek(frame: f32):
    player.seek(frame)

sample:
  out1 = player()
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut clip = [0.0_f32, 1.0, 2.0, 3.0];
    bind_buffer(
        &mut instance,
        0,
        clip.as_mut_ptr().cast(),
        clip.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind sample clip");

    let play = instance.event_index("play").expect("play event");
    trigger_event_by_index(
        &mut instance,
        play,
        &(-1.0_f32).to_ne_bytes(),
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("play should normalize from the bound clip length");

    let mut output = [0.0_f32; 2];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process wrapped play");
    assert_eq!(output, [3.0, 0.0]);

    let seek = instance.event_index("seek").expect("seek event");
    trigger_event_by_index(
        &mut instance,
        seek,
        &5.0_f32.to_ne_bytes(),
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("seek should normalize from the bound clip length");

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process wrapped seek");
    assert_eq!(output, [1.0, 2.0]);
}

#[test]
fn stdlib_sample_player_specializes_for_f64_buffers() {
    let source = r#"
import std/sample

buffers:
  clip: f64

init:
  player = std::sample::Player<f64>(clip = clip)

event play():
  player.play()

sample:
  player()
  out1 = f32(player.out1)
  out2 = f32(player.out2)
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut clip = [0.125_f64, 0.875_f64];
    bind_buffer(
        &mut instance,
        0,
        clip.as_mut_ptr().cast(),
        clip.len(),
        1,
        48_000.0,
        PrimitiveType::F64,
    )
    .expect("bind f64 sample clip");
    let play = instance.event_index("play").expect("play event");
    trigger_event_by_index(
        &mut instance,
        play,
        &[],
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("play should succeed");

    let mut output = [0.0_f32; 4];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process f64 player");
    assert_eq!(output, [0.125, 0.125, 0.875, 0.875]);
}

#[test]

fn stdlib_square_supports_f64_and_stays_bounded() {
    let frames = 128;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_OSC_SQUARE_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|sample| *sample > 0.05),
        "expected square output to go positive, got {output:?}"
    );
    assert!(
        output.iter().any(|sample| *sample < -0.05),
        "expected square output to go negative, got {output:?}"
    );
    assert!(
        output.iter().all(|sample| sample.abs() <= 0.3),
        "expected square output to stay within amp bounds, got {output:?}"
    );
}

#[test]
fn stdlib_ksine_supports_f64_and_advances_once_per_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_OSC_KSINE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process first block");
    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process second block");
    for sample in &output {
        assert_near(*sample, 0.25, 1e-6);
    }
}

#[test]

fn stdlib_osc_phasor_param_call_updates_within_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_OSC_PHASOR_DYNAMIC_PARAM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.25, 1e-6);
    assert_near(output[2], 0.5, 1e-6);
    assert_near(output[3], 0.75, 1e-6);
}

#[test]
fn stdlib_osc_parent_param_hooks_update_child_oscillators() {
    let source = r#"
import std/osc

init:
  oscillator = std::osc::Sine(freq = 0.0)
  frame = 0

sample:
  if frame == 0:
    oscillator.freq = SR * 0.25
  out1 = oscillator()
  frame += 1
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 2];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process sine hook");
    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
}

#[test]

fn proc_bind_hook_init_uses_top_level_oversample_rate() {
    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_BIND_HOOK_INIT_USES_TOP_LEVEL_OVERSAMPLE_RATE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[frames - 1], 0.5, 1e-3);
}

#[test]

fn proc_bind_hook_init_through_def_proc_array_uses_oversample_rate() {
    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_BIND_HOOK_INIT_THROUGH_DEF_PROC_ARRAY_USES_OVERSAMPLE_RATE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[frames - 1], 0.5, 1e-3);
}

#[test]

fn explicit_oversampled_proc_from_oversampled_context_is_rejected() {
    let parsed = parse_program(EXPLICIT_OVERSAMPLED_PROC_FROM_OVERSAMPLED_CONTEXT_REJECTED_EXAMPLE)
        .expect("parse should succeed");

    let err = analyze(parsed).expect_err("nested explicit oversampling should be rejected");
    let joined = err
        .iter()
        .map(|diag| diag.message.clone())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains("cannot call explicitly oversampled child"),
        "unexpected diagnostics: {joined}"
    );
}

#[test]

fn stdlib_saw_applies_amp_to_the_full_waveform() {
    let frames = 256;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_OSC_SAW_AMP_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let peak = output
        .iter()
        .fold(0.0_f32, |acc, sample| acc.max(sample.abs()));

    assert!(peak >= 0.15, "expected audible saw output, got {output:?}");
    assert!(
        peak <= 0.3,
        "expected amp-scaled saw output near 0.25 peak, got peak {peak} from {output:?}"
    );
}

#[test]

fn stdlib_triangle_supports_f64_and_stays_bounded() {
    let frames = 256;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_OSC_TRIANGLE_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|sample| *sample > 0.05),
        "expected triangle output to go positive, got {output:?}"
    );
    assert!(
        output.iter().any(|sample| *sample < -0.05),
        "expected triangle output to go negative, got {output:?}"
    );
    assert!(
        output.iter().all(|sample| sample.abs() <= 0.3),
        "expected triangle output to stay within amp bounds, got {output:?}"
    );
}

#[test]

fn stdlib_one_pole_lowpass_and_highpass_modes_behave_distinctly() {
    let frames = 256;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FILTER_ONE_POLE_MODES_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let tail = &output[(frames - 32) * out_channels..];
    let mut low_sum = 0.0_f32;
    let mut high_sum = 0.0_f32;

    for frame in tail.chunks_exact(out_channels) {
        low_sum += frame[0];
        high_sum += frame[1].abs();
    }

    let low_avg = low_sum / 32.0;
    let high_avg = high_sum / 32.0;

    assert!(
        low_avg >= 0.9,
        "expected low-pass output to settle near DC input, got avg {low_avg}"
    );
    assert!(
        high_avg <= 0.1,
        "expected high-pass output to reject steady DC input, got avg {high_avg}"
    );
}

#[test]

fn stdlib_dcblock_attenuates_steady_dc() {
    let frames = 512;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FILTER_DCBLOCK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output[0].abs() >= 0.9,
        "expected DC blocker to pass the initial transient, got {output:?}"
    );

    let tail = &output[frames - 64..];
    let tail_avg = tail.iter().map(|sample| sample.abs()).sum::<f32>() / tail.len() as f32;

    assert!(
        tail_avg <= 0.15,
        "expected DC blocker tail to decay toward zero, got avg {tail_avg} from {tail:?}"
    );
}

#[test]

fn stdlib_svf_extra_modes_are_bounded_and_distinct() {
    let frames = 256;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FILTER_SVF_EXTRA_MODES_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let mut diff_np = 0.0_f32;
    let mut diff_na = 0.0_f32;

    for frame in output.chunks_exact(out_channels) {
        assert!(
            frame.iter().all(|sample| sample.abs() <= 1.0),
            "expected bounded SVF extra mode output, got {output:?}"
        );
        diff_np += (frame[0] - frame[1]).abs();
        diff_na += (frame[0] - frame[2]).abs();
    }

    assert!(
        diff_np >= 0.5,
        "expected notch and peak to differ, got {output:?}"
    );
    assert!(
        diff_na >= 0.5,
        "expected notch and allpass to differ, got {output:?}"
    );
}

#[test]

fn stdlib_noise_family_outputs_are_bounded_and_nonzero() {
    let frames = 256;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_NOISE_FAMILY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let mut max_abs = [0.0_f32; 3];

    for frame in output.chunks_exact(out_channels) {
        for channel in 0..out_channels {
            max_abs[channel] = max_abs[channel].max(frame[channel].abs());
            assert!(
                frame[channel].abs() <= 0.3,
                "expected bounded noise output, got {output:?}"
            );
        }
    }

    for peak in max_abs {
        assert!(
            peak >= 0.01,
            "expected non-silent noise output, got peaks {max_abs:?}"
        );
    }
}

#[test]

fn stdlib_white_noise_supports_f64() {
    let frames = 128;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_NOISE_WHITE_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|sample| sample.abs() >= 0.01),
        "expected non-silent white noise output, got {output:?}"
    );
    assert!(
        output.iter().all(|sample| sample.abs() <= 0.3),
        "expected bounded white noise output, got {output:?}"
    );
}

#[test]

fn stdlib_levels_helpers_return_expected_values() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_LEVELS_HELPERS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 8);

    let mut output = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 10.0_f32.powf(-6.0 / 20.0), 1e-5);
    assert_near(output[1], 20.0 * 0.5_f32.log10(), 1e-5);
    assert_near(output[2], 0.5, 1e-6);
    assert_near(output[3], 0.5, 1e-6);
    assert_near(output[4], 0.5_f32.sqrt(), 1e-5);
    assert_near(output[5], 0.5_f32.sqrt(), 1e-5);
    assert_near(output[6], 10.0_f32.powf(-6.0 / 20.0), 1e-5);
    assert_near(output[7], 20.0 * 0.5_f32.log10(), 1e-5);
}

#[test]

fn stdlib_lag_rises_toward_target_without_jumping_immediately() {
    let frames = 64;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_SMOOTHING_LAG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output[0] > 0.0 && output[0] < 1.0,
        "expected lag output to move but not jump immediately, got {output:?}"
    );
    assert!(
        output[frames - 1] > output[0],
        "expected lag output to rise over time, got {output:?}"
    );
    assert!(
        output[frames - 1] >= 0.6,
        "expected lag output to approach the target, got {output:?}"
    );

    for pair in output.windows(2) {
        assert!(
            pair[1] + 1e-6 >= pair[0],
            "expected monotonic lag rise, got {output:?}"
        );
    }
}

#[test]

fn stdlib_slew_supports_f64_and_limits_rise_rate() {
    let frames = 6;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_SMOOTHING_SLEW_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.1, 1e-4);
    assert_near(output[1], 0.2, 1e-4);
    assert_near(output[2], 0.3, 1e-4);
    assert_near(output[3], 0.4, 1e-4);
}

#[test]

fn stdlib_mix_helpers_route_and_combine_expected_values() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MIX_HELPERS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 5);

    let mut output = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.5, 1e-6);
    assert_near(output[1], 0.5, 1e-6);
    assert_near(output[2], 0.0, 1e-6);
    assert_near(output[3], 1.25, 1e-6);
    assert_near(output[4], 0.5, 1e-6);
}

#[test]

fn stdlib_mix_namespace_channel_helpers_scale_beyond_stereo() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MIX_GENERIC_CHANNELS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 7);

    let mut output = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.25, 1e-6);
    assert_near(output[1], 0.25, 1e-6);
    assert_near(output[2], 0.25, 1e-6);
    assert_near(output[3], 0.25, 1e-6);
    assert_near(output[4], 2.5, 1e-6);
    assert_near(output[5], 0.75, 1e-6);
    assert_near(output[6], -0.5, 1e-6);
}

#[test]

fn stdlib_gain_helpers_apply_linear_and_db_scaling() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_GAIN_LINEAR_DB_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.25, 1e-6);
    assert_near(output[1], 10.0_f32.powf(-6.0 / 20.0), 1e-5);
    assert_near(output[2], -10.0_f32.powf(-6.0 / 20.0), 1e-5);
}

#[test]

fn stdlib_smoothed_db_gain_supports_f64_and_ramps() {
    let frames = 64;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_GAIN_SMOOTHED_DB_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output[0] > 0.0 && output[0] < 1.0,
        "expected smoothed gain to ramp from zero, got {output:?}"
    );
    assert!(
        output[frames - 1] > output[0],
        "expected smoothed gain to continue rising, got {output:?}"
    );
    assert!(
        output[frames - 1] >= 0.6,
        "expected smoothed gain to approach unity, got {output:?}"
    );
    assert!(
        output.iter().all(|sample| sample.abs() <= 1.0),
        "expected smoothed gain output to stay bounded, got {output:?}"
    );

    for pair in output.windows(2) {
        assert!(
            pair[1] + 1e-6 >= pair[0],
            "expected monotonic smoothed gain ramp, got {output:?}"
        );
    }
}

#[test]

fn stdlib_pitch_helpers_return_expected_values() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_PITCH_HELPERS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 6);

    let mut output = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 440.0, 1e-3);
    assert_near(output[1], 69.0, 1e-4);
    assert_near(output[2], 2.0, 1e-4);
    assert_near(output[3], 440.0, 1e-3);
    assert_near(output[4], 69.0, 1e-4);
    assert_near(output[5], 2.0, 1e-4);
}

#[test]

fn bound_io_writes_directly_for_f32_arrays() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.input_index("in1"), Some(0));

    assert_eq!(instance.output_index("out1"), Some(0));

    let in_bytes = encode_planar_f32(&[vec![1.0, 2.0, -1.0, 0.25], vec![0.5, 1.0, 2.0, -0.5]]);

    bind_input(&mut instance, 0, in_bytes.as_ptr(), in_bytes.len()).expect("bind input");

    let mut bound_out = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, bound_out.as_mut_ptr(), bound_out.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let copied_bound = decode_planar_f32(&bound_out);

    let expected = [2.5_f32, 5.0, 0.0, 0.0];

    for (sample, target) in copied_bound.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn bound_io_writes_directly_for_f64_declared_types() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_IO_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.input_type(0).as_deref(), Some("f64[2]"));

    assert_eq!(instance.output_type(0).as_deref(), Some("f64"));

    let in_bytes = encode_planar_f64(&[vec![1.0, 2.0, 4.0, 0.0], vec![0.5, 2.0, -2.0, 1.0]]);

    bind_input(&mut instance, 0, in_bytes.as_ptr(), in_bytes.len()).expect("bind input");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f64(&out_bytes);

    let expected = [1.25_f64, 3.0, 3.0, 0.5];

    for (sample, target) in out.iter().zip(expected) {
        let delta = (*sample - target).abs();

        assert!(
            delta <= 1e-9,
            "expected {sample} ~= {target}, delta={delta}"
        );
    }
}

#[test]

fn top_level_params_respect_f64_and_i64_declared_types() {
    let src = r#"

params {

  gain: f64 = 1.234567890123

  count: i64 = 9007199254740993

}

outs {

  out1: f64

  out2: i64

}

sample {

  out1 = gain

  out2 = count

}

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 2);

    assert_eq!(instance.param_type(0).as_deref(), Some("f64"));

    assert_eq!(instance.param_type(1).as_deref(), Some("i64"));

    let mut out_f64_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_f64_bytes.as_mut_ptr(),
        out_f64_bytes.len(),
    )
    .expect("bind f64 output");

    bind_output(
        &mut instance,
        1,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    for sample in decode_planar_f64(&out_f64_bytes) {
        assert!(
            (sample - 1.234567890123_f64).abs() <= 1e-12,
            "expected exact f64 param default, got {sample}"
        );
    }

    for sample in decode_planar_i64(&out_i64_bytes) {
        assert_eq!(sample, 9007199254740993_i64);
    }

    set_param_by_index(&mut instance, 0, &9.876543210987_f64.to_ne_bytes()).expect("set f64 param");

    set_param_by_index(&mut instance, 1, &9007199254740995_i64.to_ne_bytes())
        .expect("set i64 param");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    for sample in decode_planar_f64(&out_f64_bytes) {
        assert!(
            (sample - 9.876543210987_f64).abs() <= 1e-12,
            "expected exact updated f64 param, got {sample}"
        );
    }

    for sample in decode_planar_i64(&out_i64_bytes) {
        assert_eq!(sample, 9007199254740995_i64);
    }
}

#[test]

fn top_level_inputs_respect_f64_and_i64_declared_types() {
    let src = r#"

ins {

  in1: f64

  in2: i64

}

outs {

  out1: f64

  out2: i64

}

sample {

  out1 = in1

  out2 = in2

}

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 2);

    assert_eq!(instance.input_type(0).as_deref(), Some("f64"));

    assert_eq!(instance.input_type(1).as_deref(), Some("i64"));

    let in_f64 = encode_planar_f64(&[vec![
        1.234567890123_f64,
        -2.5_f64,
        0.125_f64,
        42.000000000001_f64,
    ]]);

    let in_i64 = encode_planar_i64(&[vec![
        9007199254740993_i64,
        -17_i64,
        0_i64,
        9007199254740995_i64,
    ]]);

    bind_input(&mut instance, 0, in_f64.as_ptr(), in_f64.len()).expect("bind f64 input");

    bind_input(&mut instance, 1, in_i64.as_ptr(), in_i64.len()).expect("bind i64 input");

    let mut out_f64_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_f64_bytes.as_mut_ptr(),
        out_f64_bytes.len(),
    )
    .expect("bind f64 output");

    bind_output(
        &mut instance,
        1,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out_f64 = decode_planar_f64(&out_f64_bytes);

    let out_i64 = decode_planar_i64(&out_i64_bytes);

    let expected_f64 = [1.234567890123_f64, -2.5_f64, 0.125_f64, 42.000000000001_f64];

    let expected_i64 = [9007199254740993_i64, -17_i64, 0_i64, 9007199254740995_i64];

    for (sample, expected) in out_f64.iter().zip(expected_f64) {
        assert!(
            (*sample - expected).abs() <= 1e-12,
            "expected exact f64 input sample {expected}, got {sample}"
        );
    }

    assert_eq!(out_i64.as_slice(), expected_i64.as_slice());
}

#[test]

fn top_level_event_arguments_respect_f64_and_i64_declared_types() {
    let src = r#"

outs {

  out1: f64

  out2: i64

}

events {

  set(value: f64, count: i64) {

    level = value

    total = count

  }

}

init {

  level: f64 = 0.0

  total: i64 = 0

}

sample {

  out1 = level

  out2 = total

}

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 2);

    assert_eq!(instance.event_count(), 1);

    assert_eq!(instance.event_payload_bytes(0), Some(16));

    let mut payload = Vec::new();

    payload.extend_from_slice(&1.234567890123_f64.to_ne_bytes());

    payload.extend_from_slice(&9007199254740993_i64.to_ne_bytes());

    trigger_event_by_index(
        &mut instance,
        0,
        &payload,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("trigger event");

    let mut out_f64_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_f64_bytes.as_mut_ptr(),
        out_f64_bytes.len(),
    )
    .expect("bind f64 output");

    bind_output(
        &mut instance,
        1,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    for sample in decode_planar_f64(&out_f64_bytes) {
        assert!(
            (sample - 1.234567890123_f64).abs() <= 1e-12,
            "expected exact f64 event payload, got {sample}"
        );
    }

    for sample in decode_planar_i64(&out_i64_bytes) {
        assert_eq!(sample, 9007199254740993_i64);
    }
}

#[test]

fn top_level_buffers_respect_f64_and_i64_declared_types() {
    let src = r#"

buffers {

  buf1: buffer<f64>

  buf2: buffer<i64>

}

outs {

  out1: f64

  out2: i64

}

init {

  idx: i32 = 0

}

sample {

  out1 = buf1[idx]

  out2 = buf2[idx]

  idx = idx + 1

}

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 2);

    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer<f64>"));

    assert_eq!(instance.buffer_type(1).as_deref(), Some("buffer<i64>"));

    assert_eq!(instance.output_type(0).as_deref(), Some("f64"));

    assert_eq!(instance.output_type(1).as_deref(), Some("i64"));

    let mut buf_f64 = vec![1.234567890123_f64, -2.5_f64, 0.125_f64, 42.000000000001_f64];

    let mut buf_i64 = vec![9007199254740993_i64, -17_i64, 0_i64, 9007199254740995_i64];

    bind_buffer(
        &mut instance,
        0,
        buf_f64.as_mut_ptr().cast::<u8>(),
        buf_f64.len(),
        1,
        48_000.0,
        PrimitiveType::F64,
    )
    .expect("bind f64 buffer");

    bind_buffer(
        &mut instance,
        1,
        buf_i64.as_mut_ptr().cast::<u8>(),
        buf_i64.len(),
        1,
        48_000.0,
        PrimitiveType::I64,
    )
    .expect("bind i64 buffer");

    let mut out_f64_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_f64_bytes.as_mut_ptr(),
        out_f64_bytes.len(),
    )
    .expect("bind f64 output");

    bind_output(
        &mut instance,
        1,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out_f64 = decode_planar_f64(&out_f64_bytes);

    let out_i64 = decode_planar_i64(&out_i64_bytes);

    let expected_f64 = [1.234567890123_f64, -2.5_f64, 0.125_f64, 42.000000000001_f64];

    let expected_i64 = [9007199254740993_i64, -17_i64, 0_i64, 9007199254740995_i64];

    for (sample, expected) in out_f64.iter().zip(expected_f64) {
        assert!(
            (*sample - expected).abs() <= 1e-12,
            "expected exact f64 buffer sample {expected}, got {sample}"
        );
    }

    assert_eq!(out_i64.as_slice(), expected_i64.as_slice());
}

#[test]

fn proc_params_inputs_and_outputs_respect_f64_and_i64_declared_types() {
    let src = r#"

ins {

  in1: f64

  in2: i64

}

proc Voice {

  ins {

    in1: f64

    in2: i64

  }

  params {

    gain: f64 = 0.0

    count: i64 = 0

  }

  outs {

    out1: f64

    out2: i64

  }

  sample {

    out1 = in1 + gain

    out2 = in2 + count

  }

}

outs {

  out1: f64

  out2: i64

}

init {

  voice = Voice(gain = 1.234567890123, count = 9007199254740993)

}

sample {

  voice(in1, in2)

  out1 = voice.out1

  out2 = voice.out2

}

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 2);

    assert_eq!(instance.input_type(0).as_deref(), Some("f64"));

    assert_eq!(instance.input_type(1).as_deref(), Some("i64"));

    assert_eq!(instance.output_type(0).as_deref(), Some("f64"));

    assert_eq!(instance.output_type(1).as_deref(), Some("i64"));

    let in_f64 = encode_planar_f64(&[vec![0.0_f64, -2.5_f64, 0.125_f64, 42.000000000001_f64]]);

    let in_i64 = encode_planar_i64(&[vec![0_i64, -17_i64, 0_i64, 2_i64]]);

    bind_input(&mut instance, 0, in_f64.as_ptr(), in_f64.len()).expect("bind f64 input");

    bind_input(&mut instance, 1, in_i64.as_ptr(), in_i64.len()).expect("bind i64 input");

    let mut out_f64_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_f64_bytes.as_mut_ptr(),
        out_f64_bytes.len(),
    )
    .expect("bind f64 output");

    bind_output(
        &mut instance,
        1,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out_f64 = decode_planar_f64(&out_f64_bytes);

    let out_i64 = decode_planar_i64(&out_i64_bytes);

    let expected_f64 = [
        1.234567890123_f64,
        -1.265432109877_f64,
        1.359567890123_f64,
        43.234567890124_f64,
    ];

    let expected_i64 = [
        9007199254740993_i64,
        9007199254740976_i64,
        9007199254740993_i64,
        9007199254740995_i64,
    ];

    for (sample, expected) in out_f64.iter().zip(expected_f64) {
        assert!(
            (*sample - expected).abs() <= 1e-12,
            "expected exact proc f64 sample {expected}, got {sample}"
        );
    }

    assert_eq!(out_i64.as_slice(), expected_i64.as_slice());
}

#[test]

fn proc_event_arguments_and_outputs_respect_f64_and_i64_declared_types() {
    let src = r#"

proc Voice {

  outs {

    out1: f64

    out2: i64

  }

  events {

    set(value: f64, count: i64) {

      level = value

      total = count

    }

  }

  init {

    level: f64 = 0.0

    total: i64 = 0

  }

  sample {

    out1 = level

    out2 = total

  }

}

outs {

  out1: f64

  out2: i64

}

events {

  set(value: f64, count: i64) {

    voice.set(value, count)

  }

}

init {

  voice = Voice()

}

sample {

  voice()

  out1 = voice.out1

  out2 = voice.out2

}

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 2);

    assert_eq!(instance.event_count(), 1);

    assert_eq!(instance.event_payload_bytes(0), Some(16));

    assert_eq!(instance.output_type(0).as_deref(), Some("f64"));

    assert_eq!(instance.output_type(1).as_deref(), Some("i64"));

    let mut payload = Vec::new();

    payload.extend_from_slice(&1.234567890123_f64.to_ne_bytes());

    payload.extend_from_slice(&9007199254740993_i64.to_ne_bytes());

    trigger_event_by_index(
        &mut instance,
        0,
        &payload,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("trigger event");

    let mut out_f64_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_f64_bytes.as_mut_ptr(),
        out_f64_bytes.len(),
    )
    .expect("bind f64 output");

    bind_output(
        &mut instance,
        1,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    for sample in decode_planar_f64(&out_f64_bytes) {
        assert!(
            (sample - 1.234567890123_f64).abs() <= 1e-12,
            "expected exact proc event f64 payload, got {sample}"
        );
    }

    for sample in decode_planar_i64(&out_i64_bytes) {
        assert_eq!(sample, 9007199254740993_i64);
    }
}

#[test]

fn proc_buffers_and_outputs_respect_f64_and_i64_declared_types() {
    let src = r#"

buffers {

  buf1: buffer<f64>

  buf2: buffer<i64>

}

proc Reader {

  buffers {

    line1: buffer<f64>

    line2: buffer<i64>

  }

  outs {

    out1: f64

    out2: i64

  }

  init {

    idx: i32 = 0

  }

  sample {

    out1 = line1[idx]

    out2 = line2[idx]

    idx = idx + 1

  }

}

outs {

  out1: f64

  out2: i64

}

init {

  reader = Reader(line1 = buf1, line2 = buf2)

}

sample {

  reader()

  out1 = reader.out1

  out2 = reader.out2

}

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 2);

    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer<f64>"));

    assert_eq!(instance.buffer_type(1).as_deref(), Some("buffer<i64>"));

    assert_eq!(instance.output_type(0).as_deref(), Some("f64"));

    assert_eq!(instance.output_type(1).as_deref(), Some("i64"));

    let mut buf_f64 = vec![1.234567890123_f64, -2.5_f64, 0.125_f64, 42.000000000001_f64];

    let mut buf_i64 = vec![9007199254740993_i64, -17_i64, 0_i64, 9007199254740995_i64];

    bind_buffer(
        &mut instance,
        0,
        buf_f64.as_mut_ptr().cast::<u8>(),
        buf_f64.len(),
        1,
        48_000.0,
        PrimitiveType::F64,
    )
    .expect("bind proc f64 buffer");

    bind_buffer(
        &mut instance,
        1,
        buf_i64.as_mut_ptr().cast::<u8>(),
        buf_i64.len(),
        1,
        48_000.0,
        PrimitiveType::I64,
    )
    .expect("bind proc i64 buffer");

    let mut out_f64_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_f64_bytes.as_mut_ptr(),
        out_f64_bytes.len(),
    )
    .expect("bind f64 output");

    bind_output(
        &mut instance,
        1,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out_f64 = decode_planar_f64(&out_f64_bytes);

    let out_i64 = decode_planar_i64(&out_i64_bytes);

    let expected_f64 = [1.234567890123_f64, -2.5_f64, 0.125_f64, 42.000000000001_f64];

    let expected_i64 = [9007199254740993_i64, -17_i64, 0_i64, 9007199254740995_i64];

    for (sample, expected) in out_f64.iter().zip(expected_f64) {
        assert!(
            (*sample - expected).abs() <= 1e-12,
            "expected exact proc buffer f64 sample {expected}, got {sample}"
        );
    }

    assert_eq!(out_i64.as_slice(), expected_i64.as_slice());
}

#[test]

fn buffer_mono_read_uses_clamped_index_path() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    assert_eq!(instance.buffer_name(0), Some("buf1"));

    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer<f32>"));

    assert_eq!(instance.buffer_index("buf1"), Some(0));

    let mut buf = vec![10.0_f32, 20.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    let expected = [10.0_f32, 20.0, 20.0, 20.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn buffer_i32_mono_read_uses_clamped_index_path() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_I32_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer<i32>"));

    let mut buf = vec![10_i32, 20_i32];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::I32,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = [10.0_f32, 20.0, 20.0, 20.0];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn buffer_i64_mono_read_uses_clamped_index_path() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_I64_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer<i64>"));

    let mut buf = vec![10_i64, 20_i64];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::I64,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = [10.0_f32, 20.0, 20.0, 20.0];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn buffer_bool_mono_read_uses_clamped_index_path() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_BOOL_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_type(0).as_deref(), Some("buffer<bool>"));

    let mut buf = vec![1_u8, 0_u8];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::Bool,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = [1.0_f32, 0.0, 0.0, 0.0];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn indexed_access_supports_mono_buffers() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_RW_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![1.0_f32, 2.0, 3.0, 4.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 7.0, 1e-6);
    }

    assert_near(buf[1], 7.0, 1e-6);
}

#[test]

fn validate_bindings_and_process_unchecked_work() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![10.0_f32, 20.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    validate_bindings(&mut instance).expect("validate bindings should succeed");

    unsafe {
        process_unchecked(&mut instance, onda_runtime::ExecutionOutput::none())
            .expect("unchecked process should succeed");
    }

    let out = decode_planar_f32(&out_bytes);

    let expected = [10.0_f32, 20.0, 20.0, 20.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn validate_bindings_rejects_missing_required_bindings() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![10.0_f32, 20.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let result = validate_bindings(&mut instance);

    assert!(
        result.is_err(),
        "validate_bindings should reject missing required output binding"
    );
}

#[test]

fn validate_domains_allow_partial_revalidation() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_MONO_CLAMP_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf_a = vec![10.0_f32, 20.0];

    bind_buffer(
        &mut instance,
        0,
        buf_a.as_mut_ptr().cast::<u8>(),
        buf_a.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer A");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    validate_buffers(&mut instance).expect("validate buffers should succeed");

    validate_outputs(&mut instance).expect("validate outputs should succeed");

    unsafe {
        process_unchecked(&mut instance, onda_runtime::ExecutionOutput::none())
            .expect("unchecked process should succeed");
    }

    let out_a = decode_planar_f32(&out_bytes);

    let expected_a = [10.0_f32, 20.0, 20.0, 20.0];

    for (sample, target) in out_a.iter().zip(expected_a) {
        assert_near(*sample, target, 1e-6);
    }

    let mut buf_b = vec![3.0_f32, 4.0];

    bind_buffer(
        &mut instance,
        0,
        buf_b.as_mut_ptr().cast::<u8>(),
        buf_b.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer B");

    validate_buffers(&mut instance).expect("validate buffers after rebind should succeed");

    unsafe {
        process_unchecked(&mut instance, onda_runtime::ExecutionOutput::none())
            .expect("unchecked process should succeed");
    }

    let out_b = decode_planar_f32(&out_bytes);

    let expected_b = [4.0_f32, 4.0, 4.0, 4.0];

    for (sample, target) in out_b.iter().zip(expected_b) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn buffer_stereo_two_dim_read_and_clamp_work() {
    let frames = 6;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_STEREO_2D_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    let expected = [10.0_f32, 20.0, 30.0, 40.0, 40.0, 40.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn buffer_stereo_two_dim_write_works() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_STEREO_2D_WRITE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 7.0, 1e-6);
    }

    assert_near(buf[1], 7.0, 1e-6);
}

#[test]

fn indexed_access_supports_multichannel_buffers() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_STEREO_2D_RW_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 13.0, 1e-6);
    }

    assert_near(buf[3], 13.0, 1e-6);
}

#[test]

fn buffer_stereo_rejects_one_dim_indexing() {
    let parsed = parse_program(BUFFER_STEREO_1D_INDEX_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject one-dimensional indexing on multichannel buffers"
    );
}

#[test]

fn buffer_static_chans_returns_declared_channel_count() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_STATIC_CHANS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 2.0, 1e-6);
    }
}

#[test]

fn buffer_dynamic_chans_returns_runtime_channel_count() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_DYNAMIC_CHANS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, 100.0, //
        2.0, 20.0, 200.0, //
        3.0, 30.0, 300.0, //
        4.0, 40.0, 400.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        3,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]

fn buffer_dynamic_len_returns_runtime_frame_count() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUFFER_DYNAMIC_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        3,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]

fn def_can_take_mono_buffer_typed_param() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_MONO_PARAM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![10.0_f32, 20.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    let expected = [10.0_f32, 20.0, 20.0, 20.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn def_dynamic_buffer_len_returns_runtime_frame_count() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DYNAMIC_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        3,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 3.0, 1e-6);
    }
}

#[test]

fn def_can_take_stereo_buffer_typed_param() {
    let frames = 6;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_STEREO_PARAM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    let expected = [10.0_f32, 20.0, 30.0, 40.0, 40.0, 40.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn def_buffer_typed_param_rejects_element_type_mismatch() {
    let parsed =
        parse_program(DEF_BUFFER_PARAM_TYPE_MISMATCH_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject def buffer typed param element type mismatch"
    );
}

#[test]

fn indexed_access_supports_top_level_arrays() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INDEXED_TOP_LEVEL_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 2.0, 1e-6);

        assert_near(output[base + 1], 3.0, 1e-6);
    }
}

#[test]
fn shared_scalar_dispatch_handles_sample_var_and_user_calls() {
    let frames = 4;
    let src = r#"
params { level = 4.0 }
outs { out1 }

def half(x) {
  return x * 0.5
}

init {
  buf: f32[2]
}

sample {
  idx: i32 = 1
  buf[idx] = level
  out1 = half(buf[idx])
}
"#;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn shared_scalar_dispatch_handles_def_index_and_len_calls() {
    let frames = 4;
    let src = r#"
outs { out1 }

def pick_plus_len(arr: f32[], i: i32) {
  return arr[i] + f32(arr.len())
}

sample {
  vals: f32[3] = [2.0, 3.0, 5.0]
  idx: i32 = 1
  out1 = pick_plus_len(vals, idx)
}
"#;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]

fn struct_data_field_clamps_and_truncates() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(STRUCT_DATA_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 6.5, 1e-6);
    }
}

#[test]

fn struct_data_is_per_instance() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_DATA_IS_PER_INSTANCE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn struct_data_field_non_indexed_write_is_rejected() {
    let parsed = parse_program(STRUCT_DATA_FIELD_NON_INDEXED_WRITE_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject non-indexed write to Data struct field"
    );
}

#[test]

fn implicit_io_infers_and_fills_gaps() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(IMPLICIT_IO_GAPPED_EXAMPLE, frames);

    assert_eq!(in_channels, 3);

    assert_eq!(out_channels, 2);

    let mut input = vec![0.0_f32; frames * in_channels];

    for frame in 0..frames {
        input[frame * in_channels] = 10.0;

        input[frame * in_channels + 1] = 20.0;

        input[frame * in_channels + 2] = (frame + 1) as f32;
    }

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for frame in 0..frames {
        assert_near(output[frame * out_channels], 0.0, 1e-6);

        assert_near(
            output[frame * out_channels + 1],
            ((frame + 1) as f32) * 0.5,
            1e-6,
        );
    }
}

#[test]

fn sparse_declared_io_is_expanded() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SPARSE_DECLARED_IO_EXAMPLE, frames);

    assert_eq!(in_channels, 3);

    assert_eq!(out_channels, 3);

    let mut input = vec![0.0_f32; frames * in_channels];

    for frame in 0..frames {
        input[frame * in_channels + 2] = (frame + 1) as f32;
    }

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for frame in 0..frames {
        assert_near(output[frame * out_channels], 0.0, 1e-6);

        assert_near(output[frame * out_channels + 1], 0.0, 1e-6);

        assert_near(output[frame * out_channels + 2], (frame + 1) as f32, 1e-6);
    }
}

#[test]

fn builtin_consts_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_CONSTS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = std::f32::consts::PI + 2.0 * std::f32::consts::PI;

    for sample in &output {
        assert_near(*sample, expected, 2e-3);
    }
}

#[test]

fn builtin_consts_support_lowercase_aliases() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_CONSTS_LOWERCASE_ALIASES_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = 5.0 * std::f32::consts::PI;

    for sample in &output {
        assert_near(*sample, expected, 2e-3);
    }
}

#[test]

fn builtin_consts_use_compile_time_sample_rate() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BUILTIN_CONSTS_SR_ALIAS_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 4.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-5);

    assert_near(output[1], 0.0, 1e-5);

    assert_near(output[2], -1.0, 1e-5);

    assert_near(output[3], 0.0, 1e-5);
}

#[test]

fn builtin_consts_support_samplerate_alias() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BUILTIN_CONSTS_SAMPLERATE_ALIAS_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 4.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-5);

    assert_near(output[1], 0.0, 1e-5);

    assert_near(output[2], -1.0, 1e-5);

    assert_near(output[3], 0.0, 1e-5);
}

#[test]

fn builtin_consts_support_lowercase_samplerate_alias() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BUILTIN_CONSTS_LOWERCASE_SR_ALIAS_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 4.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-5);

    assert_near(output[1], 0.0, 1e-5);

    assert_near(output[2], -1.0, 1e-5);

    assert_near(output[3], 0.0, 1e-5);
}

