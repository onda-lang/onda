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
    a = values[mark(values, 1):],
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
    gate = f64(1.0),
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
    amp = f64(0.25),
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
fn stdlib_dynamics_compressor_and_limiter_link_channels() {
    let source = r#"
import std/dynamics

init:
  compressor = std::dynamics::Compressor(
    threshold_db = -12.0,
    ratio = 4.0,
    attack_s = 0.0,
    release_s = 0.0,
    knee_db = 0.0,
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

#[test]

fn builtin_math_typed_overloads_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_MATH_TYPED_OVERLOADS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 2.0, 1e-4);

        assert_near(output[base + 1], 2.0, 1e-4);
    }
}

#[test]

fn builtin_intrinsics_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_INTRINSICS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let x = (-0.5_f32).abs()
        + (0.0_f32).cos()
        + (4.0_f32).sqrt()
        + (0.0_f32).exp()
        + (1.0_f32).exp().ln();

    let y = 2.0_f32.powf(3.0) + 3.0_f32.min(4.0) + 3.0_f32.max(4.0) + (2.0_f32).mul_add(3.0, 4.0);

    let z = (1.8_f32).floor() + (1.2_f32).ceil() + (1.6_f32).round() + (1.6_f32).trunc();

    let expected = x + y + z;

    for sample in &output {
        assert_near(*sample, expected, 1e-4);
    }
}

#[test]

fn stdlib_math_is_auto_imported() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MATH_AUTO_IMPORT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]
fn generic_calls_are_specialized_inside_index_and_slice_coordinates() {
    let source = r#"
outs:
  out1
init:
  constructed: f32[1] = [clamp(9.0, 0.0, 6.0)]
sample:
  values: f32[7] = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
  selected = values[clamp(8, 0, 6)]
  sliced = values[clamp(1, 0, 6):clamp(2, 0, 7)]
  out1 = selected + sliced[0] + constructed[0]
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

#[test]
fn nested_proc_event_updates_private_param_and_runs_bound_hook() {
    let source = r#"
proc Child:
  params:
    private value = 0.0 => update_cached
  init:
    cached = 0.0
  def update_cached():
    cached = value * 2.0
  event set(value_v: f32):
    value = value_v
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    child = Child()
  outs:
    out1
  sample:
    child.set(0.375)
    out1 = child()

init:
  parent = Parent()
sample:
  out1 = parent()
"#;
    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn stdlib_math_auto_import_allows_local_symbol_override() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_MATH_LOCAL_SYMBOL_WINS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]
fn stdlib_random_generic_rng_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_RANDOM_GENERIC_RNG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut out1_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];
    let mut out2_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];
    let mut out3_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    bind_output(&mut instance, 0, out1_bytes.as_mut_ptr(), out1_bytes.len()).expect("bind out1");
    bind_output(&mut instance, 1, out2_bytes.as_mut_ptr(), out2_bytes.len()).expect("bind out2");
    bind_output(&mut instance, 2, out3_bytes.as_mut_ptr(), out3_bytes.len()).expect("bind out3");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out1 = decode_planar_f64(&out1_bytes);
    let out2 = decode_planar_f64(&out2_bytes);
    let out3 = decode_planar_f64(&out3_bytes);

    let next_state = |state: i64| -> i64 {
        (state
            .wrapping_mul(1_103_515_245_i64)
            .wrapping_add(12_345_i64))
            & 2_147_483_647_i64
    };
    let next_unit = |state: i64| -> f64 { ((state as i32 & 2147483647) as f64) / 2147483647.0_f64 };

    let mut state = 123_i64;
    let mut expected = Vec::with_capacity(frames);
    for _ in 0..frames {
        state = next_state(state);
        let v1 = next_unit(state);
        state = next_state(state);
        let v2 = next_unit(state) * 2.0 - 1.0;
        state = next_state(state);
        let v3 = -2.0 + 4.0 * next_unit(state);
        expected.push((v1, v2, v3));
    }

    for frame in 0..frames {
        assert!(
            (out1[frame] - expected[frame].0).abs() <= 1e-6,
            "expected {} ~= {}",
            out1[frame],
            expected[frame].0
        );
        assert!(
            (out2[frame] - expected[frame].1).abs() <= 1e-6,
            "expected {} ~= {}",
            out2[frame],
            expected[frame].1
        );
        assert!(
            (out3[frame] - expected[frame].2).abs() <= 1e-6,
            "expected {} ~= {}",
            out3[frame],
            expected[frame].2
        );
    }
}

#[test]

fn stdlib_buffer_read_mono_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_READ_MONO_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

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

    for sample in &out {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn stdlib_buffer_read_linear_and_cubic_with_channel_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_INTERP_STEREO_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

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

    for sample in &out {
        assert_near(*sample, 42.0, 1e-6);
    }
}

#[test]

fn stdlib_buffer_is_auto_imported_for_arrays_and_buffers() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_BUFFER_AUTO_IMPORT_ARRAY_AND_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0, 30.0, 40.0];

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

    for sample in &out {
        assert_near(*sample, 62.0, 1e-6);
    }
}

#[test]

fn stdlib_lookup_write_array_and_buffer_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_LOOKUP_WRITE_ARRAY_AND_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![0.0_f32; 4];

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

    for sample in &out {
        assert_near(*sample, 6.5, 1e-6);
    }
}

#[test]

fn floor_fract_wrap_numeric_behavior_is_stable() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(FLOOR_FRACT_WRAP_NUMERIC_BEHAVIOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.75, 1e-6);
    }
}

#[test]

fn builtin_int_intrinsics_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BUILTIN_INT_INTRINSICS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 17.0, 1e-6);
    }
}

#[test]

fn float_only_builtin_rejects_integer_arguments() {
    let parsed =
        parse_program(BUILTIN_FLOAT_ONLY_TYPE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject integer argument for float-only builtin"
    );
}

#[test]

fn data_capacity_supports_compile_time_constants() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DATA_CONST_CAPACITY_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 16_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn data_ctor_capacity_supports_compile_time_constants() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DATA_CTOR_CONST_CAPACITY_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn block_size_constant_is_available_in_init_and_block() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BLOCK_SIZE_CONST_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, (frames as f32) * 2.0, 1e-6);
    }
}

#[test]

fn block_size_aliases_are_available() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        BLOCK_SIZE_ALIASES_CONST_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, frames as f32, 1e-6);
    }
}

#[test]

fn block_executes_once_per_process_call() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_EXEC_ONCE_PER_PROCESS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("first process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("second process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn block_assigned_scalar_is_visible_in_nested_sample() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(BLOCK_SCALAR_VISIBLE_IN_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    // We only need to assert the program compiles/runs and generates non-zero audio.

    assert!(output.iter().any(|v| v.abs() > 1e-6));
}

#[test]

fn block_cannot_access_outputs() {
    let parsed = parse_program(BLOCK_IO_FORBIDDEN_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject output access in block"
    );
}

#[test]

fn builtin_const_assignment_is_rejected() {
    let parsed = parse_program(BUILTIN_CONST_ASSIGN_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject builtin constant assignment"
    );
}

#[test]

fn builtin_const_lowercase_assignment_is_rejected() {
    let parsed =
        parse_program(BUILTIN_CONST_ASSIGN_LOWERCASE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject lowercase builtin constant assignment"
    );
}

#[test]

fn typed_narrowing_assignment_is_rejected() {
    let parsed =
        parse_program(TYPED_NARROWING_ASSIGNMENT_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject implicit i64->f32 narrowing"
    );
}

#[test]

fn if_condition_must_be_bool() {
    let parsed = parse_program(IF_CONDITION_BOOL_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject non-bool if condition"
    );
}

#[test]

fn init_if_branch_locals_do_not_introduce_state() {
    let parsed =
        parse_program(IF_BRANCH_TYPE_CONFLICT_ERROR_EXAMPLE).expect("parse should succeed");

    assert!(
        analyze(parsed).is_ok(),
        "branch-local init assignments should stay local and not introduce state"
    );
}

#[test]

fn typed_data_primitive_elements_compile_and_run() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DATA_ELEM_PRIMITIVES_OK_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut out, 1).expect("processing should succeed");

    assert!(
        (out[0] - 6.5).abs() < 1.0e-6,
        "typed array elements should preserve runtime values across primitive types"
    );
}

