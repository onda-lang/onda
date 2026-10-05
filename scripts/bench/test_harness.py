"""Harness contract tests. Set ONDA_BENCH_INTEGRATION=1 for bounded native/Wasm checks."""
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from compare import compare_runs, output_difference, read_samples
from inspection import llvm_summary
from limits import Limits
from run import write_json
from workloads import catalog

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
CPU = min(os.sched_getaffinity(0))


class HarnessContracts(unittest.TestCase):
    def test_interrupted_result_replace_preserves_previous_json(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "results.json"
            write_json(path, {"completed": 1})
            with patch("run.os.replace", side_effect=OSError("interrupted")):
                with self.assertRaises(OSError):
                    write_json(path, {"completed": 2})
            self.assertEqual(json.loads(path.read_text()), {"completed": 1})
            self.assertEqual(list(path.parent.iterdir()), [path])

    def test_typed_snapshots_and_exact_large_integer_comparison(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            left, right = directory / "left", directory / "right"
            left.write_bytes(struct.pack("<dqB", 1.0, 9007199254740993, 1))
            right.write_bytes(struct.pack("<dqB", 1.0 + 1e-12, 9007199254740994, 1))
            result = output_difference(left, right, ["f64", "i64", "bool"],
                                       {"block_size": 1, "validation_blocks": 1}, 1e-14, 0)
            self.assertEqual(result["samples"], 3)
            self.assertEqual(result["mismatches"], 2)
            self.assertFalse(result["equivalent"])
            right.write_bytes(b"short")
            with self.assertRaises(ValueError):
                list(read_samples(right, ["f64"], 1, 1))

    def test_comparison_identifies_changed_source_settings_and_outputs(self):
        with tempfile.TemporaryDirectory() as temporary:
            directories = [Path(temporary) / x for x in ("before", "after")]
            for index, directory in enumerate(directories):
                directory.mkdir()
                row = {"name": "case", "outputs": ["f64"], "source_fingerprint": str(index), "ns_per_frame": 4 - index}
                (directory / "native.json").write_text(json.dumps([row]))
                (directory / "metadata.json").write_text(json.dumps({"worker": {"path": str(index), "sha256": "same-binary"}}))
                config = {"block_size": 1, "validation_blocks": 1, "sample_rate": 48000,
                          "warmup_blocks": 0, "input_seed": 1, "input_pattern": "noise", "opt_level": 2 + index}
                (directory / "case.config.json").write_text(json.dumps(config))
                for extension in ("output.bin", "warm-output.bin"):
                    (directory / f"case.native.{extension}").write_bytes(struct.pack("<d", 1 + index * 1e-12))
            result = compare_runs(*directories, atol=1e-14, rtol=0)[0]
            self.assertEqual(result["candidate_to_baseline"], 0.75)
            self.assertTrue(result["source_changed"])
            self.assertFalse(result["compiler_changed"])
            self.assertEqual(result["settings_changed"], {"opt_level": [2, 3]})
            self.assertFalse(result["outputs"]["warm-output.bin"]["equivalent"])
            directory = directories[1]
            (directory / "other.config.json").write_bytes((directory / "case.config.json").read_bytes())
            for extension in ("output.bin", "warm-output.bin"):
                (directory / f"other.native.{extension}").write_bytes((directory / f"case.native.{extension}").read_bytes())
            row["name"] = "other"
            (directory / "native.json").write_text(json.dumps([row]))
            variant = compare_runs(*directories, case_pairs=[("case", "other")])[0]
            self.assertEqual(variant["baseline_name"], "case")
            self.assertEqual(variant["name"], "other")

    def test_bitwise_comparison_preserves_signed_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            left, right = Path(temporary) / "left", Path(temporary) / "right"
            left.write_bytes(struct.pack("<d", -0.0))
            right.write_bytes(struct.pack("<d", 0.0))
            config = {"block_size": 1, "validation_blocks": 1}
            self.assertTrue(output_difference(left, right, ["f64"], config, 0, 0)["equivalent"])
            self.assertFalse(output_difference(left, right, ["f64"], config, 0, 0, bitwise=True)["equivalent"])

    def test_inspection_counts_instructions_without_counting_function_signature(self):
        result = llvm_summary('''define <8 x float> @kernel(ptr %p) {
  %v = load <8 x float>, ptr %p, align 1
  %r = fmul <8 x float> %v, %v
  %s = shufflevector <8 x float> %r, <8 x float> %r, <8 x i32> zeroinitializer
  store <8 x float> %s, ptr %p, align 16
  ret void
}
''')["functions"]["kernel"]
        self.assertEqual(result["vector_instruction_lines"], 4)
        self.assertEqual(result["align_1_accesses"], 1)
        self.assertEqual(result["shufflevector"], 1)

    def test_catalog_defers_accuracy_generation_and_has_no_source_rewrite(self):
        cases = catalog()
        self.assertTrue(callable(cases["fft-accuracy-f64-512"]["source"]))
        self.assertIn("operator-division-f64", cases)
        self.assertFalse(any("four-accumulators" in key for key in cases))

    def test_address_space_limit_stops_allocation_and_preserves_logs(self):
        with tempfile.TemporaryDirectory() as temporary:
            prefix = Path(temporary) / "child"
            with self.assertRaises(RuntimeError):
                Limits(CPU, memory_mib=128, timeout_seconds=5).run(
                    [sys.executable, "-c", "bytearray(256 * 1024 * 1024)"], prefix)
            self.assertIn("MemoryError", prefix.with_suffix(".stderr.log").read_text())
            self.assertTrue(prefix.with_suffix(".command.json").is_file())

    def test_timeout_terminates_descendants(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            script = directory / "parent.py"
            pidfile = directory / "child.pid"
            script.write_text("import subprocess, sys, time\nfrom pathlib import Path\n"
                              "p = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\n"
                              "Path(sys.argv[1]).write_text(str(p.pid))\ntime.sleep(60)\n")
            with self.assertRaises(subprocess.TimeoutExpired):
                Limits(CPU, memory_mib=128, timeout_seconds=1).run([sys.executable, script, pidfile], directory / "job")
            pid = int(pidfile.read_text())
            stat = Path(f"/proc/{pid}/stat")
            self.assertTrue(not stat.exists() or stat.read_text().split()[2] == "Z")


@unittest.skipUnless(os.environ.get("ONDA_BENCH_INTEGRATION") == "1", "enable bounded integration checks explicitly")
class Integration(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="bench-tests-", dir=ROOT / "target")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)

    def run_case(self, source, output, *options, expect_success=True):
        command = [sys.executable, HERE / "run.py", "--source", source, "--output", output,
                   "--block-size", "16", "--sample-rate", "44100", "--warmup-blocks", "3",
                   "--validation-blocks", "2", "--latency-blocks", "32", "--round-ms", "1",
                   "--repetitions", "1", "--cpu", str(CPU), *options]
        result = subprocess.run([str(x) for x in command], capture_output=True, text=True, timeout=60)
        if expect_success:
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(json.loads((output / "status.json").read_text())["status"], "completed")
        else:
            self.assertNotEqual(result.returncode, 0)
        return result

    def test_custom_imports_typed_ports_and_mir_replay(self):
        source = self.directory / "typed.onda"
        (self.directory / "helper.onda").write_text("def scale(x: f64):\n  return x * f64(0.125)\n")
        source.write_text('''include "helper.onda"
ins:
  a: f32
  b: f64
  c: i32
  d: i64
  e: bool
outs:
  x: f32
  y: f64
  z: i32
  w: i64
  flag: bool
sample:
  x = a * 0.25 + 1.0
  y = scale(b) + f64(0.000000000000123)
  z = c * 3 - 7
  w = d * i64(3) + i64(9007199254740993)
  flag = e
''')
        baseline = self.directory / "baseline"
        self.run_case(source, baseline, "--backend", "both", "--input-pattern", "ramp", "--ir", "--assembly", "--bitwise")
        types = ["f32", "f64", "i32", "i64", "bool"]
        row = json.loads((baseline / "native.json").read_text())[0]
        self.assertEqual(row["outputs"], types)
        samples = list(read_samples(baseline / "typed.native.output.bin", types, 16, 2))
        self.assertEqual(samples[0][1], 0.75)
        self.assertEqual(samples[16][1], -0.125 + 0.000000000000123)
        self.assertEqual(samples[32][1], -199)
        self.assertEqual(samples[48][1], 9007199254740801)
        sources = json.loads((baseline / "typed.sources.json").read_text())
        self.assertEqual(len(sources["documents"]), 2)
        self.assertTrue(sources["embedded_stdlib"])
        self.assertTrue((baseline / "typed.asm").stat().st_size > 0)
        candidate = self.directory / "replay"
        self.run_case(baseline / "typed.mir.msgpack", candidate, "--backend", "both", "--input-pattern", "ramp", "--trust-mir", "--compare-to", baseline)
        comparison = json.loads((candidate / "comparison.json").read_text())
        self.assertEqual(len(comparison), 2)
        self.assertTrue(all(row["outputs"]["warm-output.bin"]["equivalent"] for row in comparison))
        wasm_only = self.directory / "wasm-only"
        self.run_case(baseline / "typed.mir.json", wasm_only, "--backend", "wasm", "--trust-mir", "--input-pattern", "ramp")
        self.assertFalse((wasm_only / "native.json").exists())
        self.assertEqual(json.loads((wasm_only / "wasm.json").read_text())[0]["validation"], "finite")
        mismatch = self.directory / "mismatch"
        self.run_case(baseline / "typed.mir.json", mismatch, "--trust-mir", "--block-size", "32", expect_success=False)
        self.assertEqual(json.loads((mismatch / "status.json").read_text())["status"], "failed")
        self.assertIn("MIR sample rate/block size differ", (mismatch / "typed-native.stderr.log").read_text())

    def test_inspection_does_not_execute_initialization(self):
        source = self.directory / "inspect.onda"
        source.write_text("init:\n  x = 0.0\n  while x < 2.0:\n    x += 0.0\nsample:\n  out1 = x\n")
        output = self.directory / "inspection"
        self.run_case(source, output, "--backend", "both", "--inspect-only", "--fast-math", "--opt-level", "1")
        self.assertTrue((output / "inspect.ll").is_file())
        self.assertTrue((output / "inspect.wat").is_file())
        self.assertFalse((output / "inspect.native.output.bin").exists())
        self.assertNotIn("ns_per_frame", json.loads((output / "native.json").read_text())[0])


if __name__ == "__main__":
    unittest.main()
