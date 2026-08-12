const STRUCT_DATA_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[4], gain: f32 }
init {
  v = Voice(0.5)
}
sample {
  v.delay[-1.2] = 1.0
  v.delay[1.9] = 2.0
  v.delay[3.7] = 4.0
  out1 = v.delay[99.1] + v.delay[1.2] + v.delay[-8.1] * v.gain
}
"#;

const STRUCT_DATA_IS_PER_INSTANCE_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[2], gain: f32 }
init {
  a = Voice(1.0)
  b = Voice(1.0)
}
sample {
  a.delay[0.0] = 1.0
  b.delay[0.0] = 3.0
  out1 = a.delay[0.0] + b.delay[0.0]
}
"#;

const STRUCT_DATA_FIELD_NON_INDEXED_WRITE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Voice { delay: f32[4], gain: f32 }
init {
  v = Voice(1.0)
}
sample {
  v.delay = 1.0
  out1 = 0.0
}
"#;

const IMPLICIT_IO_GAPPED_EXAMPLE: &str = r#"
sample {
  out2 = in3 * 0.5
}
"#;

const SPARSE_DECLARED_IO_EXAMPLE: &str = r#"
ins { in3 }
outs { out3 }
sample {
  out3 = in3
}
"#;

const BUILTIN_CONSTS_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = f32(PI) + f32(TWO_PI) + SAMPLE_RATE - SR
}
"#;

const BUILTIN_CONSTS_SR_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + f32(TWO_PI) / SR
  out1 = sin(phase)
}
"#;

const BUILTIN_CONSTS_SAMPLERATE_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + f32(TWO_PI) / SAMPLERATE
  out1 = sin(phase)
}
"#;

const BUILTIN_CONSTS_LOWERCASE_ALIASES_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = f32(pi) + f32(two_pi) + f32(twopi) + samplerate - sample_rate + f32(blocksize) - f32(block_size)
}
"#;

const BUILTIN_CONSTS_LOWERCASE_SR_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
init {
  phase = 0.0
}
sample {
  phase = phase + f32(twopi) / samplerate
  out1 = sin(phase)
}
"#;

const BUILTIN_MATH_TYPED_OVERLOADS_EXAMPLE: &str = r#"
outs {
  out1
  out2
}
sample {
  out1 = cos(0.0) + exp(0.0) + log(1.0)
  out2 = f32(cos(f64(0.0)) + exp(f64(0.0)) + log(f64(1.0)))
}
"#;

const BUILTIN_INTRINSICS_EXAMPLE: &str = r#"
outs { out1 }
sample {
  acc = abs(-0.5) + cos(0.0) + sqrt(4.0) + exp(0.0) + log(exp(1.0))
  acc = acc + pow(2.0, 3.0) + min(3.0, 4.0) + max(3.0, 4.0) + fma(2.0, 3.0, 4.0)
  out1 = acc + floor(1.8) + ceil(1.2) + round(1.6) + trunc(1.6)
}
"#;

const STDLIB_MATH_AUTO_IMPORT_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = clamp(2.0, 0.0, 1.0) + std::math::lerp(0.0, 2.0, 0.25) + map(0.0, 1.0, -1.0, 1.0, 0.75)
}
"#;

const STDLIB_MATH_LOCAL_SYMBOL_WINS_EXAMPLE: &str = r#"
outs { out1 }
def clamp(x, lo, hi) {
  return 5.0
}
sample {
  out1 = clamp(2.0, 0.0, 1.0) + std::math::clamp(2.0, 0.0, 1.0)
}
"#;

const STDLIB_RANDOM_GENERIC_RNG_EXAMPLE: &str = r#"
outs { out1: f64, out2: f64, out3: f64 }
init {
  rng = std::random::Rng<f64>(state = 123)
}
sample {
  out1 = rng.next()
  out2 = rng.bipolar()
  out3 = rng.range(f64(-2.0), f64(2.0))
}
"#;

const STDLIB_BUFFER_READ_MONO_EXAMPLE: &str = r#"
import std/lookup
buffers { b: buffer<f32> }
outs { out1 }
sample {
  out1 = std::lookup::read(b, 2)
}
"#;

const STDLIB_BUFFER_INTERP_STEREO_EXAMPLE: &str = r#"
import std/lookup
buffers { b: buffer<f32[2]> }
outs { out1 }
sample {
  out1 = std::lookup::read(b, 0, 1) + std::lookup::readL(b, 1, 0.5) + std::lookup::readLW(b, 0, 3.5) + std::lookup::readC(b, 1, 1.0) + std::lookup::readCW(b, 0, 3.5)
}
"#;