#[test]

fn typed_data_struct_scalar_primitives_compile_and_run() {
    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DATA_STRUCT_SCALAR_PRIMITIVES_OK_EXAMPLE, 1);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out = vec![0.0_f32; out_channels];

    process_interleaved(&mut instance, &[], &mut out, 1).expect("processing should succeed");

    assert!(
        (out[0] - 6.5).abs() < 1.0e-6,
        "Struct[N] should support all primitive scalar field types"
    );
}

#[test]

fn data_index_must_be_numeric() {
    let parsed = parse_program(DATA_BOOL_INDEX_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject bool array index"
    );
}

#[test]

fn data_constant_out_of_range_index_is_rejected_in_codegen() {
    let parsed = parse_program(DATA_CONST_OOB_INDEX_ERROR_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let result = lower_typed_and_jit(
        typed,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: 64,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert!(
        result.is_err(),
        "codegen should reject out-of-range constant array index"
    );
}

#[test]

fn def_return_type_is_inferred_from_return_expression() {
    let parsed = parse_program(DEF_RETURN_F64_INFERENCE_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let mydef = typed
        .defs
        .iter()
        .find(|d| d.name == "mydef")
        .expect("mydef should be present");

    assert_eq!(
        mydef.return_ty,
        onda_semantics::ReturnType::Scalar(PrimitiveType::F64)
    );
}

#[test]

fn def_monomorphizes_from_call_arguments_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_MONOMORPHIZES_FROM_CALL_ARGUMENTS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn generic_def_infers_scalar_t_from_concrete_call_return_compile_and_run() {
    let src = r#"

outs:

  out1: i64



def make(flag: f32):

  return i64(42)



def id<T>(x: T):

  return x



sample:

  out1 = id(make(0.0))

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 1);

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    for sample in decode_planar_i64(&out_i64_bytes) {
        assert_eq!(sample, 42);
    }
}

#[test]

fn def_monomorphizes_multiple_specializations_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEF_MONOMORPHIZES_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.5, 1e-6);
    }
}

#[test]

fn non_generic_def_rejects_type_args() {
    let parsed =
        parse_program(NON_GENERIC_DEF_WITH_TYPE_ARGS_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic defs"
    );
}

#[test]

fn generic_def_parses_successfully() {
    let parsed = parse_program(
        r#"

outs { out1 }

def identity<T>(x: T) {

  return x

}

sample { out1 = identity(1.5) }

"#,
    );

    assert!(
        parsed.is_ok(),
        "parser should accept generic type params on defs"
    );
}

#[test]

fn generic_struct_ctor_with_explicit_type_args_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_EXPLICIT_TYPE_ARGS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.75, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_infers_type_args_from_arguments() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_MISSING_TYPE_ARGS_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_infers_type_args_from_variable_arguments() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_INFER_FROM_VAR_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_defaults_unresolved_inference_to_f32() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_UNRESOLVED_INFERENCE_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn generic_struct_ctor_rejects_type_arg_arity_mismatch() {
    let parsed =
        parse_program(GENERIC_STRUCT_TYPE_ARG_ARITY_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject generic struct ctor calls with wrong type argument count"
    );
}

#[test]

fn non_generic_struct_ctor_rejects_type_args() {
    let parsed = parse_program(NON_GENERIC_STRUCT_WITH_TYPE_ARGS_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic struct ctors"
    );
}

#[test]

fn generic_struct_multiple_specializations_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn generic_struct_array_field_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_ARRAY_FIELD_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn generic_struct_method_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_STRUCT_METHOD_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn struct_method_local_int_inference_matches_def_for_bitwise_ops() {
    let src = r#"

struct Bits<T>:

  def run(self, n: i32):

    bits = 0

    value = n

    while (value > 1):

      value = value >> 1

      bits = bits + 1

    return f32(bits)

outs { out1 }

init:

  b = Bits<f32>()

sample:

  out1 = b.run(8)

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn struct_method_untyped_numeric_calls_compile_and_run() {
    let src = r#"

struct Math:

  def mix(self, x, y):

    return x * y + x



outs { out1 }

init:

  m = Math()

sample:

  a = m.mix(f32(1.5), f32(2.0))

  b = f32(m.mix(f64(1.25), f64(4.0)))

  out1 = a + b

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.75, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_with_explicit_type_args_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_EXPLICIT_TYPE_ARGS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_infers_type_args_from_arguments() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_MISSING_TYPE_ARGS_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_infers_type_args_from_defaults() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_DEFAULT_ONLY_INFERENCE_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_infers_array_generic_type_from_array_variable() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_ARRAY_INFER_FROM_ARRAY_VAR_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_defaults_unresolved_inference_to_f32() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_UNRESOLVED_INFERENCE_ERROR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn generic_proc_ctor_rejects_type_arg_arity_mismatch() {
    let parsed =
        parse_program(GENERIC_PROC_TYPE_ARG_ARITY_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject generic proc ctor calls with wrong type argument count"
    );
}

#[test]

fn non_generic_proc_ctor_rejects_type_args() {
    let parsed =
        parse_program(NON_GENERIC_PROC_WITH_TYPE_ARGS_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject explicit type arguments for non-generic proc ctors"
    );
}

#[test]

fn generic_proc_multiple_specializations_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]

fn generic_proc_array_decl_types_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_ARRAY_DECL_TYPES_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn generic_proc_init_typed_array_generic_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GENERIC_PROC_INIT_TYPED_ARRAY_GENERIC_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn generic_proc_buffer_decl_type_analyzes_and_codegen_compiles() {
    let parsed = parse_program(GENERIC_PROC_BUFFER_DECL_TYPE_COMPILES_EXAMPLE)
        .expect("parse should succeed");

    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 64,
        },
    )
    .expect("semantic analysis should succeed");

    let result = lower_typed_and_jit(
        typed,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: 64,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert!(
        result.is_ok(),
        "codegen should succeed for generic proc buffer<T> specialization"
    );
}

#[test]

