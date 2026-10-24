#!/usr/bin/env python3
"""Compare complete Onda/FFTW real transforms, including many-instance locality."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import statistics
import sys

from limits import Limits

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent


def source(size, precision):
    return f"""import std/fft
const N = {size}
buffers:
  input: {precision}
outs<{precision}> 2
init:
  fft = std::fft<N>::RealTransform<{precision}>()
block:
  fft.forward(input[:])
  index = 0
  sample:
    out1 = fft.real(index)
    out2 = fft.imag(index)
    index += 1
"""


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--runner', type=Path, default=ROOT / 'target/release/examples/benchmark_dsp')
    parser.add_argument('--sizes', type=int, nargs='+', default=[1024, 4096])
    parser.add_argument('--precisions', nargs='+', choices=['f32', 'f64'], default=['f32', 'f64'])
    parser.add_argument('--instances', type=int, nargs='+', default=[1, 32])
    parser.add_argument('--rounds', type=int, default=7)
    parser.add_argument('--round-ms', type=float, default=50)
    parser.add_argument('--cpu', type=int, default=min(os.sched_getaffinity(0)))
    parser.add_argument('--memory-mib', type=int, default=4096)
    parser.add_argument('--timeout-seconds', type=int, default=120)
    args = parser.parse_args()
    if any(n < 16 or n > 65536 or n & (n - 1) for n in args.sizes):
        parser.error('sizes must be powers of two between 16 and 65536')
    if any(n < 1 or n > 4096 for n in args.instances) or not 1 <= args.rounds <= 100 or not 0 < args.round_ms <= 10000:
        parser.error('invalid instance count, rounds or round duration')
    directory = args.output.resolve()
    if directory.exists() and any(directory.iterdir()):
        parser.error('output directory must be new or empty')
    directory.mkdir(parents=True, exist_ok=True)
    limits = Limits(args.cpu, args.memory_mib, args.timeout_seconds)
    results = []
    for size in args.sizes:
        for precision in args.precisions:
            name = f'fft-{precision}-{size}'
            path = directory / f'{name}.onda'
            path.write_text(source(size, precision))
            artifacts = directory / name
            limits.run([sys.executable, HERE / 'run.py', '--source', path,
                        '--inspect-only', '--assembly', '--block-size', size // 2 + 1,
                        '--runner', args.runner.resolve(), '--cpu', args.cpu,
                        '--memory-mib', args.memory_mib, '--timeout-seconds', args.timeout_seconds,
                        '--output', artifacts], directory / f'{name}-emit')
            metadata = json.loads((artifacts / f'{name}.object.json').read_text())
            runtime = metadata['runtime']
            obj = artifacts / f'{name}.o'
            binary = directory / f'{name}-bench'
            limits.run(['cc', '-O3', '-march=native', '-ffp-contract=off', '-no-pie',
                        '-I', ROOT / 'include', f'-DFFT_SIZE={size}',
                        f'-DFFT_SINGLE={int(precision == "f32")}',
                        f'-DONDA_STATE_SIZE={runtime["state_size_bytes"]}',
                        f'-DONDA_STATE_ALIGN={runtime["state_align_bytes"]}',
                        HERE / 'fft_reference.c', obj, '-lfftw3f', '-lfftw3', '-lm', '-o', binary],
                       directory / f'{name}-link')
            for instances in args.instances:
                prefix = directory / f'{name}-{instances}-run'
                limits.run([binary, instances, args.rounds, args.round_ms], prefix)
                result = json.loads(prefix.with_suffix('.stdout.log').read_text())
                result['medians_ns_per_transform'] = {
                    implementation: statistics.median(row['ns_per_transform'] for row in result['rounds']
                        if row['implementation'] == implementation)
                    for implementation in ['onda', 'fftw']}
                medians = result['medians_ns_per_transform']
                result['onda_to_fftw'] = medians['onda'] / medians['fftw']
                result['object_sha256'] = hashlib.sha256(obj.read_bytes()).hexdigest()
                result['onda_state_bytes_per_instance'] = runtime['state_size_bytes']
                result['input_and_output_bytes_per_instance'] = (size + 2 * (size // 2 + 1)) * (4 if precision == 'f32' else 8)
                results.append(result)
                (directory / 'results.json').write_text(json.dumps(results, indent=2) + '\n')
                print(f'{name}, {instances} instance(s): Onda {medians["onda"]:.1f}, FFTW {medians["fftw"]:.1f} ns/transform; ratio {result["onda_to_fftw"]:.3f}', flush=True)
    (directory / 'method.json').write_text(json.dumps({
        'contract': 'N immutable real inputs -> all N/2+1 positive split-complex bins',
        'timed_work': 'transform and output publication; allocation/planning/initialization/validation excluded',
        'numerics': 'strict Onda arithmetic; FFTW implementation validated against all bins, not bitwise equivalence',
        'round_order': 'alternating Onda/FFTW; counters measured in separate untimed passes',
        'cpu': args.cpu, 'worker_sha256': hashlib.sha256(args.runner.read_bytes()).hexdigest(),
        'sources_sha256': {name: hashlib.sha256((HERE / name).read_bytes()).hexdigest()
                           for name in ['fft.py', 'fft_reference.c', 'perf_counters.h']},
    }, indent=2) + '\n')


if __name__ == '__main__':
    main()
