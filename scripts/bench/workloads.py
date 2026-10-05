"""Small, isolated DSP and compiler workloads; no source rewriting."""
import math
import struct

def fft_accuracy_source(n, width):
    def signal(seed):
        result = []
        for _ in range(n):
            seed = (seed * 1664525 + 1013904223) & 0xffffffff
            value = (seed >> 8) / 16777216.0 * 2.0 - 1.0
            if width == "f32":
                value = struct.unpack("f", struct.pack("f", value))[0]
            result.append(value)
        return result

    real, imag = signal(12345), signal(67890)
    def dft(imaginary):
        re, im = [], []
        for k in range(n):
            terms = [(math.cos(-math.tau * k * j / n), math.sin(-math.tau * k * j / n))
                     for j in range(n)]
            re.append(math.fsum(real[j] * c - imaginary[j] * s for j, (c, s) in enumerate(terms)))
            im.append(math.fsum(real[j] * s + imaginary[j] * c for j, (c, s) in enumerate(terms)))
        return re, im
    expected_re, expected_im = dft(imag)
    expected_rr, expected_ri = dft([0.0] * n)
    arrays = {"real": real, "imag": imag, "expected_re": expected_re, "expected_im": expected_im,
              "expected_rr": expected_rr, "expected_ri": expected_ri}
    declarations = "\n".join(f"  {name}: {'f64' if name.startswith('expected_') else width}[N] = ["
                              + ", ".join(f"{x:.17f}" for x in values) + "]"
                              for name, values in arrays.items())
    # Expose scaled errors so an accidental f32 coefficient in an f64 FFT
    # remains observable through the benchmark runner's f32 output ports.
    scale = 1e9 if width == "f64" else 1e4
    return f"""import std/fft
const N = {n}
outs 6
init:
{declarations}
  fft = std::fft<N>::FFT<{width}>()
  fft.forward_complex(real, imag)
  error_re: f64[N]
  error_im: f64[N]
  for i in 0..N:
    error_re[i] = f64(fft.real(i)) - expected_re[i]
    error_im[i] = f64(fft.imag(i)) - expected_im[i]
  fft.inverse()
  full = std::fft<N>::FFT<{width}>()
  full.forward_real(real)
  index = 0
sample:
  out1 = f32(error_re[index] * f64({scale:.1f}))
  out2 = f32(error_im[index] * f64({scale:.1f}))
  out3 = f32((f64(fft.real(index)) - f64(real[index])) * f64({scale:.1f}))
  out4 = f32((f64(fft.imag(index)) - f64(imag[index])) * f64({scale:.1f}))
  out5 = f32((f64(full.real(index)) - expected_rr[index]) * f64({scale:.1f}))
  out6 = f32((f64(full.imag(index)) - expected_ri[index]) * f64({scale:.1f}))
  index += 1
  if index >= N:
    index = 0
"""


def processor(module, name, arguments="", inputs=1, init_extra="", call_extra=""):
    signal = ", ".join(f"in{i + 1}" for i in range(inputs))
    invocation = ", ".join(x for x in [signal, call_extra] if x)
    channels = 2 if module in ("dynamics", "reverb") and name not in ("PeakFollower", "RmsFollower") else 1
    output = "out1, out2" if channels == 2 else "out1"
    ports = f"ins {inputs}\n" if inputs else ""
    return (f"import std/{module.split('<')[0]}\n\n{ports}"
            f"init:\n  p = std::{module}::{name}({arguments})\n{init_extra}"
            f"sample:\n  {output} = p({invocation})\n")