fn proc_state_struct_ctor_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_STATE_STRUCT_CTOR_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_state_generic_struct_ctor_with_explicit_type_args_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_STATE_GENERIC_STRUCT_CTOR_EXPLICIT_TYPE_ARGS_OK_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_state_generic_struct_ctor_infers_type_args_compile_and_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_STATE_GENERIC_STRUCT_CTOR_INFERRED_TYPE_ARGS_OK_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn first_assignment_uses_def_return_type_and_alias_keeps_type() {
    let parsed = parse_program(FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE)
        .expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    assert_eq!(state_type_of(&typed, "x"), Some(PrimitiveType::F64));

    assert_eq!(state_type_of(&typed, "z"), Some(PrimitiveType::F64));

    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn first_assignment_from_int_literal_stays_i32() {
    let parsed =
        parse_program(FIRST_ASSIGNMENT_INT_IS_STICKY_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject implicit f32 assignment after x = 0 infers i32"
    );
}

#[test]

fn negative_literals_and_generic_negation_preserve_scalar_types() {
    let parsed = parse_program(NEGATIVE_SCALAR_INFERENCE_EXAMPLE).expect("parse should succeed");
    let typed = analyze(parsed).expect("semantic analysis should succeed");

    assert_eq!(
        state_type_of(&typed, "inferred_i32"),
        Some(PrimitiveType::I32)
    );
    assert_eq!(
        state_type_of(&typed, "inferred_i64"),
        Some(PrimitiveType::I64)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_i32"),
        Some(PrimitiveType::I32)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_i64"),
        Some(PrimitiveType::I64)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_f32"),
        Some(PrimitiveType::F32)
    );
    assert_eq!(
        state_type_of(&typed, "explicit_f64"),
        Some(PrimitiveType::F64)
    );

    let frames = 4;
    let (mut instance, in_channels, out_channels) =
        compile_instance(NEGATIVE_SCALAR_INFERENCE_EXAMPLE, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    for sample in output {
        assert_near(sample, -18.0, 1e-6);
    }

    let invalid_bool = parse_program("init { value = -true }\nsample { out1 = 0.0 }")
        .expect("boolean negation should parse before type checking");
    assert!(
        analyze(invalid_bool).is_err(),
        "unary minus must remain invalid for bool"
    );

    let declarations = parse_program(
        r#"
ins:
  inferred_input = -1
  i32_input: i32 = -1
  i64_input: i64 = -1
  f32_input: f32 = -1
  f64_input: f64 = -1

params:
  inferred_param = -1
  i32_param: i32 = -1
  i64_param: i64 = -1
  f32_param: f32 = -1
  f64_param: f64 = -1

sample:
  out1 = 0.0
"#,
    )
    .expect("negative input and parameter defaults should parse");
    let declarations =
        analyze(declarations).expect("negative input and parameter defaults should analyze");

    assert_eq!(
        declarations.in_types.get("inferred_input"),
        Some(&PrimitiveType::F32),
        "untyped inputs retain the language's f32 input default"
    );
    for (name, expected) in [
        ("i32_input", PrimitiveType::I32),
        ("i64_input", PrimitiveType::I64),
        ("f32_input", PrimitiveType::F32),
        ("f64_input", PrimitiveType::F64),
    ] {
        assert_eq!(declarations.in_types.get(name), Some(&expected));
    }
    assert_eq!(
        declarations.param_types.get("inferred_param"),
        Some(&PrimitiveType::I32),
        "untyped parameter defaults infer from the negative integer literal"
    );
    for (name, expected) in [
        ("i32_param", PrimitiveType::I32),
        ("i64_param", PrimitiveType::I64),
        ("f32_param", PrimitiveType::F32),
        ("f64_param", PrimitiveType::F64),
    ] {
        assert_eq!(declarations.param_types.get(name), Some(&expected));
    }
}

#[test]

fn proc_first_assignment_uses_def_return_type() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_FIRST_ASSIGNMENT_FROM_DEF_RETURN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn typed_widening_assignment_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_WIDENING_ASSIGNMENT_OK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_param_bind_hooks_compile_and_run() {
    let frames = 4;
    let cases = [
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 4.0)
sample:
  out1 = v()
"#,
            2.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 0.0)
  v.gain = 0.25
sample:
  out1 = v()
"#,
            0.5,
        ),
        (
            r#"
proc Voice<T>:
  params:
    gain: T = 0.0 => update
  init:
    cached = 0.0
  def update():
    cached = f32(gain) * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice<f64>(gain = f64(0.25))
sample:
  out1 = v()
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def apply(x: f32):
    cached = x * 2.0
  def update():
    apply(gain)
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 0.25)
sample:
  out1 = v()
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
  voices[0].gain = 0.25
  voice = voices[1]
  voice.gain = 0.5
sample:
  out1 = voices[0]() + voices[1]()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
  for i in 0..2:
    voices[i].gain = f32(i + 1) * 0.25
sample:
  out1 = voices[0]() + voices[1]()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice(gain = 0.25)
  v.init(gain = 0.75)
sample:
  out1 = v()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    a = 0.0 => mark
    b = 0.0 => mark
  init:
    updates = 0.0
  def mark():
    updates = updates + 1.0
  outs:
    out1
  sample:
    out1 = updates

outs:
  out1
init:
  v = Voice(a = 1.0, b = 2.0)
  v.a = 3.0
  v.b = 4.0
sample:
  out1 = v()
"#,
            4.0,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    child = Child()
  def update():
    child.gain = gain
  outs:
    out1
  sample:
    out1 = child()

outs:
  out1
init:
  p = Parent(gain = 4.0)
sample:
  out1 = p()
"#,
            2.0,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    child = Child()
  def push_child():
    child.gain = gain
  def update():
    push_child()
  outs:
    out1
  sample:
    out1 = child()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#,
            0.5,
        ),
        (
            r#"
proc Leaf:
  params:
    gain = 0.0 => update
  init:
    cached = -10.0
  def update():
    cached = gain * 4.0
  outs:
    out1
  sample:
    out1 = cached

proc Mid:
  params:
    gain = 0.0 => update
  init:
    leaf = Leaf()
  def update():
    leaf.gain = gain + 0.25
  outs:
    out1
  sample:
    out1 = leaf()

proc Parent:
  params:
    gain = 0.0 => update
  init:
    mid = Mid()
  def update():
    mid.gain = gain + 0.25
  outs:
    out1
  sample:
    out1 = mid()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#,
            3.0,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    children: Child[2] = Child()
  def update():
    for i in 0..2:
      children[i].gain = gain + f32(i) * 0.25
  outs:
    out1
  sample:
    out1 = children[0]() + children[1]()

outs:
  out1
init:
  p = Parent(gain = 0.25)
sample:
  out1 = p()
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(gain = 0.25)
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(gain = 0.25) + v(gain = 0.5)
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  ins:
    x = 0.0
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = x + cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v(v(), gain = 2.0)
"#,
            3.0,
        ),
        (
            r#"
proc Voice:
  ins:
    x = 0.0
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = x + cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v(gain = 2.0, x = v())
"#,
            4.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
sample:
  out1 = voices[0](gain = 0.25) + voices[1](gain = 0.5)
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
sample:
  mix = 0.0
  for i in 0..2:
    mix = mix + voices[i](gain = f32(i + 1) * 0.25)
  out1 = mix
"#,
            1.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
sample:
  v.gain = 1.0
  out1 = v() + v(gain = 2.0)
"#,
            3.0,
        ),
        (
            r#"
proc Voice:
  ins:
    x = 0.0
  params:
    gain = 1.0 => update
  init:
    cached = 0.0
  def update():
    cached = gain
  outs:
    out1
  sample:
    out1 = x + cached

outs:
  out1
init:
  v = Voice()
sample:
  out1 = v(x = 0.5, gain = 0.25)
"#,
            0.75,
        ),
        (
            r#"
proc Pair:
  params:
    gains: f32[2] = [0.0, 0.0]
  outs:
    out1
  sample:
    out1 = gains[0] + gains[1]

outs:
  out1
init:
  p = Pair()
sample:
  out1 = p(gains = [0.25, 0.75])
"#,
            1.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
sample:
  mix = 0.0
  for i in 0..2:
    voice = voices[i]
    mix = mix + voice(gain = f32(i + 1) * 0.25)
  out1 = mix
"#,
            1.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    child = Child()
  outs:
    out1
  sample:
    out1 = child(gain = 0.25)

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            0.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    children: Child[2] = Child()
  outs:
    out1
  sample:
    mix = 0.0
    for i in 0..2:
      mix = mix + children[i](gain = f32(i + 1) * 0.25)
    out1 = mix

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            1.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  init:
    children: Child[2] = Child()
  outs:
    out1
  sample:
    mix = 0.0
    for i in 0..2:
      child = children[i]
      mix = mix + child(gain = f32(i + 1) * 0.25)
    out1 = mix

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            1.5,
        ),
        (
            r#"
proc Leaf:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Mid:
  params:
    gain = 0.0
  init:
    leaf = Leaf()
  outs:
    out1
  sample:
    out1 = leaf(gain = gain)

proc Parent:
  init:
    mid = Mid()
  outs:
    out1
  sample:
    mid.gain = 0.25
    out1 = mid()

outs:
  out1
init:
  p = Parent()
sample:
  out1 = p()
"#,
            0.5,
        ),
        (
            r#"
proc Child:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

proc Parent:
  params:
    gain = 0.0 => update
  init:
    children: Child[2] = Child()
  def update():
    for i in 0..2:
      children[i].gain = gain + f32(i)
  outs:
    out1
  sample:
    out1 = children[0]() + children[1]()

outs:
  out1
init:
  p = Parent(gain = 2.0)
sample:
  out1 = p()
"#,
            4.0,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  v = Voice()
graph:
  0.25 >> v.gain
  v.out1 >> out1
"#,
            0.5,
        ),
        (
            r#"
proc Voice:
  params:
    gain = 0.0 {0.0, 1.0} => update
  init:
    cached = -10.0
  def update():
    cached = gain * 2.0
  outs:
    out1
  sample:
    out1 = cached

outs:
  out1
init:
  voices: Voice[2] = Voice()
graph:
  0.25 >> voices[0].gain
  0.5 >> voices[1].gain
  voices[0].out1 + voices[1].out1 >> out1
"#,
            1.5,
        ),
    ];

    for (case_idx, (src, expected)) in cases.into_iter().enumerate() {
        let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

        assert_eq!(in_channels, 0);
        assert_eq!(out_channels, 1);

        let mut output = vec![0.0_f32; frames];

        process_interleaved(&mut instance, &[], &mut output, frames)
            .expect("process should succeed");

        for (sample_idx, sample) in output.iter().enumerate() {
            assert!(
                (*sample - expected).abs() <= 1e-6,
                "case {case_idx}, sample {sample_idx}: expected {sample} ~= {expected}"
            );
        }
    }
}

