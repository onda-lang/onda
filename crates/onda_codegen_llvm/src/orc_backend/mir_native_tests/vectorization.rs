use super::*;

#[test]
fn independent_stateful_processors_retain_simd_with_fast_math() {
    let source = r#"import std/filter
import std/osc
import std/pitch

const VoiceCount = 5
const Intervals: i32[VoiceCount] = [0, 7, 12, 19, 26]

proc Voice:
  params:
    note = 36.0
    drift_rate = 0.03
    drift_depth = 0.004
    brightness = 1800.0

  init:
    saw = std::osc::Saw(freq = std::pitch::note_to_hz(note), amp = 0.48)
    sine = std::osc::Sine(freq = std::pitch::note_to_hz(note) * 0.5, amp = 0.52)
    drift = std::osc::Sine(freq = drift_rate)
    tone = std::filter::OnePole(cutoff = brightness)

  sample:
    bend = 1.0 + drift() * drift_depth
    freq = std::pitch::note_to_hz(note) * bend
    saw.freq = freq
    sine.freq = freq * 0.5
    tone.cutoff = brightness
    out1 = tone(saw() + sine())

params:
  root_note = 33.0
  motion = 0.38
  brightness = 1900.0
init:
  voices: Voice[VoiceCount] = Voice()
sample:
  total = 0.0
  for i in 0..VoiceCount:
    voices[i].note = root_note + Intervals[i]
    voices[i].drift_depth = 0.0004 + motion * (0.0012 + i * 0.00035)
    voices[i].brightness = brightness * (0.72 + i * 0.11)
    total += voices[i]()
  out1 = total
"#;
    let (_, mir) = source_program(source, 128);
    let mut target = crate::TargetConfig::for_triple("x86_64-unknown-linux-gnu");
    target.cpu = crate::TargetCpu::Explicit("raptorlake".into());
    target.features = Some("+avx,+avx2".into());
    let ir = lower_mir_to_target_llvm_ir(
        &mir,
        &MirTargetOptions {
            fast_math: true,
            target,
        },
    )
    .unwrap();
    let process = ir
        .split("@onda_process(")
        .nth(1)
        .unwrap()
        .split("\n}")
        .next()
        .unwrap();
    // Early vector-library mappings used to scalarize the whole voice loop,
    // even though its math inputs are segment-invariant. Preserve this case
    // when integrating a portable vector math implementation.
    assert!(
        process
            .lines()
            .any(|line| line.contains("call") && line.contains("llvm.masked.gather")),
        "vector math must preserve the voice loop's parallel table reads"
    );
}
