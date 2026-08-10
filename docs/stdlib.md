---
title: Standard library
description: Generated reference for Onda's built-in standard-library modules, functions, structs, and processors.
permalink: /docs/stdlib/
section: reference
eyebrow: Language reference
---

# Onda standard library

This page is generated from the standard library embedded in the compiler. Run `npm run docs:stdlib` after changing `stdlib/`; CI verifies that the checked-in reference is current. Declarations whose names begin with `_` are implementation helpers and are omitted.

`std/prelude` is imported automatically. It loads `std/math`, `std/lookup`, and `std/random`, including the unqualified forwarding functions from the first two modules. Import the other modules explicitly before using their qualified APIs.

## Modules

| Module | Provides |
| --- | --- |
| [`std/math`](#stdmath) | `ampdb`, `clamp`, `cpsmidi`, `cpsoct`, `cubic_interp`, `dbamp`, `expexp`, `explin`, `fract`, `inverse_lerp`, `lerp`, `linexp`, `linlin`, `map`, `midicps`, `midiratio`, `octcps`, `ratiomidi`, `sign`, `smoothstep`, `wrap` |
| [`std/complex`](#stdcomplex) | `Complex` |
| [`std/osc`](#stdosc) | `Phasor`, `Pulse`, `Saw`, `SawDown`, `Sine`, `Square`, `Triangle`, `poly_blep` |
| [`std/filter`](#stdfilter) | `DCBlock`, `OnePole`, `Svf`, `mode` |
| [`std/env`](#stdenv) | `ADSR`, `AR`, `ASR`, `DecayEnv`, `stage` |
| [`std/noise`](#stdnoise) | `Brown`, `Pink`, `White` |
| [`std/levels`](#stdlevels) | `DB_PER_NAT`, `DB_TO_GAIN_SCALE`, `HALF_PI`, `MIN_FLOAT`, `db_to_gain`, `gain_to_db`, `pan_3db`, `pan_linear` |
| [`std/mix`](#stdmix) | `ConstantSum`, `Crossfade`, `MonoToStereo`, `StereoToMono`, `chans` |
| [`std/gain`](#stdgain) | `Constant`, `DB_TO_GAIN_SCALE`, `Db`, `Smoothed`, `SmoothedDb` |
| [`std/pitch`](#stdpitch) | `A4_HZ`, `INV_LN_2_OVER_12`, `LN_2_OVER_12`, `MIDI_A4`, `MIN_FLOAT`, `hz_to_note`, `note_to_hz`, `ratio_between` |
| [`std/smoothing`](#stdsmoothing) | `Lag`, `LagUD`, `Slew` |
| [`std/delay`](#stddelay) | `Delay` |
| [`std/data`](#stddata) | `Data` |
| [`std/fft`](#stdfft) | `Blackman`, `FFT`, `Hamming`, `Hann`, `RealFFT`, `RealIFFT`, `Rectangular`, `STFT` |
| [`std/convolution`](#stdconvolution) | `BlockConvolver`, `FFTStorageSize`, `HopSize`, `MaxPartitions`, `TailStart`, `TimeDomainConvolver`, `ZeroLatencyConvolver` |
| [`std/lookup`](#stdlookup) | `calcIdx`, `read`, `readC`, `readCW`, `readL`, `readLW`, `wrapIdx`, `write` |
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
```

### Processor `Sine<T>`

```onda
proc Sine<T>:
  outs<T> 1
  params:
    freq: T = 440.0
    amp: T = 1.0
```

### Processor `Saw<T>`

```onda
proc Saw<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0
```

### Processor `SawDown<T>`

```onda
proc SawDown<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0
```

### Processor `Pulse<T>`

```onda
proc Pulse<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_shape
    width: T = 0.5 => update_shape
    amp: T = 1.0
```

### Processor `Square<T>`

```onda
proc Square<T>:
  outs<T> 1
  params:
    freq: T = 440.0
    amp: T = 1.0
```

### Processor `Triangle<T>`

```onda
proc Triangle<T>:
  outs<T> 1
  params:
    freq: T = 440.0 => update_freq
    amp: T = 1.0
```


## `std/filter`

```onda
import std/filter
```

Namespace: `std::filter`.

### Namespace `mode`

#### Constants

```onda
const ONE_POLE_LOWPASS: i32 = 0
const ONE_POLE_HIGHPASS: i32 = 1
const SVF_LOWPASS: i32 = 0
const SVF_HIGHPASS: i32 = 1
const SVF_BANDPASS: i32 = 2
const SVF_NOTCH: i32 = 3
const SVF_PEAK: i32 = 4
const SVF_ALLPASS: i32 = 5
```

### Processor `OnePole<T>`

```onda
proc OnePole<T>:
  ins<T> 1
  outs<T> 1
  params:
    cutoff: T = 1000.0 => update_cutoff
    mode: i32 = mode::ONE_POLE_LOWPASS
```

### Processor `DCBlock<T>`

```onda
proc DCBlock<T>:
  ins<T> 1
  outs<T> 1
```

### Processor `Svf<T>`

```onda
proc Svf<T>:
  ins<T> 1
  outs<T> 1
  params:
    pin cutoff: T = 1000.0
    pin q: T = 0.707107
    mode: i32 = 0
  events:
    update_coeffs(cutoff_v: T, q_v: T):
```


## `std/env`

```onda
import std/env
```

Namespace: `std::env`.

### Processor `DecayEnv<T>`

```onda
proc DecayEnv<T>:
  outs<T> 1
  params:
    decay_s: T = 0.2 => update_decay
    trigger: T = 0.0
```

### Processor `AR<T>`

```onda
proc AR<T>:
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_steps
    release_s: T = 0.1 => update_steps
    trigger: T = 0.0
```

### Processor `ASR<T>`

```onda
proc ASR<T>:
  outs<T> 1
  params:
    attack_s: T = 0.01 => update_shape
    sustain: T = 1.0 => update_shape
    release_s: T = 0.1 => update_shape
    gate: T = 0.0
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
    sustain: T = 0.7 => update_shape
    release_s: T = 0.2 => update_shape
    gate: T = 0.0
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
    seed(seed: i64):
```

### Processor `Pink<T>`

```onda
proc Pink<T>:
  outs<T> 1
  params:
    amp: T = 1.0
  events:
    seed(seed: i64):
```

### Processor `Brown<T>`

```onda
proc Brown<T>:
  outs<T> 1
  params:
    amp: T = 1.0
  events:
    seed(seed: i64):
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
    mix: T = 0.5
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
    mix: T = 0.5
```


## `std/gain`

```onda
import std/gain
```

Namespace: `std::gain`.

### Constants

```onda
const DB_TO_GAIN_SCALE: f64 = 0.11512925464970229
```

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


## `std/delay`

```onda
import std/delay
```

Namespace: `std::delay`.

### Processor `Delay<T>`

```onda
proc Delay<T>:
  ins<T> 1
  outs<T> 1
  params:
    delay_s: T = 0.1 => update_delay
    feedback: T = 0.0
    mix: T = 1.0
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
  twiddles: std::complex::Complex<T>[N]
  bitrev: i32[N]
  prepared: bool
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
  window_kind: i32 = _WindowHann
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
  packed: T[N]
  window_kind: i32 = _WindowHann
  write: i32
  filled: i32
  since_hop: i32
  ready: bool
  def size(self):
  def real_bin_count(self):
  def hop_size(self):
  def set_rectangular(self):
  def set_hann(self):
  def clear(self):
  def push(self, x: T):
  def is_ready(self):
  def packed_value(self, i: i32):
```

### Struct `RealIFFT<T>`

```onda
struct RealIFFT<T>:
  fft: FFT<T>
  output: T[N]
  norm: T[N]
  window_kind: i32 = _WindowHann
  frame: i32
  pending: i32
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
const MaxPartitions = (MaxImpulseLen + HopSize - 1) / HopSize
const FFTStorageSize = MaxPartitions * FFTSize
const TailStart = HopSize
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
    set_impulse(values: T[]):
    reset():
```

### Processor `ZeroLatencyConvolver<T>`

```onda
proc ZeroLatencyConvolver<T>:
  ins<T> 1
  outs<T> 1
  events:
    set_impulse(values: T[]):
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
def calcIdx(pos):
def wrapIdx(i: i32, n: i32):
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