#[test]

fn typed_init_f64_state_preserves_precision() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_INIT_F64_PRESERVES_PRECISION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn typed_init_i64_state_preserves_value() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_INIT_I64_PRESERVES_VALUE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn struct_default_i64_preserves_large_value() {
    let src = r#"

struct Box {

  x: i64 = 9007199254740993

}

outs { out1 }

init {

  b = Box()

}

sample {

  if (b.x == i64(9007199254740993)) { out1 = 1.0 } else { out1 = 0.0 }

}

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_default_i64_preserves_large_value() {
    let src = r#"

outs { out1 }

def exact(x: i64 = 9007199254740993) {

  if (x == i64(9007199254740993)) {

    return 1.0

  } else {

    return 0.0

  }

}

sample {

  out1 = exact()

}

"#;

    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn typed_block_declaration_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_BLOCK_DECLARATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.5, 1e-6);
    }
}

#[test]

fn typed_sample_declaration_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_SAMPLE_DECLARATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn typed_def_declaration_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_DEF_DECLARATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]

fn typed_i32_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_I32_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 100.0, 1e-6);
    }
}

#[test]

fn typed_f64_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_F64_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]

fn typed_i64_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_I64_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 100.0, 1e-6);
    }
}

#[test]

fn typed_bool_declarations_work_in_init_block_sample_and_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_BOOL_DECLARATIONS_ALL_PATHS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn def_can_infer_duck_typed_mono_buffer_param() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_PARAM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![2.0_f32, 4.0];

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

    let expected = [2.0_f32, 4.0, 4.0, 4.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn def_duck_typed_buffer_inference_propagates_through_def_calls() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_PROPAGATION_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut buf = vec![1.5_f32, 3.0];

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

    let expected = [1.5_f32, 3.0, 3.0, 3.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn def_duck_typed_buffer_param_allows_mixed_element_types() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_BUFFER_DUCK_MIXED_ELEM_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut a = vec![1.0_f32];

    bind_buffer(
        &mut instance,
        0,
        a.as_mut_ptr().cast::<u8>(),
        a.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind f32 buffer");

    let mut b = vec![2.0_f64];

    bind_buffer(
        &mut instance,
        1,
        b.as_mut_ptr().cast::<u8>(),
        b.len(),
        1,
        48_000.0,
        PrimitiveType::F64,
    )
    .expect("bind f64 buffer");

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

fn def_indexable_param_accepts_array_and_buffer_call_sites() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_INDEXABLE_ARG_ARRAY_AND_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![10.0_f32, 20.0, 30.0, 40.0];

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
        assert_near(sample, 22.0, 1e-6);
    }
}

#[test]

fn def_indexable_param_supports_two_dimensional_buffer_indexing() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_INDEXABLE_ARG_STEREO_BUFFER_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

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
        assert_near(sample, 30.0, 1e-6);
    }
}

#[test]

fn proc_single_out_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SINGLE_OUT_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_single_out_field_access_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SINGLE_OUT_FIELD_ACCESS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_multi_out_call_field_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_MULTI_OUT_CALL_FIELD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]

fn proc_multi_out_field_alias_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_MULTI_OUT_FIELD_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]

fn proc_param_mutation_is_immediate() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_PARAM_MUTATION_IMMEDIATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]

fn proc_init_block_is_optional() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_OPTIONAL_INIT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_state_typed_in_init_keeps_type_in_sample() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_TYPED_STATE_PRESERVED_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    assert_near(output[1], 2.0, 1e-6);

    assert_near(output[2], 3.0, 1e-6);

    assert_near(output[3], 4.0, 1e-6);
}

#[test]

fn proc_i32_array_increment_keeps_integer_inference() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_I32_ARRAY_INCREMENT_PRESERVED_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    assert_near(output[1], 2.0, 1e-6);

    assert_near(output[2], 3.0, 1e-6);

    assert_near(output[3], 4.0, 1e-6);
}

#[test]

fn proc_data_len_method_matches_top_level_behavior() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DATA_LEN_METHOD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]

fn proc_block_wraps_sample_once_per_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_BLOCK_WRAPS_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output_a, frames).expect("process should succeed");

    for sample in &output_a {
        assert_near(*sample, 6.0, 1e-6);
    }

    let mut output_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output_b, frames).expect("process should succeed");

    for sample in &output_b {
        assert_near(*sample, 6.0, 1e-6);
    }
}