const STDLIB_BUFFER_AUTO_IMPORT_ARRAY_AND_BUFFER_EXAMPLE: &str = r#"
buffers { b: buffer<f32> }
outs { out1 }
init {
  a: f32[4]
  a[0] = 1.0
  a[1] = 2.0
  a[2] = 3.0
  a[3] = 4.0
}
sample {
  out1 = a.read(1) + readL(a, 1.5) + readCW(a, 3.5) + b.readC(2.0) + b.readCW(3.5)
}
"#;

const STDLIB_LOOKUP_WRITE_ARRAY_AND_BUFFER_EXAMPLE: &str = r#"
import std/lookup
buffers { b: buffer<f32> }
outs { out1 }
init {
  a: f32[4]
}
sample {
  std::lookup::write(a, 1, 2.5)
  std::lookup::write(b, 2, 4.0)
  out1 = std::lookup::read(a, 1) + std::lookup::read(b, 2)
}
"#;

const FLOOR_FRACT_WRAP_NUMERIC_BEHAVIOR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = i64(9007199254740993)
}
sample {
  out1 = floor(1.8) + fract(2.25) + wrap(5.5, 0.0, 2.0) + f32(x - i64(9007199254740993))
}
"#;

const BUILTIN_INT_INTRINSICS_EXAMPLE: &str = r#"
outs { out1 }
init {
  a: i32 = i32(-3)
  b: i32 = 7
  c: i64 = 9
}
sample {
  out1 = f32(abs(a)) + f32(min(a, b)) + f32(max(i64(2), c)) + pow(2, 3)
}
"#;

const BUILTIN_FLOAT_ONLY_TYPE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i32 = 1
}
sample {
  out1 = sin(x)
}
"#;

const BITWISE_OPS_EXAMPLE: &str = r#"
outs { out1 }
sample {
  a: i32 = 6
  b: i32 = 3
  x: i32 = (a & b) + (a | b) + (a ^ b) + (1 << 3) + (8 >> 1) + ~1
  out1 = f32(x)
}
"#;

const BITWISE_FLOAT_OPERAND_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  out1 = f32(1.0 & 1)
}
"#;

const ASSERT_PASSES_EXAMPLE: &str = r#"
namespace Config {
  assert(BLOCK_SIZE > 0)
}
outs { out1 }
sample {
  out1 = 1.0
}
"#;

const ASSERT_NAMESPACE_POWER_OF_TWO_ERROR_EXAMPLE: &str = r#"
namespace FFT<N = 4> {
  assert((N & (N - 1)) == 0)
  struct Tag {
    value
  }
}
outs { out1 }
init {
  tag: FFT<6>::Tag
}
sample {
  out1 = 0.0
}
"#;

const STDLIB_FFT_ZERO_SIZE_ERROR_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  fft: std::fft<0>::FFT<f32>
}
sample {
  out1 = 0.0
}
"#;

const STDLIB_COMPLEX_STRUCT_EXAMPLE: &str = r#"
import std/complex
outs 4
init {
  z: std::complex::Complex<f32>
  w: std::complex::Complex<f32>
  z.set(1.0, 2.0)
  w.set(3.0, -4.0)
  z.mul_assign(w)
}
sample {
  out1 = z.real()
  out2 = z.imag()
  out3 = z.magnitude()
  out4 = z.phase()
}
"#;

const STDLIB_COMPLEX_POLAR_F64_EXAMPLE: &str = r#"
import std/complex
outs 3
init {
  z: std::complex::Complex<f64>
  z.set_polar(f64(2.0), f64(0.5))
  z.conjugate()
  z.scale_assign(f64(0.5))
}
sample {
  out1 = f32(z.real())
  out2 = f32(z.imag())
  out3 = f32(z.power())
}
"#;

const STDLIB_FFT_IMPULSE_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  input: f32[8]
  input[0] = 1.0
  input[1] = 0.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  fft: std::fft<8>::FFT<f32>
}
sample {
  fft.forward_real(input)
  out1 = fft.real(0) + fft.real(1) + fft.real(2) + fft.real(3) + fft.real(4) + fft.real(5) + fft.real(6) + fft.real(7)
}
"#;

const STDLIB_FFT_IMPULSE_F64_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  input: f64[8]
  input[0] = f64(1.0)
  input[1] = f64(0.0)
  input[2] = f64(0.0)
  input[3] = f64(0.0)
  input[4] = f64(0.0)
  input[5] = f64(0.0)
  input[6] = f64(0.0)
  input[7] = f64(0.0)
  fft: std::fft<8>::FFT<f64>
}
sample {
  fft.forward_real(input)
  out1 = f32(fft.real(0) + fft.real(1) + fft.real(2) + fft.real(3) + fft.real(4) + fft.real(5) + fft.real(6) + fft.real(7))
}
"#;

const STDLIB_FFT_REAL_PACKED_EXAMPLE: &str = r#"
import std/fft
outs { out1 }
init {
  input: f32[8]
  packed: f32[8]
  input[0] = 1.0
  input[1] = 0.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  fft: std::fft<8>::FFT<f32>
}
sample {
  fft.forward_real_packed(input, packed)
  out1 = packed[0] + packed[1] + packed[2] + packed[3] + packed[4] + packed[5] + packed[6] + packed[7]
}
"#;

