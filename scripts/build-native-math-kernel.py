#!/usr/bin/env python3
"""Regenerate embedded SLEEF bitcode; normal Cargo builds do not compile SLEEF."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
COMMIT = '906ca7512ee483296780a81a21b9ca715d40dfe1'  # SLEEF 3.9.0
DEST = ROOT / 'crates/onda_codegen_llvm/math-kernel'
SOURCE = ROOT / 'deps/sleef'
# operation, accuracy family, source suffix, arity
FUNCTIONS = [
    ('sin', 'u35', '', 1), ('cos', 'u35', '', 1), ('tan', 'u35', '', 1),
    ('tanh', 'u35', '_u35', 1), ('atan', 'u35', '', 1), ('atan2', 'u35', '', 2),
    ('exp', 'u10', '', 1), ('exp2', 'u10', '', 1), ('log', 'u35', '', 1), ('pow', 'u10', '', 2),
]


def run(command):
    return subprocess.check_output([str(x) for x in command], text=True)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check():
    manifest = json.loads((DEST / 'manifest.json').read_text())
    if manifest['commit'] != COMMIT:
        raise SystemExit('SLEEF manifest commit is stale.')
    gitlink = run(['git', '-C', ROOT, 'ls-files', '--stage', 'deps/sleef']).split()
    if gitlink[:2] != ['160000', COMMIT]:
        raise SystemExit('SLEEF submodule pin does not match the generator.')
    if set(manifest['files']) != {path.name for path in DEST.glob('*.bc')}:
        raise SystemExit('SLEEF bitcode inventory does not match the manifest.')
    for directory, entries in [(DEST, manifest['files']), (ROOT, manifest['sources'])]:
        for name, expected in entries.items():
            if digest(directory / name) != expected:
                raise SystemExit(f'SLEEF checksum mismatch: {name}; regenerate the kernels.')
    print('SLEEF submodule pin, generator, numerical mapping, license and bitcode checksums match.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true', help='verify shipped files without compiler tools or SLEEF sources')
    parser.add_argument('--clang', type=Path, help='Clang 21.1.8 executable')
    parser.add_argument('--llvm-bin', type=Path, help='LLVM 21.1.2 bin directory')
    args = parser.parse_args()
    if args.check:
        check()
        return
    if args.clang is None or args.llvm_bin is None:
        parser.error('regeneration requires --clang and --llvm-bin')
    clang_version = run([args.clang, '--version']).splitlines()[0]
    llvm_version = run([args.llvm_bin / 'opt', '--version'])
    if 'clang version 21.1.8' not in clang_version or 'LLVM version 21.1.2' not in llvm_version:
        raise SystemExit('Regeneration requires Clang 21.1.8 and LLVM 21.1.2.')
    if not (SOURCE / 'src/libm/sleefsimdsp.c').exists():
        raise SystemExit('Initialize sources with: git submodule update --init deps/sleef')
    if run(['git', '-C', SOURCE, 'rev-parse', 'HEAD']).strip() != COMMIT:
        raise SystemExit('SLEEF submodule does not match the pinned commit.')
    if run(['git', '-C', SOURCE, 'status', '--porcelain']).strip():
        raise SystemExit('SLEEF submodule has local changes; regeneration requires clean sources.')
    with tempfile.TemporaryDirectory(prefix='onda-sleef-') as temporary:
        work = Path(temporary)
        src = SOURCE
        # Freestanding declarations: the selected functions must have no libc
        # imports after dead stripping. Clang supplies integer/float/ISA headers.
        for name, text in {
            'sleef-config.h': '/* Optional quad precision is disabled. */\n',
            'assert.h': '#define assert(x) ((void)0)\n',
            'string.h': '#include <stddef.h>\nvoid *memcpy(void *, const void *, size_t);\n',
            'stdlib.h': '#include <stddef.h>\nvoid *malloc(size_t);\nvoid free(void *);\n',
        }.items():
            (work / name).write_text(text)
        DEST.mkdir(exist_ok=True)
        manifest = dict(sleef_version='3.9.0', commit=COMMIT,
                        clang=clang_version, llvm='21.1.2', files={})
        for isa, width, triple, flags in [
            ('sse2', 128, 'x86_64-unknown-linux-gnu', ['-msse2']),
            ('avx', 256, 'x86_64-unknown-linux-gnu', ['-mavx']),
            ('avx2128', 128, 'x86_64-unknown-linux-gnu', ['-mavx2', '-mfma']),
            ('avx2', 256, 'x86_64-unknown-linux-gnu', ['-mavx2', '-mfma']),
            ('avx512f', 512, 'x86_64-unknown-linux-gnu', ['-mavx512f', '-mfma']),
            ('advsimd', 128, 'aarch64-unknown-linux-gnu', []),
        ]:
            for bits, suffix in [(32, 'sp'), (64, 'dp')]:
                stem = f'{isa}-f{bits}'
                wrapper = work / f'{stem}.c'
                wrapper.write_text(f'#include "sleefsimd{suffix}.c"\n#include "rempitab.c"\n')
                ir_file = work / f'{stem}.ll'
                names = {}
                for operation, accuracy, source_suffix, _ in FUNCTIONS:
                    original = f'x{operation}{"f" if bits == 32 else ""}{source_suffix}'
                    precision = 'f' if bits == 32 else 'd'
                    names[original] = f'Sleef_{operation}{precision}{width // bits}_{accuracy}{isa}'
                command = [args.clang, '-S', '-emit-llvm', '-O2', '-ffp-contract=off',
                           '-fno-math-errno', '-fno-asynchronous-unwind-tables', '-ffreestanding',
                           '-nostdlibinc', f'--target={triple}', *flags, f'-DENABLE_{isa.upper()}',
                           '-DNDEBUG', *[f'-D{k}={v}' for k, v in names.items()],
                           *[f'-I{src}/src/{d}' for d in ['common', 'arch', 'libm']],
                           f'-I{work}', wrapper, '-o', ir_file]
                run(command)
                ir = ir_file.read_text()
                # ISA eligibility is enforced by codegen. Do not import the
                # generating host CPU/tuning policy into a different target OS.
                ir = re.sub(r' "(?:target-cpu|target-features|tune-cpu|min-legal-vector-width)"="[^"]*"', '', ir)
                ir = re.sub(r'^; ModuleID = .*$', '; Generated SLEEF helper', ir, flags=re.M)
                ir = re.sub(r'^source_filename = .*$', f'source_filename = "onda-sleef-{stem}"', ir, flags=re.M)
                ir = re.sub(r'^!llvm\.(?:module.flags|ident) = .*\n', '', ir, flags=re.M)
                ir_file.write_text(ir)
                bitcode = DEST / f'{stem}.bc'
                run([args.llvm_bin / 'opt', ir_file, '-passes=internalize,globaldce,strip',
                     '-internalize-public-api-list=' + ','.join(names.values()), '-o', bitcode])
                final_ir = run([args.llvm_bin / 'llvm-dis', bitcode, '-o', '-'])
                imports = re.findall(r'^declare[^@]*@([^ (]+)', final_ir, re.M)
                if any(not name.startswith('llvm.') for name in imports):
                    raise SystemExit(f'{stem} has external imports: {imports}')
                if 'asm ' in final_ir or 'target-features' in final_ir or 'target-cpu' in final_ir:
                    raise SystemExit(f'{stem} retained host configuration or assembly')
                exports = re.findall(r'^define (?!internal )[^@]*@([^ (]+)', final_ir, re.M)
                if sorted(exports) != sorted(names.values()):
                    raise SystemExit(f'{stem} export mismatch: {exports}')
                manifest['files'][bitcode.name] = digest(bitcode)
                print(f'{bitcode.name}: {bitcode.stat().st_size} bytes')
        spec = '// Generated by scripts/build-native-math-kernel.py. Do not edit.\n'
        spec += 'pub const FUNCTIONS: &[(&str, &str, usize)] = &[\n'
        for operation, accuracy, _, arity in FUNCTIONS:
            spec += f'    ("{operation}", "{accuracy}", {arity}),\n'
        spec += '];\n'
        (ROOT / 'crates/onda_codegen_llvm/src/vector_math_spec.rs').write_text(spec)
        (ROOT / 'licenses/SLEEF-LICENSE.txt').write_bytes((src / 'LICENSE.txt').read_bytes())
        manifest['sources'] = {name: digest(ROOT / name) for name in [
            'scripts/build-native-math-kernel.py',
            'crates/onda_codegen_llvm/src/vector_math_spec.rs',
            'licenses/SLEEF-LICENSE.txt',
        ]}
        (DEST / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')


if __name__ == '__main__':
    main()