def scenarios():
    cases = {"passthrough": "ins 1\nsample:\n  out1 = in1\n"}
    for name in ["Sine", "Phasor", "Saw", "Pulse", "Triangle"]:
        cases[f"osc-{name.lower()}"] = processor("osc", name, inputs=0)
    cases["osc-sine-fm"] = processor("osc", "Sine", inputs=1).replace("p(in1)", "p(freq = 440.0 + in1 * 100.0)")
    cases["osc-sin-libm"] = "init:\n  phase = 0.0\nsample:\n  out1 = sin(phase)\n  phase += TWO_PI * 440.0 / SR\n  if phase >= TWO_PI:\n    phase -= TWO_PI\n"
    for name in ["OnePole", "DCBlock", "Resonator", "Svf"]:
        cases[f"filter-{name.lower()}"] = processor("filter", name)
    cases["filter-biquad"] = processor("filter", "Biquad", init_extra="  p.set_coefficients(std::filter::design_lowpass(1000.0, 0.707107))\n")
    cases["filter-svf-block-update"] = cases["filter-svf"].replace("sample:\n", "block:\n  p.update_coeffs(1000.0, 0.707107)\n  sample:\n").replace("  out1 =", "    out1 =")
    cases["filter-svf-audio-update"] = cases["filter-svf"].replace("  out1 =", "  p.update_coeffs(1000.0 + in1 * 500.0, 0.707107)\n  out1 =")
    for name in ["PeakFollower", "RmsFollower", "Compressor", "Limiter", "Gate"]:
        cases[f"dynamics-{name.lower()}"] = processor("dynamics", name, inputs=1 if "Follower" in name else 2)
    cases["dynamics-compressor-quiet"] = cases["dynamics-compressor"].replace("p(in1, in2)", "p(in1 * 0.01, in2 * 0.01)")
    for name in ["Lag", "LagUD", "Slew"]:
        cases[f"smoothing-{name.lower()}"] = processor("smoothing", name)
    for capacity in [8192, 96000]:
        for name in ["Integer", "Linear", "Cubic"]:
            cases[f"delay-{name.lower()}-{capacity}"] = processor(f"delay<{capacity}>", name, "delay_samples = 1024.25" if name != "Integer" else "delay_samples = 1024")
    for name in ["Smooth", "Crossfade", "Delay", "CrossfadeDelay"]:
        cases[f"delay-{name.lower()}"] = processor("delay<8192>", name)
    cases["reverb-schroeder"] = processor("reverb", "Schroeder::Reverb", inputs=2)
    cases["pitch-dualwindow"] = processor("pitch_shift", "DualWindow")
    for name in ["White", "Pink", "Brown"]:
        cases[f"noise-{name.lower()}"] = processor("noise", name, inputs=0)
    cases["env-adsr-sustain"] = processor("env", "ADSR", inputs=0, init_extra="  p.start()\n")
    for name in ["Constant", "Db", "Smoothed", "SmoothedDb"]:
        cases[f"gain-{name.lower()}"] = processor("gain", name)
    cases["mix-crossfade"] = processor("mix", "Crossfade", inputs=2)
    cases["levels-pan3db-audio"] = "import std/levels\nins 1\nsample:\n  out1, out2 = std::levels::pan_3db(in1)\n"
    for name, expression in [("dbamp", "dbamp(in1 * 24.0)"), ("levels-exp", "std::levels::db_to_gain(in1 * 24.0)"),
                             ("midicps", "midicps(in1 * 24.0 + 69.0)")]:
        cases[f"math-{name}"] = f"import std/levels\nins 1\nsample:\n  out1 = {expression}\n"
    for n in [256, 1024, 4096, 16384]:
        for width in ["f32", "f64"]:
            cases[f"fft-real-{width}-{n}"] = (f"import std/fft\nconst N = {n}\ninit:\n"
                f"  fft = std::fft<N>::RealTransform<{width}>()\n  values: {width}[N]\n"
                "  for i in 0..N:\n    values[i] = sin(f32(i) * 0.017) + 0.25 * cos(f32(i) * 0.061)\n"
                "block:\n  fft.forward(values)\n  sample:\n    out1 = f32(fft.real(7))\n")
    for n in [1024, 4096]:
        cases[f"fft-stft-{n}"] = cases[f"fft-real-f32-{n}"].replace("RealTransform<f32>", "STFT<f32>").replace("fft.forward(values)", "fft.forward_real(values)")
        cases[f"fft-stream-roundtrip-{n}"] = (f"import std/fft\nins 1\ninit:\n"
            f"  fft = std::fft<{n}>::RealFFT()\n  ifft = std::fft<{n}>::RealIFFT()\n  spectrum: f32[{n}]\n"
            "sample:\n  if fft.push(in1):\n    fft.store_real_packed(spectrum)\n    ifft.load_packed(spectrum)\n  out1 = ifft.tick()\n")
    for n, length, name in [(256, 32, "TimeDomainConvolver"), (256, 128, "TimeDomainConvolver"),
                             (256, 512, "TimeDomainConvolver"), (256, 4096, "BlockConvolver"),
                             (256, 48000, "ZeroLatencyConvolver"), (4096, 48000, "ZeroLatencyConvolver"),
                             (16384, 48000, "ZeroLatencyConvolver")]:
        source = processor(f"convolution<{n}, {length}>", name)
        source = source.replace("init:\n", f"init:\n  impulse: f32[{length}]\n  for i in 0..{length}:\n    impulse[i] = exp(-6.0 * f32(i) / {length}) * 0.001\n")
        source = source.replace("sample:\n", "  p.set_impulse(impulse)\nsample:\n")
        if name != "TimeDomainConvolver":
            source = source.replace("  p.set_impulse", "  p.set_offset(0)\n  p.set_impulse")
        cases[f"convolution-{name.lower()}-{n}-{length}"] = source
    return cases


def array_probe(width, expression, reduction=False):
    calculation = (f"  total: {width} = 0.0\n  for i in 0..N:\n    total += values[i] * values[i]\n"
                   if reduction else f"  for i in 0..N:\n    results[i] = {expression}\n")
    return (f"const N = 512\nparams:\n  factor: {width} = 1.37\n"
            f"outs<{width}> 1\ninit:\n  values: {width}[N]\n  results: {width}[N]\n"
            f"  for i in 0..N:\n    values[i] = {width}(sin(f64(i) * 0.017))\n"
            "  index = 0\nblock:\n" + calculation + "  sample:\n"
            + ("    out1 = total\n" if reduction else
               "    out1 = results[index]\n    index += 1\n    if index >= N:\n      index = 0\n"))


def catalog():
    # Factories defer expensive DFT generation until a case is actually selected.
    cases = {name: {"source": source, "warmup_blocks": 8192 if name.startswith("convolution") else 512}
             for name, source in scenarios().items()}
    for name, case in cases.items():
        if name.startswith("fft-real-") or name in {"passthrough", "filter-onepole", "filter-dcblock", "filter-biquad", "filter-svf"}:
            case["reference"] = name
    for n in [256, 512]:
        for width in ["f32", "f64"]:
            cases[f"fft-accuracy-{width}-{n}"] = {
                "source": lambda n=n, width=width: fft_accuracy_source(n, width),
                "max_output_abs": 0.2,
                "warmup_blocks": 512,
            }
    for width in ["f32", "f64"]:
        for name, expression in {
            "arithmetic": "values[i] * factor + values[i] * values[i]",
            "division": "values[i] / factor",
            "reciprocal": "values[i] * (1.0 / factor)",
        }.items():
            cases[f"operator-{name}-{width}"] = {
                "source": lambda width=width, expression=expression: array_probe(width, expression),
                "warmup_blocks": 512,
            }
        cases[f"operator-reduction-{width}"] = {
            "source": lambda width=width: array_probe(width, "", reduction=True),
            "warmup_blocks": 512,
        }
    return cases
