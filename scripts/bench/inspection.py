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
        if operation == "call":
            callee = re.search(r"@([^\s(]+)\(", line)
            if callee:
                current[f"call:{callee[1]}"] += 1
    return {"kind": "descriptive-llvm-ir-counts", "functions": functions}
