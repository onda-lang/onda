---
title: Standard library
description: Generated reference for Onda's built-in standard-library modules, functions, structs, and processors.
permalink: /docs/stdlib/
section: reference
eyebrow: Language reference
---

# Onda standard library

This page is generated from the standard library embedded in the compiler. Run `scripts/update_stdlib_docs.sh` on Unix or `scripts/update_stdlib_docs.ps1` on Windows after changing `stdlib/`; `npm run docs:stdlib` is the equivalent package command, and CI verifies that the checked-in reference is current. Declarations whose names begin with `_` are implementation helpers and are omitted.

`std/prelude` is imported automatically. It loads `std/math`, `std/lookup`, and `std/random`, including the unqualified forwarding functions from the first two modules. Import the other modules explicitly before using their qualified APIs.

## Modules

| Module | Provides |
| --- | --- |
| [`std/math`](#stdmath) | `ampdb`, `clamp`, `cpsmidi`, `cpsoct`, `cubic_interp`, `dbamp`, `expexp`, `explin`, `fract`, `inverse_lerp`, `lerp`, `linexp`, `linlin`, `map`, `midicps`, `midiratio`, `octcps`, `ratiomidi`, `sign`, `smoothstep`, `wrap` |
| [`std/complex`](#stdcomplex) | `Complex` |
| [`std/osc`](#stdosc) | `KSine`, `Phasor`, `Pulse`, `Saw`, `SawDown`, `Sine`, `Square`, `Triangle`, `poly_blep` |
| [`std/filter`](#stdfilter) | `DCBlock`, `OnePole`, `Resonator`, `Svf`, `mode` |
| [`std/env`](#stdenv) | `ADSR`, `AR`, `ASR`, `DecayEnv`, `decay_coefficient`, `stage` |
| [`std/reverb`](#stdreverb) | `Schroeder` |
| [`std/pitch_shift`](#stdpitch_shift) | `BufferSize`, `DualWindow` |
| [`std/noise`](#stdnoise) | `Brown`, `Pink`, `White` |
| [`std/levels`](#stdlevels) | `DB_PER_NAT`, `DB_TO_GAIN_SCALE`, `HALF_PI`, `MIN_FLOAT`, `db_to_gain`, `gain_to_db`, `pan_3db`, `pan_linear` |
| [`std/mix`](#stdmix) | `ConstantSum`, `Crossfade`, `MonoToStereo`, `StereoToMono`, `chans` |
| [`std/gain`](#stdgain) | `Constant`, `Db`, `Smoothed`, `SmoothedDb` |
| [`std/pitch`](#stdpitch) | `A4_HZ`, `INV_LN_2_OVER_12`, `LN_2_OVER_12`, `MIDI_A4`, `MIN_FLOAT`, `hz_to_note`, `note_to_hz`, `ratio_between` |
| [`std/smoothing`](#stdsmoothing) | `Lag`, `LagUD`, `Slew`, `time_coefficient` |
| [`std/dynamics`](#stddynamics) | `Compressor`, `Gate`, `Limiter`, `PeakFollower`, `RmsFollower`, `soft_knee_reduction_db` |
| [`std/delay`](#stddelay) | `Cubic`, `Delay`, `Feedback`, `Integer`, `Linear`, `Smooth` |
| [`std/sample`](#stdsample) | `Player` |
| [`std/data`](#stddata) | `Data` |
| [`std/fft`](#stdfft) | `Blackman`, `FFT`, `Hamming`, `Hann`, `RealFFT`, `RealIFFT`, `Rectangular`, `STFT` |
| [`std/convolution`](#stdconvolution) | `BlockConvolver`, `DirectTaps`, `FinalStageCapacity`, `HeadFFTSize`, `HeadStageCapacity`, `HeadStageEnd`, `HopSize`, `LargeFFTSize`, `LargeStageCapacity`, `LargeStageEnd`, `MidFFTSize`, `MidStageCapacity`, `MidStageEnd`, `TailStart`, `TimeDomainConvolver`, `ZeroLatencyConvolver`, `impulse_window_count`, `impulse_window_end`, `stage_window_count` |
| [`std/lookup`](#stdlookup) | `read`, `readC`, `readCW`, `readL`, `readLW`, `write` |
| [`std/random`](#stdrandom) | `RNG_INC`, `RNG_MASK`, `RNG_MULT`, `Rng`, `seed_state`, `step_state` |
| [`std/prelude`](#stdprelude) | Automatically imports `std/math`, `std/lookup`, `std/random` |

## `std/math`

```onda
import std/math
```

### Unqualified functions

```onda
def clamp<T>(x: T, lo: T, hi: T) -> T:
def lerp<T>(a: T, b: T, t: T) -> T:
def inverse_lerp(a, b, x):
def map(in_lo, in_hi, out_lo, out_hi, x):
def sign(x):
def fract(x):
def wrap(x, lo, hi):
def cubic_interp(y0, y1, y2, y3, t):
def smoothstep(edge0, edge1, x):
def linlin(x, in_lo, in_hi, out_lo, out_hi):
def linexp(x, in_lo, in_hi, out_lo, out_hi):
def explin(x, in_lo, in_hi, out_lo, out_hi):
def expexp(x, in_lo, in_hi, out_lo, out_hi):
def dbamp(db):
def ampdb(amp):
def midicps(note):
def cpsmidi(freq):
def midiratio(semitones):
def ratiomidi(ratio):
def octcps(oct):
def cpsoct(freq):
```

Namespace: `std::math`.

### Functions

```onda
def clamp<T>(x: T, lo: T, hi: T) -> T:
def lerp<T>(a: T, b: T, t: T) -> T:
def inverse_lerp(a, b, x):
def map(in_lo, in_hi, out_lo, out_hi, x):
def sign(x):
def fract(x):
def wrap(x, lo, hi):
def cubic_interp(y0, y1, y2, y3, t):
def smoothstep(edge0, edge1, x):
def linlin(x, in_lo, in_hi, out_lo, out_hi):
def linexp(x, in_lo, in_hi, out_lo, out_hi):
def explin(x, in_lo, in_hi, out_lo, out_hi):
def expexp(x, in_lo, in_hi, out_lo, out_hi):
def dbamp(db):
def ampdb(amp):
def midicps(note):
def cpsmidi(freq):
def midiratio(semitones):
def ratiomidi(ratio):
def octcps(oct):
def cpsoct(freq):
```


## `std/complex`

```onda
import std/complex
```

Namespace: `std::complex`.

### Struct `Complex<T>`

```onda
struct Complex<T>:
  re: T = 0.0
  im: T = 0.0
  def real(self):
  def imag(self):
  def set(self, re, im):
  def clear(self):
  def copy(self, other: Complex):
  def set_polar(self, magnitude, phase):
  def add_assign(self, other: Complex):
  def add_parts(self, re, im):
  def sub_assign(self, other: Complex):
  def sub_parts(self, re, im):
  def mul_assign(self, other: Complex):
  def mul_parts(self, re, im):
  def scale_assign(self, gain):
  def conjugate(self):
  def power(self):
  def magnitude(self):
  def phase(self):
```


## `std/osc`

```onda
import std/osc
```

Namespace: `std::osc`.

### Functions

```onda
def poly_blep<T>(t: T, dt: T) -> T:
```

### Processor `Phasor<T>`

```onda
proc Phasor<T>:
  outs<T> 1
  params:
    freq: T = 1.0 => update_freq
  events:
    reset(phase_cycles: T = 0.0):
```

### Processor `Sine<T>`

```onda
proc Sine<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0
    phase_offset: T = 0.0
  events:
    reset(phase_cycles: T = 0.0):
```

### Processor `KSine<T>`

```onda
proc KSine<T>:
  kouts<T> 1
  params:
    freq: T = 1.0
    amp: T = 1.0
    phase_offset: T = 0.0
  events:
    reset(phase_cycles: T = 0.0):
```

### Processor `Saw<T>`

```onda
proc Saw<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0
  events:
    reset(phase_cycles: T = 0.0):
```

### Processor `SawDown<T>`

```onda
proc SawDown<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0
  events:
    reset(phase_cycles: T = 0.0):
```

### Processor `Pulse<T>`

```onda
proc Pulse<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    width: T = 0.5 {0.001, 0.999} => update_width
    amp: T = 1.0
  events:
    reset(phase_cycles: T = 0.0):
```

### Processor `Square<T>`

```onda
proc Square<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0 => update_amp
  events:
    reset(phase_cycles: T = 0.0):
```

### Processor `Triangle<T>`

```onda
proc Triangle<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0
  events:
    reset(phase_cycles: T = 0.0):
```


## `std/filter`

```onda
import std/filter
```

Namespace: `std::filter`.

### Namespace `mode`

#### Constants

```onda
const ONE_POLE_LOWPASS = 0
const ONE_POLE_HIGHPASS = 1
const SVF_LOWPASS = 0
const SVF_HIGHPASS = 1
const SVF_BANDPASS = 2
const SVF_NOTCH = 3
const SVF_PEAK = 4
const SVF_ALLPASS = 5
```

### Processor `OnePole<T>`

```onda
proc OnePole<T>:
  ins<T> 1
  outs<T> 1
  params:
    cutoff: T = 1000.0 {0.0, T(SR * 0.48)} => update_cutoff
    mode: i32 = mode::ONE_POLE_LOWPASS {mode::ONE_POLE_LOWPASS, mode::ONE_POLE_HIGHPASS}
```

### Processor `DCBlock<T>`

```onda
proc DCBlock<T>:
  ins<T> 1
  outs<T> 1
```

### Processor `Resonator<T>`

```onda
proc Resonator<T>:
  ins<T> 1
  outs<T> 1
  params:
    freq: T = 1000.0 {1.0, T(SR * 0.48)} => update_coefficients
    bandwidth: T = 120.0 {1.0, T(SR * 0.48)} => update_coefficients
```

### Processor `Svf<T>`

```onda
proc Svf<T>:
  ins<T> 1
  outs<T> 1
  params:
    private cutoff: T = 1000.0 {0.0, T(SR * 0.48)}
    private q: T = 0.707107
    mode: i32 = mode::SVF_LOWPASS {mode::SVF_LOWPASS, mode::SVF_ALLPASS}
  events:
    update_coeffs(cutoff_v: T, q_v: T):
```


## `std/env`

```onda
import std/env
```

Namespace: `std::env`.

### Functions

```onda
def decay_coefficient<T>(time_s: T):
```

### Processor `DecayEnv<T>`

```onda
proc DecayEnv<T>:
  outs<T> 1
  params:
    decay_s: T = 0.2 => update_decay
    end_level: T = 0.00001 {0.000000001, 1.0}
    trigger: T = 0.0 {0.0, 1.0}
  delegates:
    finished()
  events:
    start(level: T = 1.0):
    reset():
```

### Processor `AR<T>`

```onda
proc AR<T>:
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_steps
    release_s: T = 0.1 => update_steps
    trigger: T = 0.0 {0.0, 1.0}
  delegates:
    finished()
  events:
    start():
    reset():
```

### Processor `ASR<T>`

```onda
proc ASR<T>:
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_shape
    sustain: T = 1.0 {0.0, 1.0} => update_shape
    release_s: T = 0.1 => update_shape
    gate: T = 0.0 {0.0, 1.0}
  delegates:
    finished()
  events:
    start():
    release():
    reset():
```

### Namespace `stage`

#### Constants

```onda
const IDLE = 0
const ATTACK = 1
const DECAY = 2
const SUSTAIN = 3
const RELEASE = 4
```

### Processor `ADSR<T>`

```onda
proc ADSR<T>:
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_shape
    decay_s: T = 0.1 => update_shape
    sustain: T = 0.7 {0.0, 1.0} => update_shape
    release_s: T = 0.2 => update_shape
    gate: T = 0.0 {0.0, 1.0}
  delegates:
    finished()
  events:
    start():
    release():
    reset():
```


## `std/reverb`

```onda
import std/reverb
```

Namespace: `std::reverb`.

### Namespace `Schroeder<CombCapacity = 8192, AllpassCapacity = 4096>`

#### Constants

```onda
const CombLines = 8
const AllpassLines = 4
const ReferenceRate = 48000
const CombTuning: i32[CombLines] = [1116, 1188, 1277, 1356, 1139, 1211, 1300, 1379]
const AllpassTuning: i32[AllpassLines] = [556, 441, 579, 464]
```

#### Processor `Reverb<T>`

```onda
proc Reverb<T>:
  ins<T> 2
  outs<T> 2
  params:
    room_size: T = 0.82 {0.0, 1.0}
    damping: T = 0.34 {0.0, 0.98}
    width: T = 0.92 {0.0, 1.0}
    mix: T = 1.0 {0.0, 1.0}
```


## `std/pitch_shift`

```onda
import std/pitch_shift
```

Namespace: `std::pitch_shift`.

### Constants

```onda
const BufferSize = i32(SR / 2)
```

### Processor `DualWindow<T>`

```onda
proc DualWindow<T>:
  ins<T> 1
  outs<T> 1
  params:
    semitones: T = 12.0
    window_s: T = 0.09 {T(16.0) / SR, T(BufferSize - 2) / SR}
```


## `std/noise`

```onda
import std/noise
```

Namespace: `std::noise`.

### Processor `White<T>`

```onda
proc White<T>:
  outs<T> 1
  params:
    amp: T = 1.0
  events:
    seed(value: i64):
```

### Processor `Pink<T>`

```onda
proc Pink<T>:
  outs<T> 1
  params:
    amp: T = 1.0
  events:
    seed(value: i64):
```

### Processor `Brown<T>`

```onda
proc Brown<T>:
  outs<T> 1
  params:
    amp: T = 1.0
  events:
    seed(value: i64):
```


## `std/levels`

```onda
import std/levels
```

Namespace: `std::levels`.

### Constants

```onda
const HALF_PI: f64 = PI / 2.0
const DB_TO_GAIN_SCALE: f64 = 0.11512925464970229
const DB_PER_NAT: f64 = 8.685889638065037
const MIN_FLOAT: f64 = 0.00000000000000000001
```

### Functions

```onda
def db_to_gain<T>(x: T):
def gain_to_db<T>(x: T):
def pan_linear<T>(x: T):
def pan_3db<T>(x: T):
```


## `std/mix`

```onda
import std/mix
```

Namespace: `std::mix`.

### Namespace `chans<N = 2>`

#### Processor `Broadcast<T>`

```onda
proc Broadcast<T>:
  ins<T> 1
  outs<T> N
```

#### Processor `Sum<T>`

```onda
proc Sum<T>:
  ins<T> N
  outs<T> 1
```

#### Processor `Average<T>`

```onda
proc Average<T>:
  ins<T> N
  outs<T> 1
```

#### Processor `Crossfade<T>`

```onda
proc Crossfade<T>:
  ins:
    a: T[N]
    b: T[N]
  outs:
    out: T[N]
  params:
    mix: T = 0.5 {0.0, 1.0}
```

### Processor `MonoToStereo<T>`

```onda
proc MonoToStereo<T>:
  ins<T> 1
  outs<T> 2
```

### Processor `StereoToMono<T>`

```onda
proc StereoToMono<T>:
  ins<T> 2
  outs<T> 1
  params:
    norm: T = 0.5
```

### Processor `ConstantSum<T>`

```onda
proc ConstantSum<T>:
  ins<T> 2
  outs<T> 1
  params:
    gain1: T = 1.0
    gain2: T = 1.0
```

### Processor `Crossfade<T>`

```onda
proc Crossfade<T>:
  ins<T> 2
  outs<T> 1
  params:
    mix: T = 0.5 {0.0, 1.0}
```


## `std/gain`

```onda
import std/gain
```

Namespace: `std::gain`.

### Processor `Constant<T>`

```onda
proc Constant<T>:
  ins<T> 1
  outs<T> 1
  params:
    gain: T = 1.0
```

### Processor `Db<T>`

```onda
proc Db<T>:
  ins<T> 1
  outs<T> 1
  params:
    db: T = 0.0 => update_gain
```

### Processor `Smoothed<T>`

```onda
proc Smoothed<T>:
  ins<T> 1
  outs<T> 1
  params:
    gain: T = 1.0
    time_s: T = 0.05 => update_time
```

### Processor `SmoothedDb<T>`

```onda
proc SmoothedDb<T>:
  ins<T> 1
  outs<T> 1
  params:
    db: T = 0.0 => update_gain
    time_s: T = 0.05 => update_time
```


## `std/pitch`

```onda
import std/pitch
```

Namespace: `std::pitch`.

### Constants

```onda
const A4_HZ: f64 = 440.0
const MIDI_A4: f64 = 69.0
const LN_2_OVER_12: f64 = 0.05776226504666211
const INV_LN_2_OVER_12: f64 = 17.31234049066756
const MIN_FLOAT: f64 = 0.00000000000000000001
```

### Functions

```onda
def note_to_hz<T>(note: T):
def note_to_hz<T>(note: T, a4_hz: T):
def hz_to_note<T>(hz: T):
def hz_to_note<T>(hz: T, a4_hz: T):
def ratio_between<T>(source_note: T, target_note: T):
```


## `std/smoothing`

```onda
import std/smoothing
```

Namespace: `std::smoothing`.

### Functions

```onda
def time_coefficient<T>(time_s: T):
```

### Processor `Lag<T>`

```onda
proc Lag<T>:
  ins<T> 1
  outs<T> 1
  params:
    time_s: T = 0.05 => update_time
```

### Processor `LagUD<T>`

```onda
proc LagUD<T>:
  ins<T> 1
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_attack
    release_s: T = 0.1 => update_release
```

### Processor `Slew<T>`

```onda
proc Slew<T>:
  ins<T> 1
  outs<T> 1
  params:
    rise_per_s: T = 1.0 => update_rise
    fall_per_s: T = 1.0 => update_fall
```


## `std/dynamics`

```onda
import std/dynamics
```

Namespace: `std::dynamics`.

### Functions

```onda
def soft_knee_reduction_db<T>(level_db: T, threshold_db: T, ratio: T, knee_db: T):
```

### Processor `PeakFollower<T>`

```onda
proc PeakFollower<T>:
  ins<T> 1
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_attack
    release_s: T = 0.1 => update_release
  events:
    reset():
```

### Processor `RmsFollower<T>`

```onda
proc RmsFollower<T>:
  ins<T> 1
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_attack
    release_s: T = 0.1 => update_release
  events:
    reset():
```

### Processor `Compressor<T>`

```onda
proc Compressor<T>:
  ins<T> 2
  outs<T> 2
  params:
    threshold_db: T = -18.0
    ratio: T = 4.0
    attack_s: T = 0.01 => update_attack
    release_s: T = 0.1 => update_release
    knee_db: T = 6.0
    makeup_db: T = 0.0
  events:
    reset():
```

### Processor `Limiter<T>`

```onda
proc Limiter<T>:
  ins<T> 2
  outs<T> 2
  params:
    ceiling_db: T = -0.3 => update_ceiling
    release_s: T = 0.05 => update_release
  events:
    reset():
```

### Processor `Gate<T>`

```onda
proc Gate<T>:
  ins<T> 2
  outs<T> 2
  params:
    threshold_db: T = -48.0 => update_threshold
    attack_s: T = 0.002 => update_attack
    release_s: T = 0.08 => update_release
  events:
    reset():
```


## `std/delay`

```onda
import std/delay
```

Namespace: `std::delay<Capacity = SR * 2>`.

### Processor `Integer<T>`

```onda
proc Integer<T>:
  ins<T> 1
  outs<T> 1
  params:
    delay_samples = 1 {0, Capacity - 1}
  events:
    reset():
```

### Processor `Linear<T>`

```onda
proc Linear<T>:
  ins<T> 1
  outs<T> 1
  params:
    delay_samples: T = 1.0 {0.0, Capacity - 2}
  events:
    reset():
```

### Processor `Cubic<T>`

```onda
proc Cubic<T>:
  ins<T> 1
  outs<T> 1
  params:
    delay_samples: T = 1.0 {1.0, Capacity - 3}
  events:
    reset():
```

### Processor `Smooth<T>`

```onda
proc Smooth<T>:
  ins<T> 1
  outs<T> 1
  params:
    delay_samples: T = 1.0 {0.0, Capacity - 2}
    transition_s: T = 0.02 => update_transition
  events:
    reset():
```

### Processor `Feedback<T>`

```onda
proc Feedback<T>:
  ins<T> 1
  outs<T> 1
  params:
    delay_s: T = 0.1 {T(1.0) / SR, T(Capacity - 2) / SR}
    feedback: T = 0.0
    mix: T = 1.0 {0.0, 1.0}
  events:
    reset():
```

### Processor `Delay<T>`

```onda
proc Delay<T>:
  ins<T> 1
  outs<T> 1
  params:
    delay_s: T = 0.1 {T(1.0) / SR, T(Capacity - 2) / SR}
    feedback: T = 0.0
    mix: T = 1.0 {0.0, 1.0}
  events:
    reset():
```


## `std/sample`

```onda
import std/sample
```

Namespace: `std::sample<Channels = 2>`.

### Processor `Player<T>`

```onda
proc Player<T>:
  outs<T> Channels
  params:
    speed: T = 1.0
    looping: bool = false
  buffers:
    clip: T[]
  delegates:
    finished()
    looped()
  events:
    play(start_frame: T = 0.0):
    stop():
    seek(frame: T):
    reset():
```


## `std/data`

```onda
import std/data
```

Namespace: `std::data<S = SR, C = 1>`.

### Struct `Data<T>`

```onda
struct Data<T>:
  storage: T[S * C]
  def len(self):
  def frames(self):
  def chans(self):
  def read(self, frame_i: i32, ch_i: i32 = 0):
  def write(self, frame_i: i32, value: T, ch_i: i32 = 0):
  def readL(self, pos, ch_i: i32 = 0):
  def readC(self, pos, ch_i: i32 = 0):
```


## `std/fft`

```onda
import std/fft
```

Namespace: `std::fft<N = 256>`.

### Constants

```onda
const Hann: f64[N] = _hann_window()
const Rectangular: f64[N] = _rectangular_window()
const Hamming: f64[N] = _hamming_window()
const Blackman: f64[N] = _blackman_window()
```

### Struct `FFT<T>`

```onda
struct FFT<T>:
  bins: std::complex::Complex<T>[N]
  def size(self):
  def real_bin_count(self):
  def clear(self):
  def set_bin(self, i: i32, re, im):
  def load_real(self, input: T[]):
  def load_complex(self, real: T[], imag: T[]):
  def store_real(self, output: T[]):
  def store_imag(self, output: T[]):
  def store_magnitude(self, output: T[]):
  def store_power(self, output: T[]):
  def store_phase(self, output: T[]):
  def store_real_packed(self, output: T[]):
  def load_real_packed(self, input: T[]):
  def store_real_spectrum_magnitude(self, output: T[]):
  def store_real_spectrum_power(self, output: T[]):
  def store_real_spectrum_phase(self, output: T[]):
  def real(self, i: i32):
  def imag(self, i: i32):
  def power(self, i: i32):
  def magnitude(self, i: i32):
  def phase(self, i: i32):
  def forward_real(self, input: T[]):
  def forward_real_packed(self, input: T[], output: T[]):
  def forward_complex(self, real: T[], imag: T[]):
  def forward_real_magnitude(self, input: T[], output: T[]):
  def forward_real_power(self, input: T[], output: T[]):
  def forward_real_phase(self, input: T[], output: T[]):
  def forward(self):
  def inverse(self):
  def inverse_real_packed(self, input: T[], output: T[]):
```

### Struct `STFT<T>`

```onda
struct STFT<T>:
  fft: FFT<T>
  window_kind: f32 = _WindowHann
  def size(self):
  def real_bin_count(self):
  def set_hann(self):
  def set_rectangular(self):
  def set_hamming(self):
  def set_blackman(self):
  def window_value(self, i: i32):
  def store_window(self, output: T[]):
  def clear(self):
  def real(self, i: i32):
  def imag(self, i: i32):
  def power(self, i: i32):
  def magnitude(self, i: i32):
  def phase(self, i: i32):
  def store_real_packed(self, output: T[]):
  def store_real_spectrum_magnitude(self, output: T[]):
  def store_real_spectrum_power(self, output: T[]):
  def store_real_spectrum_phase(self, output: T[]):
  def forward_real(self, input: T[]):
  def forward_real_packed(self, input: T[], output: T[]):
  def forward_real_magnitude(self, input: T[], output: T[]):
  def forward_real_power(self, input: T[], output: T[]):
  def forward_real_phase(self, input: T[], output: T[]):
```

### Struct `RealFFT<T>`

```onda
struct RealFFT<T>:
  fft: FFT<T>
  input: T[N]
  window_kind: f32 = _WindowHann
  write: i32 = 0
  filled: i32 = 0
  since_hop: i32 = 0
  ready: bool = false
  def size(self):
  def real_bin_count(self):
  def hop_size(self):
  def set_rectangular(self):
  def set_hann(self):
  def clear(self):
  def push(self, x: T):
  def is_ready(self):
  def real(self, i: i32):
  def imag(self, i: i32):
  def power(self, i: i32):
  def magnitude(self, i: i32):
  def phase(self, i: i32):
  def packed_value(self, i: i32):
  def store_real_packed(self, output: T[]):
```

### Struct `RealIFFT<T>`

```onda
struct RealIFFT<T>:
  fft: FFT<T>
  output: T[N]
  norm: T[N]
  window_kind: f32 = _WindowHann
  frame: i32 = 0
  pending: i32 = 0
  overlap_frames: i32 = 0
  def size(self):
  def hop_size(self):
  def set_hann(self):
  def set_rectangular(self):
  def clear(self):
  def load_packed(self, input: T[]):
  def load_complex(self, real: T[], imag: T[]):
  def tick(self):
  def is_active(self):
```


## `std/convolution`

```onda
import std/convolution
```

Namespace: `std::convolution<FFTSize = 256, MaxImpulseLen = 16384>`.

### Constants

```onda
const HopSize = FFTSize / 2
const HeadFFTSize = min(FFTSize, 256)
const MidFFTSize = min(FFTSize, 1024)
const LargeFFTSize = min(FFTSize, 4096)
const DirectTaps = HeadFFTSize / 2
const TailStart = DirectTaps
const HeadStageEnd = MidFFTSize / 2
const MidStageEnd = LargeFFTSize / 2
const LargeStageEnd = HopSize
const HeadStageCapacity = max(HeadStageEnd - DirectTaps, 1)
const MidStageCapacity = max(MidStageEnd - HeadStageEnd, 1)
const LargeStageCapacity = max(LargeStageEnd - MidStageEnd, 1)
const FinalStageCapacity = max(MaxImpulseLen - LargeStageEnd, 1)
```

### Functions

```onda
def stage_window_count(frames: i32, start: i32, end: i32, window_size: i32) -> i32:
def impulse_window_count(frames: i32) -> i32:
def impulse_window_end(start: i32, frames: i32) -> i32:
```

### Processor `TimeDomainConvolver<T>`

```onda
proc TimeDomainConvolver<T>:
  ins<T> 1
  outs<T> 1
  events:
    set_impulse(values: T[]):
    reset():
```

### Processor `BlockConvolver<T>`

```onda
proc BlockConvolver<T>:
  ins<T> 1
  outs<T> 1
  events:
    set_offset(value: i32 = -1):
    set_impulse(values: T[]):
    reset():
```

### Processor `ZeroLatencyConvolver<T>`

```onda
proc ZeroLatencyConvolver<T>:
  ins<T> 1
  outs<T> 1
  events:
    set_offset(value: i32 = -1):
    set_impulse(values: T[]):
    begin_impulse(value_count: i32):
    set_impulse_window(start: i32, values: T[]):
    reset():
```


## `std/lookup`

```onda
import std/lookup
```

### Unqualified functions

```onda
def read(buf, i: i32):
def read(buf, ch: i32, i: i32):
def write(buf, i: i32, value):
def write(buf, ch: i32, i: i32, value):
def readL(buf, pos):
def readL(buf, ch: i32, pos):
def readLW(buf, pos):
def readLW(buf, ch: i32, pos):
def readC(buf, pos):
def readC(buf, ch: i32, pos):
def readCW(buf, pos):
def readCW(buf, ch: i32, pos):
```

Namespace: `std::lookup`.

### Functions

```onda
def read(buf, i: i32):
def read(buf, ch: i32, i: i32):
def write(buf, i: i32, value):
def write(buf, ch: i32, i: i32, value):
def readL(buf, pos):
def readL(buf, ch: i32, pos):
def readLW(buf, pos):
def readLW(buf, ch: i32, pos):
def readC(buf, pos):
def readC(buf, ch: i32, pos):
def readCW(buf, pos):
def readCW(buf, ch: i32, pos):
```


## `std/random`

```onda
import std/random
```

Namespace: `std::random`.

### Constants

```onda
const RNG_MASK: i64 = 2147483647
const RNG_MULT: i64 = 1103515245
const RNG_INC: i64 = 12345
```

### Functions

```onda
def seed_state(seed: i64):
def step_state(state: i64):
```

### Struct `Rng<T>`

```onda
struct Rng<T>:
  state: i64 = 1
  def seed(self, seed: i64):
  def next_u31(self):
  def next(self):
  def bipolar(self):
  def range(self, lo: T, hi: T):
```


## `std/prelude`

Imported automatically. It loads:

- `std/math`
- `std/lookup`
- `std/random`

