"""Descriptive LLVM IR counts, kept separate from runtime measurements."""
from collections import Counter
import re


def llvm_summary(text):
    functions = {}
    current = None
    for line in text.splitlines() if isinstance(text, str) else text:
        line = line.rstrip()
        definition = re.match(r"define\b.*?@([^\s(]+)\(", line)
        if definition:
            current = Counter()
            functions[definition[1]] = current
            continue
        if line == "}":
            current = None
        if current is None:
            continue
        instruction = re.match(r"\s+(?:%[^=]+ = )?([a-z][a-z0-9]*)\b", line)
        if not instruction:
            continue
        operation = instruction[1]
        current["instruction_lines"] += 1
        if re.search(r"<\d+ x (?:float|double|i\d+)>", line):
            current["vector_instruction_lines"] += 1
        if operation == "shufflevector":
            current["shufflevector"] += 1
        if operation in {"load", "store"} and re.search(r"\balign 1\b", line):
            current["align_1_accesses"] += 1
        if re.search(r"\bcall\b", line):
            callee = re.search(r"@([^\s(]+)\(", line)
            if callee:
                current[f"call:{callee[1]}"] += 1
    return {"kind": "descriptive-llvm-ir-counts", "functions": functions}


def assembly_summary(text):
    """Static hints, not dynamic counts or a register-allocation spill oracle."""
    counts = Counter()
    relocations = Counter()
    for line in text.splitlines():
        relocation = re.search(r"\bR_(?:X86_64_(?:PLT32|GOTPCREL\w*)|AARCH64_CALL26)\s+(\S+)", line)
        if relocation:
            relocations[re.sub(r"[+-]0x[0-9a-f]+$", "", relocation[1])] += 1
        instruction = re.match(r"\s*[0-9a-f]+:\s+(?:[0-9a-f]{2,8}\s+)+([a-z][a-z0-9.]*)\s*(.*)", line)
        if not instruction:
            continue
        operation, operands = instruction.groups()
        counts["instructions"] += 1
        if operation in {"call", "callq", "bl", "blr"}:
            counts["calls"] += 1
        if re.search(r"\([^)]*%(?:rsp|rbp)[^)]*\)|\[sp\b", operands):
            counts["stack_address_operands"] += 1
        if re.search(r"\b[xyz]mm\d+\b|\bv\d+\.[0-9]+[bhsd]\b", operands):
            counts["vector_register_instructions"] += 1
    return {"kind": "descriptive-assembly-counts", "counts": counts,
            "external_relocations": relocations,
            "note": "Stack addresses include locals, ABI saves and spills; counts do not predict throughput."}
