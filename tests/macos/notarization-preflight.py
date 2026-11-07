#!/usr/bin/env python3
"""Probe notarization prerequisites locally; no Apple account or secrets required."""

import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import sys
import wave


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "target/notarization-preflight"
EXPECTED_TESTS = {
    "math_policy", "vector_math", "slice_optimization", "target_initialization",
    "onda_ui", "onda_gpu", "onda_scope",
}


def run(directory, label, arguments, *, check=True, timeout=300):
    print(f"::group::{directory.name}: {label}", flush=True)
    result = subprocess.run(
        [str(argument) for argument in arguments], cwd=ROOT,
        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=timeout,
    )
    output = result.stdout.decode(errors="replace")
    (directory / f"{label}.log").write_text(output)
    print(output, end="", flush=True)
    print("::endgroup::", flush=True)
    if check and result.returncode != 0:
        raise RuntimeError(f"{label} exited with {result.returncode}")
    return result


def sign(directory, binary, entitlements, *, hardened=True):
    arguments = ["codesign", "--force", "--sign", "-", "--timestamp=none"]
    if hardened:
        arguments += ["--options", "runtime"]
    if entitlements is not None:
        arguments += ["--entitlements", entitlements]
    run(directory, f"{binary.name}-sign", [*arguments, binary])
    run(directory, f"{binary.name}-verify", ["codesign", "--verify", "--strict", binary])
    details = run(directory, f"{binary.name}-signature", ["codesign", "-dvvv", binary])
    has_runtime = re.search(rb"flags=.*\([^\n]*\bruntime\b", details.stdout) is not None
    if has_runtime != hardened:
        raise RuntimeError(f"Unexpected hardened-runtime flags for {binary}")


def render(directory, cli, name, *, check=True):
    output = directory / f"{name}.wav"
    result = run(directory, name, [
        cli, "run", "render", "examples/basic/sine.onda", "--dur", "0.25",
        "--sample-rate", "48000", "--output", output,
    ], check=check)
    return result, output


def control():
    directory = OUTPUT / "control"
    directory.mkdir(exist_ok=True)
    cli = directory / "onda"
    shutil.copy2(ROOT / "target/release/onda", cli)
    sign(directory, cli, None, hardened=False)
    _, baseline = render(directory, cli, "baseline")
    with wave.open(str(baseline), "rb") as audio:
        if (audio.getframerate(), audio.getnframes(), audio.getsampwidth()) != (48000, 12000, 2):
            raise RuntimeError("Baseline render has unexpected audio dimensions")
        if not any(audio.readframes(audio.getnframes())):
            raise RuntimeError("Baseline render is silent")

    # First prove the same executable runs, then fails specifically on its JIT path.
    sign(directory, cli, None)
    run(directory, "hardened-startup", [cli, "--version"])
    result, _ = render(directory, cli, "jit-without-entitlement", check=False)
    if result.returncode == 0:
        raise RuntimeError("JIT worked without an exception; this runner is not a valid control")
    print(f"Hardened runtime blocked unentitled JIT (exit {result.returncode}).", flush=True)
    return baseline.read_bytes()


def test_executables():
    binaries = {}
    for manifest in ("compiler-tests.jsonl", "ui-tests.jsonl"):
        for line in (OUTPUT / manifest).read_text().splitlines():
            message = json.loads(line)
            if (message.get("reason") == "compiler-artifact"
                    and message.get("executable") and message["profile"]["test"]):
                binaries[message["target"]["name"]] = Path(message["executable"])
    if binaries.keys() != EXPECTED_TESTS:
        raise RuntimeError(f"Expected {EXPECTED_TESTS}; found {set(binaries)}")
    return binaries


def profile(name, binaries, baseline):
    directory = OUTPUT / name
    directory.mkdir(exist_ok=True)
    entitlements = directory / "entitlements.plist"
    entitlements.write_bytes(plistlib.dumps({f"com.apple.security.cs.{name}": True}))
    failures = []

    def check(label, action):
        try:
            action()
        except (RuntimeError, subprocess.TimeoutExpired) as error:
            failures.append(f"{label}: {error}")
            print(f"::warning::{name}: {label}: {error}", flush=True)

    def cli_render():
        cli = directory / "onda"
        shutil.copy2(ROOT / "target/release/onda", cli)
        sign(directory, cli, entitlements)
        run(directory, "startup", [cli, "--version"])
        _, output = render(directory, cli, "render")
        if output.read_bytes() != baseline:
            raise RuntimeError("Signed render differs from the unhardened baseline")

    check("CLI render", cli_render)
    for name, source in binaries.items():
        def execute_test(name=name, source=source):
            binary = directory / name
            shutil.copy2(source, binary)
            sign(directory, binary, entitlements)
            run(directory, name, [binary, "--test-threads=1", "--nocapture"], timeout=600)
        check(name, execute_test)

    # Entitlements belong to the host process, rather than to these shared libraries.
    for name in ("libonda.dylib", "libonda_ui.dylib"):
        def sign_library(name=name):
            binary = directory / name
            shutil.copy2(ROOT / "target/release" / name, binary)
            sign(directory, binary, None, hardened=False)
        check(name, sign_library)
    return failures


def main():
    if sys.platform != "darwin":
        raise SystemExit("Run this preflight on macOS")
    os.chdir(ROOT)
    OUTPUT.mkdir(parents=True, exist_ok=True)
    binaries = test_executables()
    baseline = control()
    results = {
        name: profile(name, binaries, baseline)
        for name in ("allow-jit", "allow-unsigned-executable-memory")
    }
    (OUTPUT / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    summary = [
        "# macOS notarization preflight", "",
        "Ad hoc signatures only; no Developer ID, notarization, or Gatekeeper approval tested.",
        "", "The control rendered successfully, then hardened runtime blocked JIT without an exception.",
        "", "| Executable-memory exception | Result |", "| --- | --- |",
    ]
    for name, failures in results.items():
        summary.append(f"| `{name}` | {'FAIL' if failures else 'PASS'} |")
    for name, failures in results.items():
        for failure in failures:
            summary.append(f"\n- `{name}`: {failure}")
    summary += [
        "", "Checks cover release CLI rendering, native compiler/JIT suites, GPU UI library tests,",
        "and shared-library signing. They do not exercise an interactive app launch, audio hardware,",
        "a Developer ID trust chain, or the complete release app bundle.", "",
    ]
    report = "\n".join(summary)
    (OUTPUT / "summary.md").write_text(report)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as stream:
            stream.write(report)
    print(report, flush=True)
    if all(results.values()):
        raise SystemExit("Neither executable-memory exception passed all checks")


if __name__ == "__main__":
    main()