const STDLIB_FFT_REAL_PACKED_ROUNDTRIP_EXAMPLE: &str = r#"
import std/fft
outs 4
init {
  input: f32[4]
  packed: f32[4]
  output: f32[4]
  input[0] = 1.0
  input[1] = 2.0
  input[2] = 3.0
  input[3] = 4.0
  fft: std::fft<4>::FFT<f32>
}
sample {
  fft.forward_real_packed(input, packed)
  fft.inverse_real_packed(packed, output)
  out1 = output[0]
  out2 = output[1]
  out3 = output[2]
  out4 = output[3]
}
"#;

const STDLIB_FFT_REAL_SPECTRUM_HELPERS_EXAMPLE: &str = r#"
import std/fft
outs 4
init {
  input: f32[8]
  mags: f32[5]
  power: f32[5]
  phase: f32[5]
  input[0] = 0.0
  input[1] = 1.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  fft: std::fft<8>::FFT<f32>
}
sample {
  fft.forward_real_magnitude(input, mags)
  fft.forward_real_power(input, power)
  fft.forward_real_phase(input, phase)
  out1 = mags[0] + mags[1] + mags[2] + mags[3] + mags[4]
  out2 = power[0] + power[1] + power[2] + power[3] + power[4]
  out3 = phase[0] + phase[1] + phase[2] + phase[3] + phase[4]
  out4 = f32(fft.size() + fft.real_bin_count())
}
"#;

const STDLIB_STFT_HANN_WINDOW_EXAMPLE: &str = r#"
import std/fft
outs 4
init {
  input: f32[8]
  mags: f32[5]
  window: f32[8]
  input[0] = 0.0
  input[1] = 1.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0
  stft: std::fft<8>::STFT<f32>
}
sample {
  stft.set_hann()
  stft.store_window(window)
  stft.forward_real_magnitude(input, mags)
  out1 = mags[0] + mags[1] + mags[2] + mags[3] + mags[4]
  out2 = window[1] + window[6]
  out3 = stft.magnitude(0)
  out4 = f32(stft.size() + stft.real_bin_count())
}
"#;

const STDLIB_STFT_DEFAULT_HANN_WINDOW_EXAMPLE: &str = r#"
import std/fft
outs 2
init {
  input: f32[4]
  mags: f32[3]
  window: f32[4]
  input[0] = 0.0
  input[1] = 1.0
  input[2] = 0.0
  input[3] = 0.0
  stft: std::fft<4>::STFT<f32>
}
sample {
  stft.store_window(window)
  stft.forward_real_magnitude(input, mags)
  out1 = window[0] + window[1] + window[2] + window[3]
  out2 = mags[0] + mags[1] + mags[2]
}
"#;

const STDLIB_REALFFT_STRUCT_EXAMPLE: &str = r#"
import std/fft
import std/osc
outs 1
init {
  saw = std::osc::Saw(freq = 440.0)
  fwd = std::fft<64>::RealFFT()
  inv = std::fft<64>::RealIFFT()
  scratch_re: f32[64]
  scratch_im: f32[64]
}
sample {
  saw.freq = 440.0
  if (fwd.push(saw())) {
    half = 64 >> 1
    for i in 0..(half + 1) {
      scratch_re[i] = 0.0
      scratch_im[i] = 0.0
    }
    scratch_re[0] = fwd.real(0)
    for k in 1..half {
      shifted = k + 1
      if (shifted < half) {
        scratch_re[shifted] = fwd.real(k)
        scratch_im[shifted] = fwd.imag(k)
      }
    }
    inv.load_complex(scratch_re, scratch_im)
  }
  out1 = inv.tick()
}
"#;

const STDLIB_REALFFT_REFERENCE_EXAMPLE: &str = r#"
import std/fft
outs 1
init {
  fwd = std::fft<64>::RealFFT()
  reference = std::fft<64>::STFT()
  input: f32[64]
  clock: i32 = 0
}
sample {
  x = sin(TWO_PI * 5.0 * f32(clock) / 64.0)
  x = x + cos(TWO_PI * 13.0 * f32(clock) / 64.0) * 0.37
  if (clock < 64) {
    input[clock] = x
  }

  error = 0.0
  if (fwd.push(x)) {
    reference.forward_real(input)
    for i in 0..(fwd.real_bin_count()) {
      error = max(error, abs(fwd.real(i) - reference.real(i)))
      error = max(error, abs(fwd.imag(i) - reference.imag(i)))
    }
  }
  clock = clock + 1
  out1 = error
}
"#;

const STDLIB_REALFFT_NAMESPACED_PROC_EXAMPLE: &str = r#"
import std/fft
import std/osc

