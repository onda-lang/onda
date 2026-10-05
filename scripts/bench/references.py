"""Validate supported builtin workloads against independent C/FFTW kernels."""
from pathlib import Path
from compare import read_samples


def reference_result(name, reference, native, config):
    prefix = Path(config["prefix"])
    if native["outputs"] != ["f32"]:
        raise ValueError("C/FFTW references expect a single f32 output")
    if name.startswith("fft-real-"):
        expected = next(read_samples(Path(f"{prefix}.native.output.bin"), ["f32"], config["block_size"], config["validation_blocks"]))[1]
        error = abs(expected - reference["check"])
        limit = 1e-4 + 1e-4 * abs(reference["check"])
        coverage = "real part of bin 7; use fft-accuracy cases for full-spectrum checks"
    else:
        error = 0.0
        for extension in ("output.bin", "warm-output.bin"):
            expected = read_samples(Path(f"{prefix}.native.{extension}"), ["f32"], config["block_size"], config["validation_blocks"])
            actual = read_samples(Path(f"{prefix}-reference.{extension}"), ["f32"], config["block_size"], config["validation_blocks"])
            error = max(error, max(abs(a[1] - b[1]) for a, b in zip(actual, expected, strict=True)))
        limit = 2e-6
        coverage = "startup and warmed scalar filter samples"
    if not error <= limit:
        raise ValueError(f"{name}: independent reference error {error} exceeds {limit}")
    return {"name": name, **reference, "native_to_reference": native["ns_per_frame"] / reference["ns_per_frame"],
            "validation_max_abs": error, "coverage": coverage}
