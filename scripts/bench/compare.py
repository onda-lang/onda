"""Compare saved runs, including typed startup and warmed output snapshots."""
import argparse
import json
import math
from pathlib import Path
import struct

FORMATS = {"f32": "f", "f64": "d", "i32": "i", "i64": "q", "bool": "B"}
OUTPUT_SETTINGS = ("sample_rate", "block_size", "input_seed", "input_pattern", "warmup_blocks", "validation_blocks")


def read_samples(path, types, block_size, blocks):
    data = path.read_bytes()
    expected = blocks * block_size * sum(struct.calcsize("<" + FORMATS[t]) for t in types)
    if len(data) != expected:
        raise ValueError(f"{path}: {len(data)} bytes, expected {expected}")
    offset = 0
    for _ in range(blocks):
        for ty in types:
            size = block_size * struct.calcsize("<" + FORMATS[ty])
            for value in struct.unpack_from("<" + FORMATS[ty] * block_size, data, offset):
                yield ty, value
            offset += size


def output_difference(actual, reference, types, config, atol, rtol, bitwise=False):
    maximum = 0.0
    mismatches = 0
    count = 0
    for (ty, a), (_, b) in zip(read_samples(actual, types, config["block_size"], config["validation_blocks"]),
                               read_samples(reference, types, config["block_size"], config["validation_blocks"]), strict=True):
        if not math.isfinite(a) or not math.isfinite(b):
            raise ValueError("nonfinite output in comparison")
        error = abs(a - b)
        allowed = atol + rtol * max(abs(a), abs(b)) if ty in {"f32", "f64"} else 0
        different_bits = bitwise and struct.pack("<" + FORMATS[ty], a) != struct.pack("<" + FORMATS[ty], b)
        mismatches += different_bits or error > allowed
        maximum = max(maximum, error)
        count += 1
    return {"samples": count, "max_abs": maximum, "mismatches": mismatches, "equivalent": mismatches == 0}


def compare_runs(baseline, candidate, atol=1e-6, rtol=1e-6, bitwise=False, case_pairs=None):
    baseline, candidate = Path(baseline), Path(candidate)
    old_metadata = json.loads((baseline / "metadata.json").read_text()) if (baseline / "metadata.json").exists() else {}
    metadata = json.loads((candidate / "metadata.json").read_text()) if (candidate / "metadata.json").exists() else {}
    result = []
    for backend in ("native", "wasm"):
        left_path, right_path = baseline / f"{backend}.json", candidate / f"{backend}.json"
        if not left_path.exists() or not right_path.exists():
            continue
        left = {row["name"]: row for row in json.loads(left_path.read_text())}
        right = {row["name"]: row for row in json.loads(right_path.read_text())}
        pairs = case_pairs if case_pairs is not None else [(name, name) for name in right]
        for old_name, name in pairs:
            if old_name not in left or name not in right:
                continue
            before, row = left[old_name], right[name]
            old_config = json.loads((baseline / f"{old_name}.config.json").read_text())
            config = json.loads((candidate / f"{name}.config.json").read_text())
            changes = {key: [old_config.get(key), config.get(key)] for key in config
                       if key not in {"source", "prefix", "mode"} and config.get(key) != old_config.get(key)}
            source_changed = before.get("source_fingerprint") != row.get("source_fingerprint")
            comparison = {"name": name, "baseline_name": old_name, "backend": backend, "settings_changed": changes,
                          "source_changed": source_changed, "baseline_ns_per_frame": before.get("ns_per_frame"),
                          "candidate_ns_per_frame": row.get("ns_per_frame")}
            if old_metadata and metadata:
                comparison["compiler_changed"] = old_metadata.get("worker", {}).get("sha256") != metadata.get("worker", {}).get("sha256")
                if backend == "wasm":
                    comparison["wasm_backend_changed"] = any(old_metadata.get(key) != metadata.get(key)
                        for key in ("wasm_backend_sha256", "package_lock_sha256"))
            for key in ("mir_sha256", "llvm_ir_sha256", "wasm_sha256"):
                if key in before and key in row:
                    comparison[f"{key.removesuffix('_sha256')}_changed"] = before[key] != row[key]
            if "ns_per_frame" in before and "ns_per_frame" in row:
                comparison["candidate_to_baseline"] = row["ns_per_frame"] / before["ns_per_frame"]
            if before.get("outputs") == row.get("outputs") and "outputs" in row and not any(k in changes for k in OUTPUT_SETTINGS):
                if config.get("max_output_abs") is not None or old_config.get("max_output_abs") is not None:
                    comparison["output_validation"] = "bounded-error diagnostics; compare each run's contract"
                else:
                    comparison["outputs"] = {extension: output_difference(
                        candidate / f"{name}.{backend}.{extension}", baseline / f"{old_name}.{backend}.{extension}",
                        row["outputs"], config, atol, rtol, bitwise) for extension in ("output.bin", "warm-output.bin")}
            else:
                comparison["output_validation"] = "incompatible output settings or inspection-only run"
            result.append(comparison)
    if not result:
        raise ValueError("runs have no matching cases/backends")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline", type=Path)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--atol", type=float, default=1e-6)
    parser.add_argument("--rtol", type=float, default=1e-6)
    parser.add_argument("--bitwise", action="store_true", help="require identical floating-point bits, including signed zero")
    parser.add_argument("--case-pair", action="append", help="BASELINE_NAME:CANDIDATE_NAME; repeat to compare source variants")
    args = parser.parse_args()
    if not all(math.isfinite(x) and x >= 0 for x in (args.atol, args.rtol)):
        parser.error("tolerances must be finite and nonnegative")
    pairs = None
    if args.case_pair:
        pairs = [pair.split(":") for pair in args.case_pair]
        if any(len(pair) != 2 or not all(pair) for pair in pairs):
            parser.error("case pairs must have the form BASELINE_NAME:CANDIDATE_NAME")
    rows = compare_runs(args.baseline, args.candidate, args.atol, args.rtol, args.bitwise, pairs)
    print(json.dumps(rows, indent=2))


if __name__ == "__main__":
    main()