namespace BinShift<N = 64>:
  proc Main:
    outs 1
    params:
      freq = 440.0
    init:
      saw = std::osc::Saw(freq = freq)
      fwd = std::fft<N>::RealFFT()
      inv = std::fft<N>::RealIFFT()
      scratch_re: f32[N]
      scratch_im: f32[N]
    block:
      saw.freq = freq
      sample:
        if (fwd.push(saw())):
          half = N >> 1
          for i in 0..(half + 1):
            scratch_re[i] = 0.0
            scratch_im[i] = 0.0
          scratch_re[0] = fwd.real(0)
          for k in 1..half:
            shifted = k + 1
            if (shifted < half):
              scratch_re[shifted] = fwd.real(k)
              scratch_im[shifted] = fwd.imag(k)
          inv.load_complex(scratch_re, scratch_im)
        out1 = inv.tick()

outs 1
init:
  p = BinShift<64>::Main()
sample:
  out1 = p()
"#;

const STDLIB_REALFFT_HANN_OLA_PASSTHROUGH_EXAMPLE: &str = r#"
import std/fft
import std/osc
outs 3
init {
  osc = std::osc::Sine(freq = 220.0)
  fwd = std::fft<64>::RealFFT()
  inv = std::fft<64>::RealIFFT()
  packed: f32[64]
  delay: f32[64]
  delay_i: i32 = 0
  frames_seen: i32 = 0
}
sample {
  x = osc()
  expected_i = delay_i + 1
  if (expected_i >= 64) {
    expected_i = expected_i - 64
  }
  expected = delay[expected_i]
  delay[delay_i] = x
  delay_i = delay_i + 1
  if (delay_i >= 64) {
    delay_i = 0
  }

  if (fwd.push(x)) {
    fwd.store_real_packed(packed)
    inv.load_packed(packed)
  }

  y = inv.tick()
  frames_seen = frames_seen + 1
  if (frames_seen > 192) {
    out1 = y - expected
  } else {
    out1 = 0.0
  }
  out2 = f32(fwd.hop_size())
  out3 = f32(inv.hop_size())
}
"#;

const STDLIB_REALIFFT_HANN_PRIMING_EXAMPLE: &str = r#"
import std/fft
outs 1
init {
  inv = std::fft<64>::RealIFFT()
  spectrum_re: f32[64]
  spectrum_im: f32[64]
  clock: i32 = 0
}
sample {
  if (clock == 0) {
    spectrum_re[5] = 32.0
    inv.load_complex(spectrum_re, spectrum_im)
  } elif (clock == 32) {
    spectrum_re[5] = -32.0
    inv.load_complex(spectrum_re, spectrum_im)
  } elif (clock == 128) {
    spectrum_re[5] = 32.0
    inv.load_complex(spectrum_re, spectrum_im)
  }
  out1 = inv.tick()
  clock = clock + 1
}
"#;

const STDLIB_REALIFFT_FIRST_FRAME_EXAMPLE: &str = r#"
import std/fft
outs 1
init {
  inv = std::fft<64>::RealIFFT()
  spectrum_re: f32[64]
  spectrum_im: f32[64]
  clock: i32 = 0
  inv.set_rectangular()
}
sample {
  if (clock == 0) {
    for k in 0..33 {
      phase = -TWO_PI * f32(k) / 64.0
      spectrum_re[k] = cos(phase)
      spectrum_im[k] = sin(phase)
    }
    inv.load_complex(spectrum_re, spectrum_im)
  }
  expected = 0.0
  if (clock == 1) {
    expected = 1.0
  }
  out1 = abs(inv.tick() - expected)
  clock = clock + 1
}
"#;

