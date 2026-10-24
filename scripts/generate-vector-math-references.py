#!/usr/bin/env python3
"""Generate native math test intervals with MPFR; MPFR is not needed to run tests."""
import argparse
import ctypes as c
import ctypes.util
import hashlib
import math
from pathlib import Path
import random
import struct

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / 'crates/onda_codegen_llvm/tests/fixtures/vector_math_reference.txt'
OPERATIONS = ('sin', 'cos', 'tan', 'tanh', 'atan', 'atan2', 'exp', 'exp2', 'log', 'pow')


def bits(value, width):
    return int.from_bytes(struct.pack('<f' if width == 32 else '<d', value), 'little')


def value(raw, width):
    return struct.unpack('<f' if width == 32 else '<d', raw.to_bytes(width // 8, 'little'))[0]


def ordered(raw, width):
    sign = 1 << (width - 1)
    return ((~raw) & ((1 << width) - 1)) if raw & sign else raw | sign


def adjacent(x, width):
    key = ordered(bits(x, width), width)
    sign = 1 << (width - 1)
    for k in range(key - 2, key + 3):
        yield value(k ^ sign if k & sign else (~k) & ((1 << width) - 1), width)


class Number(c.Structure):
    _fields_ = [('precision', c.c_long), ('sign', c.c_int),
                ('exponent', c.c_long), ('limbs', c.c_void_p)]


class Oracle:
    def __init__(self, library):
        self.lib = c.CDLL(library)
        ptr = c.POINTER(Number)
        signatures = {
            'init2': ([ptr, c.c_long], None), 'clear': ([ptr], None),
            'set_d': ([ptr, c.c_double, c.c_int], c.c_int),
            'get_d': ([ptr, c.c_int], c.c_double),
            'get_flt': ([ptr, c.c_int], c.c_float),
            'get_version': ([], c.c_char_p),
            'mul_d': ([ptr, ptr, c.c_double, c.c_int], c.c_int),
        }
        for name in OPERATIONS + ('add', 'sub'):
            binary = name in ('atan2', 'pow', 'add', 'sub')
            signatures[name] = ([ptr, ptr] + ([ptr] if binary else []) + [c.c_int], c.c_int)
        for name, (arguments, result) in signatures.items():
            fn = getattr(self.lib, 'mpfr_' + name)
            fn.argtypes, fn.restype = arguments, result

    def interval(self, operation, width, x, y, precision):
        numbers = [Number() for _ in range(5)]
        a, b, result, budget, edge = numbers
        for n in numbers:
            self.lib.mpfr_init2(n, precision)
        try:
            self.lib.mpfr_set_d(a, x, 0)
            self.lib.mpfr_set_d(b, y, 0)
            fn = getattr(self.lib, 'mpfr_' + operation)
            fn(result, a, b, 0) if operation in ('atan2', 'pow') else fn(result, a, 0)
            get = self.lib.mpfr_get_flt if width == 32 else self.lib.mpfr_get_d
            rounded = get(result, 0)  # MPFR_RNDN, ties to even.
            if math.isnan(rounded):
                return 'N', 0, 0
            if rounded == 0 or math.isinf(rounded):
                key = ordered(bits(rounded, width), width)
                return 'E', key, key
            # Match SLEEF's countULPsp/countULPdp: gradual-underflow spacing,
            # with exact classification for NaNs, rounded zeros and infinities.
            exponent = math.frexp(self.lib.mpfr_get_d(result, 0))[1]
            mantissa, minimum = (24, -149) if width == 32 else (53, -1074)
            self.lib.mpfr_set_d(a, max(exponent - mantissa, minimum), 0)
            self.lib.mpfr_exp2(budget, a, 0)
            accuracy = 1.0 if operation in ('exp', 'exp2', 'pow') else 3.5
            self.lib.mpfr_mul_d(budget, budget, accuracy, 0)
            # Round inward, so every accepted float lies within the error budget.
            self.lib.mpfr_sub(edge, result, budget, 2)  # MPFR_RNDU
            lower = ordered(bits(get(edge, 2), width), width)
            self.lib.mpfr_add(edge, result, budget, 3)  # MPFR_RNDD
            upper = ordered(bits(get(edge, 3), width), width)
            return 'R', lower, upper
        finally:
            for n in numbers:
                self.lib.mpfr_clear(n)


def inputs(width):
    maximum = value(0x7f7fffff if width == 32 else 0x7fefffffffffffff, width)
    normal = value(0x00800000 if width == 32 else 0x0010000000000000, width)
    tiny = value(1, width)
    common = [-math.inf, -maximum, -32., -8., -2., -1., -.5, -.125, -0.,
              0., .125, .5, 1., 2., 8., 32., maximum, math.inf, math.nan,
              tiny, -tiny, value(2, width), normal / 2, normal, -normal]
    rng = random.Random(0x4f4e4441 + width)
    for _ in range(24):
        raw = rng.getrandbits(width)
        x = value(raw, width)
        if math.isfinite(x):
            common.append(x)
    for operation in OPERATIONS:
        pairs = [(x, 0.) for x in common]
        if operation in ('sin', 'cos', 'tan'):
            for x in [math.pi / 2, math.pi, 3 * math.pi / 2, 1e10, 1e20]:
                pairs += [(v, 0.) for v in adjacent(x, width)]
        elif operation == 'log':
            pairs += [(v, 0.) for x in [normal, 1., 2.] for v in adjacent(x, width)]
        elif operation == 'exp':
            pairs += [(v, 0.) for x in [math.log(maximum), math.log(tiny) - math.log(2.)]
                      for v in adjacent(x, width)]
        elif operation == 'exp2':
            q, top = (-149, 128) if width == 32 else (-1074, 1024)
            pairs += [(q + offset, 0.) for offset in [-2., -1.5, -1., -.5, 0., .5, 1., 2.]]
            pairs += [(v, 0.) for v in adjacent(float(top), width)]
        elif operation in ('atan2', 'pow'):
            special = [math.nan, -math.inf, math.inf, -0., 0., -1., 1., -2., 2., .5]
            pairs = [(x, y) for x in special for y in special]
            pairs += [(x, y) for x in [-maximum, -normal, -tiny, tiny, normal, maximum]
                      for y in [-2., -.5, .5, 2., normal, maximum]]
            pairs += [(-2., y) for y in adjacent(2., width)]
        # Quantize inputs before MPFR evaluation: the tested arguments are binary32/64.
        for x, y in pairs:
            yield operation, bits(x, width), bits(y, width)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--mpfr', default=ctypes.util.find_library('mpfr'))
    parser.add_argument('--check', action='store_true', help='check generator provenance without MPFR')
    args = parser.parse_args()
    digest = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    provenance = f'# Generator SHA-256: {digest}'
    if args.check:
        lines = DEST.read_text().splitlines()
        payload = '\n'.join(line for line in lines if not line.startswith('#')) + '\n'
        checksum = '# Case data SHA-256: ' + hashlib.sha256(payload.encode()).hexdigest()
        if provenance not in lines or checksum not in lines:
            raise SystemExit('Vector math references are stale; regenerate with MPFR.')
        print('Vector math reference generator provenance and case checksum match.')
        return
    if not args.mpfr:
        parser.error('MPFR is required only to regenerate the reference fixture')
    oracle = Oracle(args.mpfr)
    rows = []
    for width in [32, 64]:
        for operation, xb, yb in sorted(set(inputs(width))):
            x, y = value(xb, width), value(yb, width)
            interval = oracle.interval(operation, width, x, y, 4096)
            if interval != oracle.interval(operation, width, x, y, 8192):
                raise SystemExit(f'Unstable interval: {operation} f{width} {xb:x} {yb:x}')
            kind, lower, upper = interval
            rows.append(f'{operation} {width} {xb:x} {yb:x} {kind} {lower:x} {upper:x}')
    payload = '\n'.join(rows) + '\n'
    lines = ['# Generated by scripts/generate-vector-math-references.py; do not edit.', provenance,
             '# Case data SHA-256: ' + hashlib.sha256(payload.encode()).hexdigest(),
             f'# MPFR {oracle.lib.mpfr_get_version().decode()}; intervals agree at 4096 and 8192 bits.',
             '# operation precision x_bits y_bits kind lower_ordered_bits upper_ordered_bits']
    DEST.write_text('\n'.join(lines) + '\n' + payload)
    print(f'Wrote {len(rows)} independently evaluated reference cases to {DEST.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
