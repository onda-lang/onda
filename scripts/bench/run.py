#!/usr/bin/env python3
"""Bounded Onda/native/Wasm benchmarks and compiler inspection. See README.md."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys
import tempfile

from compare import compare_runs
from inspection import llvm_summary
from limits import Limits
from workloads import catalog

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def write_json(path, value):
    contents = json.dumps(value, indent=2, allow_nan=False) + "\n"
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=f".{path.name}-", delete=False) as file:
            temporary = Path(file.name)
            file.write(contents)
            file.flush()
            os.fsync(file.fileno())
        os.replace(temporary, path)
        descriptor = os.open(path.parent, os.O_DIRECTORY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def digest(path):
    checksum = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            checksum.update(block)
    return checksum.hexdigest()


def fingerprint(prefix, source):
    path = prefix.with_suffix(".sources.json")
    if not path.exists():
        return digest(source)
    manifest = json.loads(path.read_text())
    # Paths vary between run directories. Compare the exact compiled contents.
    hashes = {section: [hashlib.sha256(d["contents"].encode()).hexdigest() for d in documents]
              for section, documents in manifest.items()}
    write_json(prefix.with_suffix(".source-hashes.json"), hashes)
    return hashlib.sha256(json.dumps(hashes, sort_keys=True).encode()).hexdigest()


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument("--source", type=Path, action="append", help="Onda or MIR file; repeat for multiple cases")
    selection.add_argument("--filter", help="builtin-name substring; separate alternatives with |")
    selection.add_argument("--all", action="store_true", help="explicit complete sequential sweep")
    parser.add_argument("--list", action="store_true", help="list builtin cases without executing")
    parser.add_argument("--output", type=Path, help="new or empty artifact directory")
    parser.add_argument("--backend", choices=("native", "wasm", "both"), default="native")
    parser.add_argument("--inspect-only", action="store_true", help="compile artifacts without initializing/executing DSP")
    parser.add_argument("--ir", action="store_true", help="save readable MIR, LLVM IR and Wasm WAT")
    parser.add_argument("--assembly", action="store_true", help="save host object, metadata and disassembly")
    parser.add_argument("--trust-mir", action="store_true", help="retain proofs in compiler-produced MIR")
    parser.add_argument("--sample-rate", type=float, default=48000)
    parser.add_argument("--block-size", type=int, default=128)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--round-ms", type=float, default=10)
    parser.add_argument("--warmup-blocks", type=int, help="override case warmup (default 512; convolution 8192)")
    parser.add_argument("--validation-blocks", type=int, default=8)
    parser.add_argument("--latency-blocks", type=int, default=1024)
    parser.add_argument("--input-seed", type=int, default=12345)
    parser.add_argument("--input-pattern", choices=("noise", "ramp", "zero", "impulse"), default="noise")
    parser.add_argument("--atol", type=float, default=1e-6)
    parser.add_argument("--rtol", type=float, default=1e-6)
    parser.add_argument("--bitwise", action="store_true", help="require identical float bits in parity/comparisons")
    parser.add_argument("--max-output-abs", type=float, help="validate custom error-output workloads against this bound")
    parser.add_argument("--fast-math", action="store_true", help="explicitly permit relaxed floating-point optimization")
    parser.add_argument("--opt-level", type=int, choices=range(4), default=3)
    parser.add_argument("--wasm-opt-level", type=int, choices=range(5), default=4)
    parser.add_argument("--cpu", type=int, default=min(os.sched_getaffinity(0)) if hasattr(os, "sched_getaffinity") else 0)
    parser.add_argument("--memory-mib", type=int, default=2048)
    parser.add_argument("--timeout-seconds", type=int, default=30)
    parser.add_argument("--node-heap-mib", type=int, default=256)
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
    parser.add_argument("--runner", type=Path, default=target / "release/examples/benchmark_dsp")
    parser.add_argument("--references", action="store_true", help="compare supported builtins with independent C/FFTW kernels")
    parser.add_argument("--compare-to", type=Path, help="compare timings and typed outputs against a saved run")
    parser.add_argument("--label", default="", help="free-form experiment description recorded in metadata")
    args = parser.parse_args()
    if args.list:
        return args
    if not (args.source or args.filter or args.all) or args.output is None:
        parser.error("select --source, --filter or --all and provide --output")
    for key in ("sample_rate", "block_size", "repetitions", "round_ms", "validation_blocks", "latency_blocks", "memory_mib", "timeout_seconds", "node_heap_mib"):
        value = getattr(args, key)
        if not math.isfinite(value) or value <= 0:
            parser.error(f"{key} must be finite and positive")
    if args.block_size > 2**31 - 1 or args.sample_rate > 3.4028234663852886e38:
        parser.error("sample rate or block size exceed the processor configuration range")
    if args.warmup_blocks is not None and args.warmup_blocks < 0:
        parser.error("warmup must be nonnegative")
    if not 0 <= args.input_seed <= 0xffffffff:
        parser.error("input seed must fit u32")
    for key in ("atol", "rtol", "max_output_abs"):
        value = getattr(args, key)
        if value is not None and (not math.isfinite(value) or value < 0):
            parser.error(f"{key} must be finite and nonnegative")
    if hasattr(os, "sched_getaffinity") and args.cpu not in os.sched_getaffinity(0):
        parser.error("selected CPU is outside this process's allowed affinity")
    if args.node_heap_mib >= args.memory_mib:
        parser.error("Node heap must leave room within the process memory limit")
    if not args.runner.is_file():
        parser.error(f"build the worker first: cargo build --release -j1 -p onda_examples --features llvm-orc --example benchmark_dsp ({args.runner})")
    if args.references and (args.inspect_only or args.backend == "wasm" or args.fast_math or args.input_seed != 12345 or args.input_pattern != "noise"):
        parser.error("C/FFTW references require a strict native run with default noise input/seed")
    if args.backend != "native" and not shutil.which("systemd-run"):
        parser.error("bounded V8 execution requires Linux systemd user services")
    if args.assembly and not (shutil.which("llvm-objdump") or shutil.which("objdump")):
        parser.error("--assembly requires llvm-objdump or objdump on PATH")
    return args


def selected_cases(args):
    if args.source:
        cases = {}
        for source in args.source:
            source = source.resolve()
            if not source.is_file():
                raise ValueError(f"source does not exist: {source}")
            name = re.sub(r"[^A-Za-z0-9_-]", "-", source.name.removesuffix(".msgpack").removesuffix(".json").removesuffix(".mir").removesuffix(".onda").removesuffix(".on"))
            if not name or name in cases:
                raise ValueError(f"source names must be unique: {source}")
            cases[name] = {"path": source, "warmup_blocks": 512}
        return cases
    cases = catalog()
    if args.filter:
        parts = args.filter.split("|")
        if any(not p for p in parts):
            raise ValueError("filter alternatives must be nonempty")
        cases = {name: case for name, case in cases.items() if any(p in name for p in parts)}
    if not cases:
        raise ValueError("selection matched no workloads")
    return cases


def main():
    args = parse_args()
    if args.list:
        print("\n".join(catalog()))
        return
    cases = selected_cases(args)
    if args.bitwise and (args.max_output_abs is not None or any(case.get("max_output_abs") is not None for case in cases.values())):
        raise ValueError("bitwise parity does not apply to bounded-error diagnostics")
    if args.references and not any("reference" in case for case in cases.values()):
        raise ValueError("selection contains no independent C/FFTW reference cases")
    directory = args.output.resolve()
    if directory.exists() and any(directory.iterdir()):
        raise ValueError("output directory must be empty; use a new directory for each experiment")
    directory.mkdir(parents=True, exist_ok=True)
    limits = Limits(args.cpu, args.memory_mib, args.timeout_seconds)
    env = os.environ.copy()
    env.setdefault("RUST_MIN_STACK", "16777216")
    settings = {key: str(value.resolve()) if isinstance(value, Path) else value for key, value in vars(args).items()}
    if args.source:
        settings["source"] = [str(path.resolve()) for path in args.source]
    metadata = {"schema_version": 1, "label": args.label, "settings": settings,
                "platform": platform.platform(), "machine": platform.machine(), "python": platform.python_version(),
                "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
                "worker": {"path": str(args.runner.resolve()), "sha256": digest(args.runner)},
                "tools_sha256": {str(p.relative_to(ROOT)): digest(p) for p in HERE.glob("*") if p.suffix in {".py", ".mjs", ".c"}},
                "execution": {"sequential": True, "native_memory": "RLIMIT_AS", "wasm_memory": "systemd MemoryMax; swap disabled",
                              "native_fp_mode": "Onda runtime default; FTZ/DAZ on x86", "wasm_fp_mode": "WebAssembly float semantics"},
                "cases": list(cases)}
    metadata["working_tree_dirty"] = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    if args.backend != "native":
        metadata["wasm_backend_sha256"] = {
            str(path.relative_to(ROOT)): digest(path)
            for path in sorted((ROOT / "packages/onda_binaryen_web/src").rglob("*.js"))
        }
        metadata["package_lock_sha256"] = digest(ROOT / "package-lock.json")
    cpuinfo = Path("/proc/cpuinfo")
    if cpuinfo.is_file():
        for section in cpuinfo.read_text().split("\n\n"):
            info = dict(line.split(":", 1) for line in section.splitlines() if ":" in line)
            info = {key.strip(): value.strip() for key, value in info.items()}
            if info.get("processor") == str(args.cpu):
                metadata["cpu_info"] = info
                break
    write_json(directory / "metadata.json", metadata)
    write_json(directory / "status.json", {"status": "running"})
    native_rows, wasm_rows, reference_rows = [], [], []
    try:
        reference_binary = None
        for name, case in cases.items():
            prefix = directory / name
            source = case.get("path")
            if source is None:
                source = prefix.with_suffix(".onda")
                factory = case["source"]
                source.write_text(factory() if callable(factory) else factory)
            else:
                shutil.copyfile(source, directory / f"{name}.input{''.join(source.suffixes)}")
            config = {"schema_version": 1, "source": str(source), "prefix": str(prefix),
                      "mode": "inspect" if args.inspect_only else "prepare" if args.backend == "wasm" else "bench",
                      "sample_rate": args.sample_rate, "block_size": args.block_size,
                      "repetitions": args.repetitions, "round_ms": args.round_ms,
                      "warmup_blocks": args.warmup_blocks if args.warmup_blocks is not None else case["warmup_blocks"],
                      "validation_blocks": args.validation_blocks, "latency_blocks": args.latency_blocks,
                      "input_seed": args.input_seed, "input_pattern": args.input_pattern,
                      "atol": args.atol, "rtol": args.rtol,
                      "bitwise": args.bitwise,
                      "opt_level": args.opt_level, "wasm_opt_level": args.wasm_opt_level,
                      "fast_math": args.fast_math, "ir": args.ir or args.inspect_only or args.assembly,
                      "assembly": args.assembly, "trust_mir": args.trust_mir,
                      "max_output_abs": args.max_output_abs if args.max_output_abs is not None else case.get("max_output_abs"),
                      "compare_native": args.backend == "both" and not args.inspect_only}
            config_path = directory / f"{name}.config.json"
            write_json(config_path, config)
            row = {"name": name, **json.loads(limits.run([args.runner.resolve(), config_path], directory / f"{name}-native", env=env))}
            row["source_fingerprint"] = fingerprint(prefix, source)
            row["mir_sha256"] = digest(prefix.with_suffix(".mir.msgpack"))
            if config["ir"]:
                row["llvm_ir_sha256"] = digest(prefix.with_suffix(".ll"))
            write_json(directory / f"{name}.native.json", row)
            if config["mode"] == "bench":
                for extension in ("output.bin", "warm-output.bin"):
                    (directory / f"{name}.{extension}").rename(directory / f"{name}.native.{extension}")
            if args.backend != "wasm":
                native_rows.append(row)
                write_json(directory / "native.json", native_rows)
                if "ns_per_frame" in row:
                    print(f"{name} native: {row['ns_per_frame']:.3f} ns/frame; p99 {row['block_p99_us']:.3f} us", flush=True)
            if config["ir"]:
                with prefix.with_suffix(".ll").open() as ir:
                    write_json(directory / f"{name}.inspection.json", llvm_summary(ir))
            if args.assembly:
                disassembler = shutil.which("llvm-objdump") or shutil.which("objdump")
                text = limits.run([disassembler, "-d", prefix.with_suffix(".o")], directory / f"{name}-disassemble", env=env)
                prefix.with_suffix(".asm").write_text(text)
            if args.backend in {"wasm", "both"}:
                wasm_row = {"name": name, **json.loads(limits.run([
                    "node", f"--max-old-space-size={args.node_heap_mib}", HERE / "wasm.mjs", config_path,
                ], directory / f"{name}-wasm", virtual_memory=True))}
                wasm_row["source_fingerprint"] = row["source_fingerprint"]
                wasm_row["mir_sha256"] = row["mir_sha256"]
                wasm_row["wasm_sha256"] = digest(prefix.with_suffix(".wasm"))
                wasm_rows.append(wasm_row)
                write_json(directory / "wasm.json", wasm_rows)
                if "ns_per_frame" in wasm_row:
                    print(f"{name} wasm: {wasm_row['ns_per_frame']:.3f} ns/frame; validation {wasm_row['validation']}", flush=True)
            if args.references and "reference" in case:
                from references import reference_result
                if reference_binary is None:
                    reference_binary = directory / "reference"
                    limits.run(["gcc", "-O3", "-march=native", "-ffp-contract=off", "-Wall", "-Wextra",
                                HERE / "reference.c", "-lfftw3f", "-lfftw3", "-lm", "-o", reference_binary], directory / "reference-build", env=env)
                reference = json.loads(limits.run([reference_binary, case["reference"], args.block_size,
                    directory / f"{name}-reference", args.sample_rate, args.repetitions, args.round_ms,
                    config["warmup_blocks"], args.validation_blocks], directory / f"{name}-reference-run", env=env))
                reference_rows.append(reference_result(name, reference, row, config))
                write_json(directory / "references.json", reference_rows)
        if args.compare_to:
            write_json(directory / "comparison.json", compare_runs(args.compare_to, directory, args.atol, args.rtol, args.bitwise))
        write_json(directory / "status.json", {"status": "completed"})
    except BaseException as error:
        write_json(directory / "status.json", {"status": "failed", "error": str(error)})
        raise
    print(f"Artifacts: {directory}")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        sys.exit(str(error))