const STDLIB_CONVOLUTION_TIME_DOMAIN_EVENT_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
events {
  set_ir(values: f32[4]) {
    conv.set_impulse(values)
  }
}
init {
  conv = std::convolution<8, 4>::TimeDomainConvolver<f32>()
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_BLOCK_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
init {
  conv = std::convolution<8, 4>::BlockConvolver<f32>()
  ir: f32[4] = [1.0, 0.5, 0.25, 0.0]
  conv.set_impulse(ir)
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
init {
  conv = std::convolution<8, 8>::ZeroLatencyConvolver<f32>()
  ir: f32[5] = [1.0, 0.5, 0.25, 0.0, 0.125]
  conv.set_impulse(ir)
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_CONST_NAMESPACE_EXAMPLE: &str = r#"
import std/convolution
const FFT_SIZE = 8
const MAX_IR = 8
outs { out1 }
init {
  conv = std::convolution<FFT_SIZE, MAX_IR>::ZeroLatencyConvolver<f32>()
  ir: f32[5] = [1.0, 0.5, 0.25, 0.0, 0.125]
  conv.set_impulse(ir)
}
sample {
  out1 = conv(in1)
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_ANALYZE_EXAMPLE: &str = r#"
import std/convolution
const FFT_SIZE = 1024
const MAX_IR = 100000
outs { out1 }
init {
  conv = std::convolution<FFT_SIZE, MAX_IR>::ZeroLatencyConvolver<f32>()
}
sample {
  out1 = 0.0
}
"#;

const STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_WRAPPER_ANALYZE_EXAMPLE: &str = r#"
import std/convolution
const MAX_IR = 100000
const FFT_SIZE = 1024

namespace convolution_wav_impulse<N = MAX_IR>:
  proc Engine:
    init:
      conv = std::convolution<FFT_SIZE, N>::ZeroLatencyConvolver<f32>()
    sample:
      out1 = 0.0

init:
  engine = convolution_wav_impulse<MAX_IR>::Engine()

sample:
  out1 = 0.0
"#;

const STDLIB_CONVOLUTION_F64_EXAMPLE: &str = r#"
import std/convolution
outs { out1 }
init {
  conv = std::convolution<8, 4>::TimeDomainConvolver<f64>()
  ir: f64[4] = [1.0, 0.5, 0.25, 0.0]
  conv.set_impulse(ir)
}
sample {
  out1 = f32(conv(f64(in1)))
}
"#;

const NESTED_STRUCT_FIELD_AND_METHOD_EXAMPLE: &str = r#"
outs 2
struct Inner<T>:
  data: T[2]

  def set_pair(self, a: T, b: T):
    self.data[0] = a
    self.data[1] = b

  def sum(self):
    return self.data[0] + self.data[1]

struct Outer<T>:
  inner: Inner<T>

  def init_pair(self, a: T, b: T):
    self.inner.set_pair(a, b)

  def sum(self):
    return self.inner.sum()

init {
  outer: Outer<f32>
}
sample {
  outer.init_pair(1.5, 2.5)
  out1 = outer.inner.data[0]
  out2 = outer.sum()
}
"#;

const MULTILINE_STRUCT_METHOD_CALL_EXAMPLE: &str = r#"
outs { out1 }
struct Pair:
  a: f32
  b: f32

  def set(self, a, b):
    self.a = a
    self.b = b

init {
  p: Pair
}
sample {
  p.set(
    1.25,
    2.75,
  )
  out1 = max(
    p.a,
    p.b,
  )
}
"#;

const NESTED_GENERIC_STRUCT_ARRAY_FIELD_EXAMPLE: &str = r#"
outs 2
struct Stereo<T>:
  v: T[2]

struct Rack:
  items: Stereo<f32>[2]

init {
  rack: Rack
}
sample {
  s = rack.items[1]
  s.v[0] = 1.0
  s.v[1] = 2.0
  out1 = s.v[0]
  out2 = s.v[0] + s.v[1]
}
"#;

const BLOCK_SIZE_CONST_EXAMPLE: &str = r#"
outs { out1 }
init {
  v = BLOCK_SIZE
}
block {
  v = v + BLOCK_SIZE
  sample {
    out1 = v
  }
}
"#;

const BLOCK_SIZE_ALIASES_CONST_EXAMPLE: &str = r#"
outs { out1 }
init {
  v = blocksize + BLOCKSIZE - block_size
}
sample {
  out1 = v
}
"#;

const BLOCK_EXEC_ONCE_PER_PROCESS_EXAMPLE: &str = r#"
outs { out1 }
init {
  ctr = 0.0
}
block {
  ctr = ctr + 1.0
  sample {
    out1 = ctr
  }
}
"#;

const BLOCK_SCALAR_VISIBLE_IN_SAMPLE_EXAMPLE: &str = r#"
outs { out1 }
params { freq = 440.0 }
init { phase = 0.0 }
block {
  incr = freq * f32(TWO_PI) / SR
  sample {
    phase = phase + incr
    if (phase > f32(TWO_PI)) { phase = phase - f32(TWO_PI) }
    out1 = sin(phase)
  }
}
"#;

const BLOCK_IO_FORBIDDEN_ERROR_EXAMPLE: &str = r#"
outs { out1 }
block {
  out1 = 0.0
  sample {
    out1 = 0.0
  }
}
"#;

const BUILTIN_CONST_ASSIGN_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  PI = 0.0
  out1 = 0.0
}
"#;

const BUILTIN_CONST_ASSIGN_LOWERCASE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  pi = 0.0
  out1 = 0.0
}
"#;

const NAMESPACE_STRUCT_CTOR_EXAMPLE: &str = r#"
outs { out1 }
namespace FX:
  struct MyStruct:
    field: f32 = 0.75
init:
  a = FX::MyStruct()
sample:
  out1 = a.field
"#;

const NAMESPACE_DEF_RESOLUTION_EXAMPLE: &str = r#"
outs { out1 }
def g(x) {
  return x + 100.0
}
namespace NS {
  def p(x) {
    return x + 10.0
  }
  namespace Inner:
    def run(x):
      return p(x) + g(x)
}
sample {
  out1 = NS::Inner::run(1.0)
}
"#;

const NAMESPACE_TOP_LEVEL_UNQUALIFIED_CALL_ERROR_EXAMPLE: &str = r#"
outs { out1 }
namespace NS:
  def f(x):
    return x
sample:
  out1 = f(1.0)
"#;

const TYPED_NARROWING_ASSIGNMENT_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = i64(1.0)
}
sample {
  out1 = x
}
"#;

const IF_CONDITION_BOOL_ERROR_EXAMPLE: &str = r#"
outs { out1 }
sample {
  if (1.0) { out1 = 1.0 } else { out1 = 0.0 }
}
"#;

const IF_BRANCH_TYPE_CONFLICT_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  if true {
    x = 1
  } else {
    x = 1.0
  }
}
sample {
  out1 = 0.0
}
"#;