#[test]
fn checked_segment_processing_runs_top_level_block_hooks_once_per_logical_block() {
    let frames = 4;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        0,
        segment_frames,
        PROCESS_BEGIN_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process first segment");
    let first = decode_planar_f32(&out_bytes);
    assert_near(first[0], 100.0, 1e-6);
    assert_near(first[1], 100.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        segment_frames,
        segment_frames,
        PROCESS_END_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process final segment");
    let second = decode_planar_f32(&out_bytes);
    assert_near(second[0], 0.0, 1e-6);
    assert_near(second[1], 0.0, 1e-6);
    assert_near(second[2], 100.0, 1e-6);
    assert_near(second[3], 100.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        0,
        frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process next block");
    let next = decode_planar_f32(&out_bytes);
    for sample in next {
        assert_near(sample, 201.0, 1e-6);
    }
}

#[test]
fn unchecked_segment_processing_runs_top_level_block_hooks_once_per_logical_block() {
    let frames = 4;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
outs { out1 }
init {
  pre = 0.0
  post = 0.0
}
block {
  pre = pre + 1.0
  sample {
    out1 = pre * 100.0 + post
  }
  post = post + 1.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");
    prepare_unchecked_process(&mut instance).expect("prepare unchecked process");

    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            segment_frames,
            PROCESS_BEGIN_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("process first unchecked segment");
    }
    let first = decode_planar_f32(&out_bytes);
    assert_near(first[0], 100.0, 1e-6);
    assert_near(first[1], 100.0, 1e-6);

    out_bytes.fill(0);
    unsafe {
        process_unchecked_segment(
            &mut instance,
            segment_frames,
            segment_frames,
            PROCESS_END_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("process final unchecked segment");
    }
    let second = decode_planar_f32(&out_bytes);
    assert_near(second[0], 0.0, 1e-6);
    assert_near(second[1], 0.0, 1e-6);
    assert_near(second[2], 100.0, 1e-6);
    assert_near(second[3], 100.0, 1e-6);

    out_bytes.fill(0);
    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            frames,
            PROCESS_FULL_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("process next unchecked block");
    }
    let next = decode_planar_f32(&out_bytes);
    for sample in next {
        assert_near(sample, 201.0, 1e-6);
    }
}

#[test]
fn checked_segment_processing_uses_start_frame_for_io_addressing() {
    let frames = 4;
    let segment_start = 2;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
ins { in1 }
outs { out1 }
sample {
  out1 = in1 + 10.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = [1.0_f32, 2.0, 3.0, 4.0];
    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_input(
        &mut instance,
        0,
        input.as_ptr().cast::<u8>(),
        input.len() * std::mem::size_of::<f32>(),
    )
    .expect("bind input");
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        segment_start,
        segment_frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process segment");
    let output = decode_planar_f32(&out_bytes);
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
    assert_near(output[2], 13.0, 1e-6);
    assert_near(output[3], 14.0, 1e-6);
}

#[test]
fn checked_oversampled_segment_processing_uses_start_frame_for_io_addressing() {
    let frames = 4;
    let segment_start = 2;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
outs { out1 }
sample 2 {
  out1 = 7.0
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        segment_start,
        segment_frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process oversampled segment");
    let output = decode_planar_f32(&out_bytes);
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
    assert!(
        output[2].abs() > 1e-6,
        "expected oversampled segment to write frame 2, got {output:?}"
    );
    assert!(
        output[3].abs() > 1e-6,
        "expected oversampled segment to write frame 3, got {output:?}"
    );
}

#[test]
fn segmented_dynamic_proc_array_hooks_span_one_logical_block() {
    let frames = 4;
    let segment_frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(
        r#"
proc Voice {
  outs { out1, pre, post }
  init {
    pre_count = 0.0
    post_count = 0.0
  }
  block {
    pre_count = pre_count + 1.0
    sample {
      out1 = 0.0
      pre = pre_count
      post = post_count
    }
    post_count = post_count + 1.0
  }
}
outs { out1 }
init {
  voices: Voice[2] = [Voice(), Voice()]
  idx: i32 = 1
  i: i32 = 0
}
sample {
  if i < 2 {
    out1 = 0.0
  } else {
    voices[idx]()
    v1 = voices[1]
    out1 = v1.pre * 100.0 + v1.post
  }
  i = i + 1
}
"#,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        0,
        segment_frames,
        PROCESS_BEGIN_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process inactive first segment");
    let first = decode_planar_f32(&out_bytes);
    assert_near(first[0], 0.0, 1e-6);
    assert_near(first[1], 0.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        segment_frames,
        segment_frames,
        PROCESS_END_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process active final segment");
    let second = decode_planar_f32(&out_bytes);
    assert_near(second[0], 0.0, 1e-6);
    assert_near(second[1], 0.0, 1e-6);
    assert_near(second[2], 100.0, 1e-6);
    assert_near(second[3], 100.0, 1e-6);

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        0,
        frames,
        PROCESS_FULL_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process next block");
    let next = decode_planar_f32(&out_bytes);
    for sample in next {
        assert_near(sample, 201.0, 1e-6);
    }
}

#[test]

fn proc_without_block_has_only_step_entrypoint() {
    let parsed = parse_program(PROC_OPTIONAL_INIT_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let def_names = typed
        .defs
        .iter()
        .map(|d| d.name.as_str())
        .collect::<std::collections::HashSet<_>>();

    assert!(def_names.contains("NoInitProc.__onda_proc_step"));

    assert!(!def_names.contains("NoInitProc.__onda_proc_block_pre"));

    assert!(!def_names.contains("NoInitProc.__onda_proc_block_post"));
}

#[test]

fn proc_nested_block_runs_when_outer_has_no_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_NESTED_BLOCK_WITHOUT_OUTER_BLOCK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn proc_outer_without_user_block_gets_effective_block_entrypoints_when_needed() {
    let parsed =
        parse_program(PROC_NESTED_BLOCK_WITHOUT_OUTER_BLOCK_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    let def_names = typed
        .defs
        .iter()
        .map(|d| d.name.as_str())
        .collect::<std::collections::HashSet<_>>();

    assert!(def_names.contains("OuterProc.__onda_proc_block_pre"));

    assert!(def_names.contains("OuterProc.__onda_proc_block_post"));
}

#[test]

fn proc_array_dynamic_index_runs_block_hooks_only_for_active_slot_per_block() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]

fn proc_array_dynamic_index_assignment_call_runs_block_hooks_only_for_active_slot_per_block() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]

fn nested_proc_array_dynamic_index_assignment_call_runs_block_hooks_only_for_active_slot_per_block()
{
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        NESTED_PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_ACTIVE_SLOT_ONLY_ASSIGN_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 1000.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 1110.0, 1e-6);
}

#[test]

fn proc_array_dynamic_index_block_hooks_use_same_clamped_slot_for_guard_and_call() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_BLOCK_HOOKS_CLAMPED_INDEX_CONSISTENCY_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 100.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 201.0, 1e-6);
}

#[test]

fn proc_array_dynamic_loop_scalar_sugar_into_block_proc_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    mix = mix + voices[i]()

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop scalar sugar");
}

#[test]

fn proc_array_dynamic_loop_explicit_out1_into_block_proc_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    mix = mix + voices[i]().out1

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop explicit out1");
}

#[test]

fn proc_array_dynamic_loop_statement_call_then_field_read_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    voices[i]()

    mix = mix + voices[i].out1

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop statement call then field read");
}

#[test]

fn proc_array_dynamic_loop_alias_call_into_block_proc_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"sample {

  mix = 0.0

  for i in 0..10 {

    v = voices[i]

    mix = mix + v()

  }

  out1 = mix

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop alias call");
}

#[test]

fn proc_array_dynamic_loop_inside_explicit_top_level_block_is_not_silent() {
    let src = proc_array_harmonics_block_voice_program(
        r#"block {

  sample {

    mix = 0.0

    for i in 0..10 {

      mix = mix + voices[i]()

    }

    out1 = mix

  }

}"#,
    );

    let frames = 256;

    let (mut instance, in_channels, out_channels) = compile_instance(&src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_non_silent(&output, "dynamic loop explicit top-level block");
}

#[test]

fn proc_array_dynamic_multi_call_expression_preserves_left_to_right_call_eval_order() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_INDEX_MULTI_CALL_EXPR_EVAL_ORDER_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 12.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 34.0, 1e-6);
}

#[test]

fn proc_array_dynamic_five_call_expression_preserves_left_to_right_eval_order() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_ARRAY_DYNAMIC_INDEX_FIVE_CALL_EXPR_EVAL_ORDER_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut out_a = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_a, frames).expect("process should succeed");

    assert_near(out_a[0], 12345.0, 1e-6);

    let mut out_b = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut out_b, frames).expect("process should succeed");

    assert_near(out_b[0], 67900.0, 1e-6);
}

#[test]

fn proc_can_bind_and_read_top_level_buffer() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_BUFFER_MONO_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

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

fn proc_buffer_missing_ctor_arg_is_rejected() {
    let parsed =
        parse_program(PROC_BUFFER_MISSING_CTOR_ARG_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject proc ctor missing required buffer arg"
    );
}

#[test]

fn proc_ctor_positional_args_are_rejected() {
    let parsed =
        parse_program(PROC_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject positional proc constructor arguments"
    );
}

#[test]

fn nested_proc_ctor_positional_args_are_rejected() {
    let parsed =
        parse_program(PROC_NESTED_CTOR_POSITIONAL_ARG_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject positional nested proc constructor arguments"
    );
}

#[test]

