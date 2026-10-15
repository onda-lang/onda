#!/usr/bin/env python3
"""Regenerate the embedded SLEEF helpers; normal Cargo builds need no C compiler."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
COMMIT = '906ca7512ee483296780a81a21b9ca715d40dfe1'  # SLEEF 3.9.0
DEST = ROOT / 'crates/onda_codegen_llvm/math-kernel'


def run(command):
    return subprocess.check_output([str(x) for x in command], text=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--clang', required=True, type=Path, help='Clang 21.1.8 executable')
    parser.add_argument('--llvm-bin', required=True, type=Path, help='LLVM 21.1.2 bin directory')
    parser.add_argument('--source', type=Path, help='local source archive at the pinned commit')
    args = parser.parse_args()
    clang_version = run([args.clang, '--version']).splitlines()[0]
    llvm_version = run([args.llvm_bin / 'opt', '--version'])
    if 'clang version 21.1.8' not in clang_version or 'LLVM version 21.1.2' not in llvm_version:
        raise SystemExit('Regeneration requires Clang 21.1.8 and LLVM 21.1.2.')
    with tempfile.TemporaryDirectory(prefix='onda-sleef-') as temporary:
        work = Path(temporary)
        archive = args.source
        if archive is None:
            archive = work / 'source.tar.gz'
            urllib.request.urlretrieve(f'https://codeload.github.com/shibatch/sleef/tar.gz/{COMMIT}', archive)
        source_hash = hashlib.sha256(archive.read_bytes()).hexdigest()
        if source_hash != SOURCE_SHA256:
            raise SystemExit('SLEEF source archive checksum mismatch')
        with tarfile.open(archive) as tar:
            tar.extractall(work, filter='data')
        src = work / f'sleef-{COMMIT}'
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
        manifest = dict(sleef_version='3.9.0', commit=COMMIT, source_sha256=source_hash,
                        clang=clang_version, llvm='21.1.2', files={})
        for isa, triple, flags in [
            ('sse2', 'x86_64-unknown-linux-gnu', ['-msse2']),
            ('avx2', 'x86_64-unknown-linux-gnu', ['-mavx2', '-mfma']),
            ('advsimd', 'aarch64-unknown-linux-gnu', []),
        ]:
            for bits, suffix in [(32, 'sp'), (64, 'dp')]:
                stem = f'{isa}-f{bits}'
                wrapper = work / f'{stem}.c'
                wrapper.write_text(f'#include "sleefsimd{suffix}.c"\n#include "rempitab.c"\n')
                ir_file = work / f'{stem}.ll'
                names = {}
                for operation in ['sin', 'cos', 'exp', 'log']:
                    original = f'x{operation}{"f" if bits == 32 else ""}{"_u1" if operation != "exp" else ""}'
                    names[original] = f'onda_sleef_{isa}_{operation}_f{bits}'
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
                manifest['files'][bitcode.name] = hashlib.sha256(bitcode.read_bytes()).hexdigest()
                print(f'{bitcode.name}: {bitcode.stat().st_size} bytes')
        (DEST / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        (ROOT / 'licenses/SLEEF-LICENSE.txt').write_bytes((src / 'LICENSE.txt').read_bytes())


SOURCE_SHA256 = '76b779dba27aaf0b0551042ab59164754d859cd9a493695dcb6572a4cb5f3787'
if __name__ == '__main__':
    main()