const TYPED_DATA_ELEM_PRIMITIVES_OK_EXAMPLE: &str = r#"
outs { out1 }
init {
  a: f64[8]
  b: i32[4]
  c: i64[2]
  d: bool[2]
  a[0] = 0.5
  b[0] = 2
  c[0] = i64(3)
  d[0] = true
}
sample {
  out1 = f32(a[0.0]) + f32(b[0.0]) + f32(c[0.0]) + f32(d[0.0])
}
"#;

const TYPED_DATA_STRUCT_SCALAR_PRIMITIVES_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Cell { a: i32, b: f64, c: bool }
init {
  cells: Cell[1]
  cell = cells[0]
  cell.a = 2
  cell.b = 3.5
  cell.c = true
}
sample {
  cell = cells[0]
  out1 = f32(cell.a) + f32(cell.b) + f32(cell.c)
}
"#;
const DATA_BOOL_INDEX_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[4]
}
sample {
  buf[true] = 1.0
  out1 = buf[0.0]
}
"#;

const DATA_CONST_OOB_INDEX_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  buf: f32[4]
}
sample {
  out1 = buf[4]
}
"#;

const TYPED_WIDENING_ASSIGNMENT_OK_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = 1
}
sample {
  out1 = f32(x)
}
"#;

const TYPED_INIT_F64_PRESERVES_PRECISION_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: f64 = f64(1.234567890123)
}
sample {
  if (x == f64(1.234567890123)) { out1 = 1.0 } else { out1 = 0.0 }
}
"#;

const TYPED_INIT_I64_PRESERVES_VALUE_EXAMPLE: &str = r#"
outs { out1 }
init {
  x: i64 = i64(9007199254740993)
}
sample {
  if (x == i64(9007199254740993)) { out1 = 1.0 } else { out1 = 0.0 }
}
"#;

const TYPED_BLOCK_DECLARATION_EXAMPLE: &str = r#"
outs { out1 }
block {
  x: f64 = f64(2.5)
  sample {
    out1 = f32(x)
  }
}
"#;

const TYPED_SAMPLE_DECLARATION_EXAMPLE: &str = r#"
outs { out1 }
sample {
  x: i32 = i32(7)
  out1 = f32(x)
}
"#;

const TYPED_DEF_DECLARATION_EXAMPLE: &str = r#"
outs { out1 }
def foo() {
  x: i32 = i32(3)
  y: f64 = f64(0.5)
  return f32(x) + f32(y)
}
sample {
  out1 = foo()
}
"#;

const DEF_RETURN_F64_INFERENCE_EXAMPLE: &str = r#"
outs { out1 }
def mydef() {
  return f64(0.5)
}
sample {
  out1 = f32(mydef())
}
"#;

const DEF_MONOMORPHIZES_FROM_CALL_ARGUMENTS_OK_EXAMPLE: &str = r#"
outs { out1 }
def id(x) {
  return x
}
sample {
  out1 = f32(id(f64(1.25)))
}
"#;

const DEF_MONOMORPHIZES_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
outs { out1 }
def twice(x) {
  return x + x
}
sample {
  out1 = twice(1.5) + f32(twice(f64(2.25)))
}
"#;

const NON_GENERIC_DEF_WITH_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
def id(x) {
  return x
}
sample {
  out1 = id<f32>(1.0)
}
"#;

const GENERIC_STRUCT_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> { a: T, b: T }
init {
  p = Pair<f64>(f64(1.25), f64(0.5))
}
sample {
  out1 = f32(p.a + p.b)
}
"#;

const GENERIC_STRUCT_MISSING_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> { a: T, b: T }
init {
  p = Pair(1.0, 2.0)
}
sample {
  out1 = p.a + p.b
}
"#;

const GENERIC_STRUCT_INFER_FROM_VAR_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Box<T> { v: T }
init {
  x = f64(2.5)
  b = Box(x)
}
sample {
  out1 = f32(b.v)
}
"#;