fn sample_oversample_factor_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);

    let (mut proc_instance, proc_in_channels, proc_out_channels) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(proc_in_channels, 0);

    assert_eq!(proc_out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    let mut proc_output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    process_interleaved(&mut proc_instance, &[], &mut proc_output, frames)
        .expect("proc process should succeed");

    for (actual, expected) in output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_factor_is_recorded_in_typed_program() {
    let parsed = parse_program(SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("semantic analysis should succeed");

    assert_eq!(typed.sample_oversample_factor, 4);
}

#[test]

fn proc_sample_oversample_factor_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_FACTOR_EXAMPLE, frames);

    let (mut top_level_instance, top_in_channels, top_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_2_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(top_in_channels, 0);

    assert_eq!(top_out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    let mut top_level_output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    process_interleaved(&mut top_level_instance, &[], &mut top_level_output, frames)
        .expect("top-level process should succeed");

    for (actual, expected) in output.iter().zip(top_level_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_factor_32_compiles_and_runs_smoke() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_32_SMOKE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|v| v.is_finite()));

    assert!(output[frames - 1] > output[0]);
}

#[test]

fn proc_sample_oversample_factor_64_compiles_and_runs_smoke() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_FACTOR_64_SMOKE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|v| v.is_finite()));

    assert!(output[frames - 1] > output[0]);
}

#[test]

fn sample_oversample_factor_512_compiles_and_runs_smoke() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_FACTOR_512_SMOKE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output.iter().all(|v| v.is_finite()));

    assert!(output[frames - 1] > output[0]);
}

#[test]

fn sample_oversample_invalid_factor_is_rejected() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_INVALID_FACTOR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "invalid oversample factor should be rejected"
    );

    let diags = result.expect_err("expected semantic diagnostics");

    assert!(
        diags.iter().any(|d| {
            d.message.contains("{1,2,4,8,16,32,64,128,256,512}") && d.message.contains("got 3")
        }),
        "expected explicit allowed-factor diagnostic, got: {diags:?}"
    );
}

#[test]

fn sample_oversample_const_expr_factor_is_accepted() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_CONST_EXPR_FACTOR_EXAMPLE).expect("parse should succeed");

    let typed = analyze(parsed).expect("const oversample factor should analyze");

    assert_eq!(typed.sample_oversample_factor, 4);
}

#[test]

fn proc_sample_oversample_namespace_factor_compiles_and_runs() {
    let parsed = parse_program(PROC_SAMPLE_OVERSAMPLE_NAMESPACE_FACTOR_EXAMPLE)
        .expect("parse should succeed");

    let typed = analyze(parsed).expect("namespace oversample factor should analyze");

    assert!(
        typed
            .def_sample_oversample_factors
            .values()
            .any(|&factor| factor == 8),
        "expected lowered proc oversample factor map to contain 8x specialization, got {:?}",
        typed.def_sample_oversample_factors
    );
}

#[test]

fn sample_oversample_non_literal_factor_is_rejected() {
    let parsed =
        parse_program(SAMPLE_OVERSAMPLE_NON_LITERAL_FACTOR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "non-literal oversample factor should be rejected"
    );

    let diags = result.expect_err("expected semantic diagnostics");

    assert!(
        diags.iter().any(|d| {
            d.message
                .contains("compile-time integer constant expression")
                && d.message.contains("{1,2,4,8,16,32,64,128,256,512}")
        }),
        "expected integer-literal diagnostic, got: {diags:?}"
    );
}

#[test]

fn sample_oversample_interpolates_input_reads() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE, frames);

    let (mut proc_instance, proc_in_channels, proc_out_channels) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_INPUT_INTERP_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    assert_eq!(proc_in_channels, 1);

    assert_eq!(proc_out_channels, 1);

    let input = vec![0.0_f32, 1.0, 2.0, 3.0];

    let mut output = vec![0.0_f32; frames];

    let mut proc_output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    process_interleaved(&mut proc_instance, &input, &mut proc_output, frames)
        .expect("proc process should succeed");

    for (actual, expected) in output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_passthrough_preserves_more_high_band_level() {
    let frames = 4096;

    let sample_rate = 48_000.0_f32;

    let freq = sample_rate * 0.2;

    let (mut base_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_PASSTHROUGH_1X_EXAMPLE, frames);

    let (mut over_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_PASSTHROUGH_4X_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| f32::sin(2.0 * std::f32::consts::PI * freq * idx as f32 / sample_rate))
        .collect::<Vec<_>>();

    let mut base_output = vec![0.0_f32; frames];

    let mut over_output = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &input, &mut base_output, frames)
        .expect("base passthrough should succeed");

    process_interleaved(&mut over_instance, &input, &mut over_output, frames)
        .expect("oversampled passthrough should succeed");

    let base_rms = rms_after_skip(&base_output, 512);

    let over_rms = rms_after_skip(&over_output, 512);

    let attenuation_db = 20.0 * f32::log10(over_rms / base_rms);

    assert!(

        attenuation_db > -3.0,

        "expected 4x oversampled passthrough to stay within 3 dB at 9.6 kHz, got {attenuation_db} dB"

    );
}

#[test]

fn sample_oversample_reduces_high_frequency_energy_on_nonlinear_patch() {
    let frames = 64;

    let (mut base_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_BASELINE_EXAMPLE, frames);

    let (mut over_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| if idx % 2 == 0 { 1.0_f32 } else { -1.0_f32 })
        .collect::<Vec<_>>();

    let mut base_output = vec![0.0_f32; frames];

    let mut over_output = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &input, &mut base_output, frames)
        .expect("baseline process should succeed");

    process_interleaved(&mut over_instance, &input, &mut over_output, frames)
        .expect("oversampled process should succeed");

    let high_freq_energy = |samples: &[f32]| -> f32 {
        samples
            .windows(2)
            .map(|w| {
                let d = w[1] - w[0];

                d * d
            })
            .sum::<f32>()
    };

    let base_energy = high_freq_energy(&base_output);

    let over_energy = high_freq_energy(&over_output);

    assert!(

        over_energy < base_energy * 0.5,

        "expected oversampling to reduce high-frequency energy, base={base_energy}, oversampled={over_energy}"

    );
}

#[test]

fn sample_oversample_proc_and_top_level_match_on_nonlinear_patch() {
    let frames = 64;

    let (mut top_level_instance, _, _) =
        compile_instance(SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let (mut proc_instance, _, _) =
        compile_instance(PROC_EQUIV_SAMPLE_OVERSAMPLE_ALIAS_FILTERED_EXAMPLE, frames);

    let input = (0..frames)
        .map(|idx| if idx % 2 == 0 { 1.0_f32 } else { -1.0_f32 })
        .collect::<Vec<_>>();

    let mut top_level_output = vec![0.0_f32; frames];

    let mut proc_output = vec![0.0_f32; frames];

    process_interleaved(
        &mut top_level_instance,
        &input,
        &mut top_level_output,
        frames,
    )
    .expect("top-level oversampled process should succeed");

    process_interleaved(&mut proc_instance, &input, &mut proc_output, frames)
        .expect("proc oversampled process should succeed");

    for (actual, expected) in top_level_output.iter().zip(proc_output.iter()) {
        assert_near(*actual, *expected, 1e-6);
    }
}

#[test]

fn sample_oversample_keeps_proc_sine_pitch_constant() {
    let frames = 48_000;

    let sample_rate = 48_000.0_f32;

    let (mut base_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_1X_EXAMPLE, frames);

    let (mut os2_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_2X_EXAMPLE, frames);

    let (mut os4_instance, _, _) = compile_instance(SAMPLE_OVERSAMPLE_STD_SINE_4X_EXAMPLE, frames);

    let mut out_1x = vec![0.0_f32; frames];

    let mut out_2x = vec![0.0_f32; frames];

    let mut out_4x = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &[], &mut out_1x, frames)
        .expect("1x process should succeed");

    process_interleaved(&mut os2_instance, &[], &mut out_2x, frames)
        .expect("2x process should succeed");

    process_interleaved(&mut os4_instance, &[], &mut out_4x, frames)
        .expect("4x process should succeed");

    let f1 = estimate_positive_zero_cross_frequency(&out_1x, sample_rate);

    let f2 = estimate_positive_zero_cross_frequency(&out_2x, sample_rate);

    let f4 = estimate_positive_zero_cross_frequency(&out_4x, sample_rate);

    assert!(
        (f1 - 50.0).abs() < 1.5,
        "expected ~50 Hz at 1x, got {f1} Hz"
    );

    assert!(
        (f2 - f1).abs() < 1.5,
        "expected 2x oversampling pitch to match 1x, got f1={f1}, f2={f2}"
    );

    assert!(
        (f4 - f1).abs() < 1.5,
        "expected 4x oversampling pitch to match 1x, got f1={f1}, f4={f4}"
    );
}