const GENERIC_STRUCT_UNRESOLVED_INFERENCE_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Bank<T> { taps: T[2] }
init {
  b = Bank()
}
sample {
  out1 = 0.0
}
"#;

const GENERIC_STRUCT_TYPE_ARG_ARITY_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> { a: T, b: T }
init {
  p = Pair<f32, f64>(1.0, 2.0)
}
sample {
  out1 = 0.0
}
"#;

const NON_GENERIC_STRUCT_WITH_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
outs { out1 }
struct Pair { a: f32, b: f32 }
init {
  p = Pair<f32>(1.0, 2.0)
}
sample {
  out1 = p.a + p.b
}
"#;

const GENERIC_STRUCT_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Box<T> { v: T }
init {
  a = Box<f32>(1.0)
  b = Box<f64>(f64(0.25))
}
sample {
  out1 = a.v + f32(b.v)
}
"#;

const GENERIC_STRUCT_ARRAY_FIELD_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Bank<T> { taps: T[2] }
init {
  b = Bank<f64>()
  b.taps[0.0] = f64(1.5)
  b.taps[1.0] = f64(0.5)
}
sample {
  out1 = f32(b.taps[0.0] + b.taps[1.0])
}
"#;

const GENERIC_STRUCT_METHOD_OK_EXAMPLE: &str = r#"
outs { out1 }
struct Pair<T> {
  a: T
  b: T
  def sum(self) {
    return self.a + self.b
  }
}
init {
  p = Pair<f64>(f64(1.25), f64(0.75))
}
sample {
  out1 = f32(p.sum())
}
"#;

const GENERIC_PROC_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p = Gain<f64>(g = f64(0.5))
}
sample {
  out1 = f32(p(f64(2.0)))
}
"#;

const GENERIC_PROC_MISSING_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p = Gain(g = 0.5)
}
sample {
  out1 = p(2.0)
}
"#;

const GENERIC_PROC_DEFAULT_ONLY_INFERENCE_OK_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 0.5 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p = Gain()
}
sample {
  out1 = p(2.0)
}
"#;

const GENERIC_PROC_ARRAY_INFER_FROM_ARRAY_VAR_OK_EXAMPLE: &str = r#"
proc Tap<T> {
  params { w: T[2] = [0.0, 0.0] }
  outs { out1: T }
  sample {
    out1 = w[0.0] + w[1.0]
  }
}
outs { out1 }
init {
  w0: f64[2]
  w0[0.0] = f64(0.25)
  w0[1.0] = f64(0.75)
  p = Tap(w = w0)
}
sample {
  out1 = f32(p())
}
"#;

const GENERIC_PROC_UNRESOLVED_INFERENCE_ERROR_EXAMPLE: &str = r#"
proc Hold<T> {
  outs { out1: T }
  sample {
    out1 = 0.0
  }
}
outs { out1 }
init {
  p = Hold()
}
sample {
  out1 = 0.0
}
"#;

const PROC_STATE_STRUCT_CTOR_OK_EXAMPLE: &str = r#"
struct Pair { a: f32, b: f32 }

proc Voice {
  outs { out1 }
  init {
    s = Pair(1.0, 2.0)
  }
  sample {
    out1 = s.a + s.b
  }
}

outs { out1 }
init {
  v = Voice()
}
sample {
  out1 = v()
}
"#;

const PROC_STATE_GENERIC_STRUCT_CTOR_EXPLICIT_TYPE_ARGS_OK_EXAMPLE: &str = r#"
struct Pair<T> { a: T, b: T }

proc Voice {
  outs { out1 }
  init {
    s = Pair<f64>(f64(1.0), f64(2.0))
  }
  sample {
    out1 = f32(s.a + s.b)
  }
}

outs { out1 }
init {
  v = Voice()
}
sample {
  out1 = v()
}
"#;

const PROC_STATE_GENERIC_STRUCT_CTOR_INFERRED_TYPE_ARGS_OK_EXAMPLE: &str = r#"
struct Pair<T> { a: T, b: T }

proc Voice {
  outs { out1 }
  init {
    x = f64(1.0)
    y = f64(2.0)
    s = Pair(x, y)
  }
  sample {
    out1 = f32(s.a + s.b)
  }
}

outs { out1 }
init {
  v = Voice()
}
sample {
  out1 = v()
}
"#;

const GENERIC_PROC_TYPE_ARG_ARITY_ERROR_EXAMPLE: &str = r#"
proc Gain<T, U> {
  ins { in1: T }
  outs { out1: T }
  sample {
    out1 = in1
  }
}
outs { out1 }
init {
  p = Gain<f64>()
}
sample {
  out1 = f32(p(f64(2.0)))
}
"#;