#[test]

fn proc_sample_oversample_keeps_local_sine_pitch_constant() {
    let frames = 48_000;

    let sample_rate = 48_000.0_f32;

    let (mut base_instance, _, _) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_1X_EXAMPLE, frames);

    let (mut os8_instance, _, _) =
        compile_instance(PROC_SAMPLE_OVERSAMPLE_LOCAL_SINE_8X_EXAMPLE, frames);

    let mut out_1x = vec![0.0_f32; frames];

    let mut out_8x = vec![0.0_f32; frames];

    process_interleaved(&mut base_instance, &[], &mut out_1x, frames)
        .expect("1x process should succeed");

    process_interleaved(&mut os8_instance, &[], &mut out_8x, frames)
        .expect("8x process should succeed");

    let f1 = estimate_positive_zero_cross_frequency(&out_1x, sample_rate);

    let f8 = estimate_positive_zero_cross_frequency(&out_8x, sample_rate);

    assert!(
        (f1 - 50.0).abs() < 1.5,
        "expected ~50 Hz at 1x, got {f1} Hz"
    );

    assert!(
        (f8 - f1).abs() < 1.5,
        "expected proc sample 8 pitch to match 1x, got f1={f1}, f8={f8}"
    );
}

#[test]
#[ignore = "perf benchmark; run manually"]
fn sample_oversample_n4_performance_budget_benchmark() {
    const FRAMES: usize = 128;

    const WARMUP_ITERS: usize = 256;

    const TIMED_ITERS: usize = 4096;

    const TARGET_RATIO: f64 = 2.5;

    let (mut baseline, baseline_in_channels, baseline_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_PERF_BASELINE_EXAMPLE, FRAMES);

    let (mut oversampled, os_in_channels, os_out_channels) =
        compile_instance(SAMPLE_OVERSAMPLE_PERF_N4_EXAMPLE, FRAMES);

    assert_eq!(baseline_in_channels, 1);

    assert_eq!(baseline_out_channels, 1);

    assert_eq!(os_in_channels, 1);

    assert_eq!(os_out_channels, 1);

    let input = (0..FRAMES)
        .map(|idx| ((idx % 97) as f32 / 48.0) - 1.0)
        .collect::<Vec<_>>();

    let mut baseline_output = vec![0.0_f32; FRAMES];

    let mut os_output = vec![0.0_f32; FRAMES];

    let baseline_secs = benchmark_process_runtime(
        &mut baseline,
        &input,
        &mut baseline_output,
        FRAMES,
        WARMUP_ITERS,
        TIMED_ITERS,
    );

    let os_secs = benchmark_process_runtime(
        &mut oversampled,
        &input,
        &mut os_output,
        FRAMES,
        WARMUP_ITERS,
        TIMED_ITERS,
    );

    let ratio = os_secs / baseline_secs.max(f64::EPSILON);

    eprintln!(
        "oversample N=4 runtime ratio: {:.3}x (baseline={:.6}s, os4={:.6}s, frames={}, iters={})",
        ratio, baseline_secs, os_secs, FRAMES, TIMED_ITERS
    );

    if std::env::var("ONDA_ENFORCE_OVERSAMPLE_PERF_BUDGET")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        assert!(
            ratio <= TARGET_RATIO,
            "oversample N=4 runtime ratio {:.3}x exceeded target {:.3}x",
            ratio,
            TARGET_RATIO
        );
    }
}

#[test]

fn proc_array_input_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INPUT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_array_input_from_local_array_symbol_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_INPUT_VAR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.3, 1e-6);
    }
}

#[test]

fn proc_array_output_field_read_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_OUTPUT_INDEXED_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_array_param_constructor_args_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_PARAM_CTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.75, 1e-6);
    }
}

#[test]
fn proc_array_dynamic_index_reads_use_clamped_path() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_ARRAY_DYNAMIC_INDEX_CLAMP_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]
fn proc_array_constant_index_out_of_range_is_rejected() {
    let parsed = parse_program(PROC_ARRAY_CONSTANT_INDEX_OOB_REJECTED_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject out-of-range constant proc-array indexing"
    );
}

#[test]

fn proc_instance_array_indexed_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_field_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_FIELD_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_with_oversampled_callee_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_OVERSAMPLED_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert!(
            sample.is_finite() && *sample > 0.0 && *sample < 2.0,
            "expected finite oversampled dynamic dispatch output in (0,2), got {}",
            sample
        );
    }
}

#[test]

fn proc_instance_array_indexed_field_call_dynamic_index_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_FIELD_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_selects_slot_buffer_binding() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];

    let mut buf2 = vec![0.75_f32; frames];

    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");

    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2.as_mut_ptr().cast::<u8>(),
        buf2.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in out {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_alias_call_dynamic_index_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_DYNAMIC_INDEX_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_alias_out_read_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_INDEXED_ALIAS_OUT_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn nested_proc_instance_array_indexed_alias_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        NESTED_PROC_INSTANCE_ARRAY_INDEXED_ALIAS_CALL_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_len_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_uses_rebound_buffer_on_process_checked() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];

    let mut buf2_old = vec![0.75_f32; frames];

    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");

    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked with old buf2");

    let out_old = decode_planar_f32(&out_bytes);

    for sample in out_old {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked with new buf2");

    let out_new = decode_planar_f32(&out_bytes);

    for sample in out_new {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]
fn checked_end_segment_uses_rebound_proc_array_buffer() {
    let frames = 4;
    let segment_frames = 2;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];
    let mut buf2_old = vec![0.75_f32; frames];
    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");
    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind old buf2");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked_segment(
        &mut instance,
        0,
        segment_frames,
        PROCESS_BEGIN_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process begin segment with old buf2");

    let mut buf2_new = vec![0.5_f32; frames];
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind new buf2 before end segment");

    out_bytes.fill(0);
    process_checked_segment(
        &mut instance,
        segment_frames,
        segment_frames,
        PROCESS_END_BLOCK,
        onda_runtime::ExecutionOutput::none(),
    )
    .expect("process end segment after rebind");
    let out = decode_planar_f32(&out_bytes);
    assert_near(out[0], 0.0, 1e-6);
    assert_near(out[1], 0.0, 1e-6);
    assert_near(out[2], 0.5, 1e-6);
    assert_near(out[3], 0.5, 1e-6);
}

#[test]

fn proc_instance_array_indexed_call_dynamic_index_uses_validated_rebound_buffer_on_process_unchecked(
) {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];

    let mut buf2_old = vec![0.75_f32; frames];

    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");

    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked with old buf2");

    let out_seed = decode_planar_f32(&out_bytes);

    for sample in out_seed {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];

    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    validate_buffers(&mut instance).expect("validate buffers after rebind");

    validate_outputs(&mut instance).expect("validate outputs");

    // MIR buffer arguments are transient symbolic resources. Once validation publishes the
    // rebound host table, unchecked processing observes that current table rather than a
    // pointer-bearing processor cache.
    unsafe {
        process_unchecked(&mut instance, onda_runtime::ExecutionOutput::none())
            .expect("unchecked process after rebind");
    }

    let out_unchecked = decode_planar_f32(&out_bytes);

    for sample in out_unchecked {
        assert_near(sample, 0.5, 1e-6);
    }

    process_checked(&mut instance, frames, onda_runtime::ExecutionOutput::none())
        .expect("process checked uses current binding");

    let out_checked = decode_planar_f32(&out_bytes);

    for sample in out_checked {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]