const NON_GENERIC_PROC_WITH_TYPE_ARGS_ERROR_EXAMPLE: &str = r#"
proc Gain {
  ins { in1 }
  outs { out1 }
  sample {
    out1 = in1
  }
}
outs { out1 }
init {
  p = Gain<f64>()
}
sample {
  out1 = p(2.0)
}
"#;

const GENERIC_PROC_MULTIPLE_SPECIALIZATIONS_OK_EXAMPLE: &str = r#"
proc Gain<T> {
  ins { in1: T }
  outs { out1: T }
  params { g: T = 1.0 }
  sample {
    out1 = in1 * g
  }
}
outs { out1 }
init {
  p1 = Gain<f32>(g = 2.0)
  p2 = Gain<f64>(g = f64(0.25))
}
sample {
  out1 = p1(1.0) + f32(p2(f64(2.0)))
}
"#;

const GENERIC_PROC_ARRAY_DECL_TYPES_OK_EXAMPLE: &str = r#"
proc Mix<T> {
  ins { in1: T[2] }
  outs { out1: T }
  params { gains: T[2] = [1.0, 0.5] }
  sample {
    out1 = in1[0] * gains[0] + in1[1] * gains[1]
  }
}
outs { out1 }
init {
  p = Mix<f64>()
}
sample {
  out1 = f32(p([f64(2.0), f64(4.0)]))
}
"#;

const GENERIC_PROC_INIT_TYPED_ARRAY_GENERIC_OK_EXAMPLE: &str = r#"
proc Sum2<T> {
  outs { out1: T }
  init {
    x: T[2]
    x[0.0] = 1.0
    x[1.0] = 2.0
  }
  sample {
    out1 = x[0.0] + x[1.0]
  }
}
outs { out1 }
init {
  p = Sum2<f64>()
}
sample {
  out1 = f32(p())
}
"#;

const GENERIC_PROC_BUFFER_DECL_TYPE_COMPILES_EXAMPLE: &str = r#"
buffers { buf1: buffer<f64> }
proc Tap<T> {
  buffers { line: buffer<T> }
  outs { out1: T }
  sample {
    out1 = line[0]
  }
}
outs { out1 }
init {
  p = Tap<f64>(line = buf1)
}
sample {
  out1 = f32(p())
}
"#;

const FIRST_ASSIGNMENT_FROM_DEF_RETURN_AND_ALIAS_EXAMPLE: &str = r#"
outs { out1 }
def mydef() {
  return f64(0.5)
}
init {
  x = mydef() * 2
  z = x
  x = x + f64(0.25)
  z = z + f64(0.25)
}
sample {
  out1 = f32(z)
}
"#;

const FIRST_ASSIGNMENT_INT_IS_STICKY_ERROR_EXAMPLE: &str = r#"
outs { out1 }
init {
  x = 0
  x = 1.5
}
sample {
  out1 = 0.0
}
"#;

const PROC_FIRST_ASSIGNMENT_FROM_DEF_RETURN_EXAMPLE: &str = r#"
def mydef() {
  return f64(0.5)
}
proc AutoTypeProc {
  outs { out1 }
  init {
    x = mydef() * 2
    z = x
    x = x + f64(0.25)
    z = z + f64(0.25)
  }
  sample {
    out1 = f32(z)
  }
}
outs { out1 }
init {
  p = AutoTypeProc()
}
sample {
  out1 = p()
}
"#;

const TYPED_I32_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_i32() {
  x: i32 = i32(40)
  return f32(x)
}
init {
  xi: i32 = i32(10)
}
block {
  xb: i32 = i32(20)
  sample {
    xs: i32 = i32(30)
    out1 = f32(xi) + f32(xb) + f32(xs) + local_i32()
  }
}
"#;

const TYPED_F64_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_f64() {
  x: f64 = f64(4.0)
  return x
}
init {
  xi: f64 = f64(1.0)
}
block {
  xb: f64 = f64(2.0)
  sample {
    xs: f64 = f64(3.0)
    out1 = f32(xi + xb + xs + local_f64())
  }
}
"#;

const TYPED_I64_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_i64() {
  x: i64 = i64(40)
  return f32(x)
}
init {
  xi: i64 = i64(10)
}
block {
  xb: i64 = i64(20)
  sample {
    xs: i64 = i64(30)
    out1 = f32(xi) + f32(xb) + f32(xs) + local_i64()
  }
}
"#;

const TYPED_BOOL_DECLARATIONS_ALL_PATHS_EXAMPLE: &str = r#"
outs { out1 }
def local_bool_gate() {
  x: bool = true
  if (x) { return 1.0 } else { return 0.0 }
}
init {
  bi: bool = true
}
block {
  bb: bool = false
  sample {
    bs: bool = true
    if (bi && bs && (bb == false) && (local_bool_gate() > 0.5)) {
      out1 = 1.0
    } else {
      out1 = 0.0
    }
  }
}
"#;