fn prepare_unchecked_process_uses_current_proc_array_buffer_binding() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_INDEXED_CALL_DYNAMIC_INDEX_BUFFER_BINDING_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);
    assert_eq!(instance.buffer_count(), 2);

    let mut buf1 = vec![0.25_f32; frames];
    let mut buf2_old = vec![0.75_f32; frames];
    let buf1_idx = instance.buffer_index("buf1").expect("buf1 index");
    let buf2_idx = instance.buffer_index("buf2").expect("buf2 index");

    bind_buffer(
        &mut instance,
        buf1_idx,
        buf1.as_mut_ptr().cast::<u8>(),
        buf1.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf1");
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_old.as_mut_ptr().cast::<u8>(),
        buf2_old.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 old");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];
    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    prepare_unchecked_process(&mut instance).expect("prepare unchecked with old buf2");
    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            frames,
            PROCESS_FULL_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("unchecked process with old buf2");
    }
    let out_old = decode_planar_f32(&out_bytes);
    for sample in out_old {
        assert_near(sample, 0.75, 1e-6);
    }

    let mut buf2_new = vec![0.5_f32; frames];
    bind_buffer(
        &mut instance,
        buf2_idx,
        buf2_new.as_mut_ptr().cast::<u8>(),
        buf2_new.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buf2 new");

    prepare_unchecked_process(&mut instance).expect("prepare unchecked after rebind");
    unsafe {
        process_unchecked_segment(
            &mut instance,
            0,
            frames,
            PROCESS_FULL_BLOCK,
            onda_runtime::ExecutionOutput::none(),
        )
        .expect("unchecked process with refreshed refs");
    }
    let out_new = decode_planar_f32(&out_bytes);
    for sample in out_new {
        assert_near(sample, 0.5, 1e-6);
    }
}

#[test]

fn nested_proc_instance_array_indexed_call_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INSTANCE_ARRAY_INDEXED_CALL_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn deep_nested_proc_instance_array_dynamic_index_chain_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEEP_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 50.5, 1e-6);
    }
}

#[test]

fn deeper_nested_proc_instance_array_dynamic_index_chain_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEEPER_NESTED_PROC_INSTANCE_ARRAY_DYNAMIC_INDEX_CHAIN_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 55.5, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_INSTANCE_ARRAY_BROADCAST_CTOR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.5, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_array_literal_arg_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_LITERAL_ARG_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.8, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_array_symbol_arg_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        PROC_INSTANCE_ARRAY_BROADCAST_CTOR_ARRAY_SYMBOL_ARG_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.8, 1e-6);
    }
}

#[test]

fn untyped_init_array_first_element_type_is_enforced() {
    let parsed = parse_program(UNTYPED_INIT_ARRAY_FIRST_ELEMENT_TYPE_MISMATCH_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject untyped init array literals whose later elements are not assignable to the first element type"

    );
}

#[test]

fn top_level_proc_instance_array_broadcast_ctor_mixed_buffer_array_arg_analyzes() {
    let parsed = parse_program(PROC_INSTANCE_ARRAY_BROADCAST_CTOR_MIXED_BUFFER_ARRAY_ARG_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_ok(),

        "semantic analysis should allow broadcast processor-array ctor with scalar and per-slot buffer arguments"

    );
}

#[test]

fn nested_proc_init_untyped_array_symbol_arg_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INIT_UNTYPED_ARRAY_SYMBOL_ARG_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_const_expr_size_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn nested_proc_instance_array_const_expr_size_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NESTED_PROC_INSTANCE_ARRAY_CONST_EXPR_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn top_level_proc_instance_array_initializer_arity_is_rejected() {
    let parsed = parse_program(TOP_LEVEL_PROC_INSTANCE_ARRAY_INIT_ARITY_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject mismatched constructor count for top-level processor arrays"

    );
}

#[test]

fn proc_nested_state_persists_across_samples() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_NESTED_STATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.25, 1e-6);

    assert_near(output[1], 0.5, 1e-6);

    assert_near(output[2], 0.75, 1e-6);

    assert_near(output[3], 1.0, 1e-6);
}

#[test]

fn proc_deep_nested_state_persists_across_samples() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DEEP_NESTED_STATE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.5, 1e-6);

    assert_near(output[1], 1.0, 1e-6);

    assert_near(output[2], 1.5, 1e-6);

    assert_near(output[3], 2.0, 1e-6);
}

#[test]

fn proc_deep_nested_buffer_binding_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(PROC_DEEP_NESTED_BUFFER_BIND_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    assert_eq!(instance.buffer_count(), 1);

    let mut buf = vec![3.0_f32, 4.0];

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

    let expected = [3.0_f32, 4.0, 4.0, 4.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn top_level_init_block_is_optional() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_OPTIONAL_INIT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn struct_method_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(STRUCT_METHOD_EXAMPLE, frames);

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

fn struct_method_data_write_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_METHOD_DATA_WRITE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn struct_method_requires_self_param() {
    let parsed =
        parse_program(STRUCT_METHOD_SELF_REQUIRED_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject struct method without self first parameter"
    );
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_gain_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        GAIN,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    set_param_f32(&mut instance, "gain", 0.5);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32).collect();

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, out) in output.iter().enumerate() {
        assert_near(*out, input[idx] * 0.5, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_sine_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        SINE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let input = Vec::<f32>::new();

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let phase_step = 440.0_f32 * 6.2831855_f32 / 48_000.0_f32;

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32 * phase_step).sin();

        assert_near(*sample, expected, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_one_pole_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        ONE_POLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32; frames];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 0.1, 1e-6);

    assert!(output[frames - 1] > output[0]);

    assert!(output[frames - 1] < 1.0);
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_if_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        IF_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.25, 1e-6);
    }

    set_param_f32(&mut instance, "gate", 0.0);

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, -0.25, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]
fn scoped_branch_and_loop_locals_do_not_reuse_a_later_bool_binding() {
    let source = r#"
outs:
  out1

sample:
  if in1 > 0.0:
    temp = 0.0
  for i in 0..1:
    temp = f32(i)
  temp = true
  if temp:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, -1.0, 0.5, -0.5];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_eq!(output, vec![1.0; frames]);
}

#[cfg(feature = "llvm-orc")]
#[test]
fn comparison_literal_uses_the_concrete_f32_peer_width_at_runtime() {
    let source = r#"
outs:
  out1

sample:
  x: f32 = f32(16777216.0)
  if x == 16777217.0:
    out1 = 1.0
  else:
    out1 = 0.0
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");
    assert_eq!(output, vec![1.0; frames]);
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_for_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        FOR_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.6, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_def_call_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DEF_CALL_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_def_return_exits_early() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        DEF_EARLY_RETURN_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_struct_compiles_and_runs() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        STRUCT_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[cfg(feature = "llvm-orc")]
#[test]

fn explicit_mir_orc_struct_reserved_method_names_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance_with_options(
        RESERVED_METHOD_NAMES_EXAMPLE,
        frames,
        CompileOptions {
            sample_rate: 48_000.0,

            block_size: frames,

            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn analyze_supports_typed_init_generic_struct_ctor_decl() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data::Data<f32> = std::data::Data()

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(result.is_ok(), "semantic analysis should succeed");
}

#[test]

fn analyze_supports_typed_init_generic_struct_default_ctor_decl() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data::Data<f32>

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(result.is_ok(), "semantic analysis should succeed");
}

#[test]

fn analyze_rejects_typed_init_generic_struct_default_ctor_decl_without_type_args() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data::Data

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject generic struct ctor without type args when inference cannot resolve"

    );
}

#[test]

fn analyze_rejects_typed_init_namespace_instantiated_struct_default_ctor_decl_without_type_args() {
    let src = r#"

import std/data

outs { out1 }

init {

  line: std::data<SR, 1>::Data

}

sample {

  out1 = line.read(0)

}

"#;

    let parsed = parse_program(src).expect("parser should accept typed generic init declaration");

    let result = analyze(parsed);

    assert!(

        result.is_err(),

        "semantic analysis should reject generic struct ctor without type args when inference cannot resolve"

    );
}
