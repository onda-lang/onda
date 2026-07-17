#!/usr/bin/env python3

import argparse
import json
import pathlib
import struct
import sys
from typing import Optional


SCALAR_FORMATS = {
    "bool": "?",
    "f32": "f",
    "f64": "d",
    "i32": "i",
    "i64": "q",
}

SCALAR_KINDS = {
    "bool": "PROCESSOR_SCALAR_BOOL",
    "f32": "PROCESSOR_SCALAR_F32",
    "f64": "PROCESSOR_SCALAR_F64",
    "i32": "PROCESSOR_SCALAR_I32",
    "i64": "PROCESSOR_SCALAR_I64",
}


def fail(message: str) -> None:
    raise ValueError(message)


def flattened_slots(entries: list[dict]) -> int:
    return max(
        (entry["slot_offset"] + entry.get("array_len", 1) for entry in entries),
        default=0,
    )


def scalar_name(type_repr: str) -> str:
    if type_repr.startswith("buffer[") and type_repr.endswith("]"):
        type_repr = type_repr[len("buffer[") : -1]
    scalar = type_repr.split("[", 1)[0]
    if scalar not in SCALAR_FORMATS:
        fail(f"unsupported scalar type {type_repr!r}")
    return scalar


def parse_scalar(text: str) -> object:
    return json.loads(text)


def parse_default(text: Optional[str], count: int) -> list[object]:
    if text is None:
        return [0] * count
    value = json.loads(text)
    values = value if isinstance(value, list) else [value]
    if len(values) != count:
        fail(f"default {text!r} has {len(values)} elements, expected {count}")
    return values


def target_endian(descriptor: dict) -> str:
    byte_order = descriptor["target"]["byte_order"]
    if byte_order == "little_endian":
        return "<"
    if byte_order == "big_endian":
        return ">"
    fail(f"unsupported target byte order {byte_order!r}")


def encode_parameter_defaults(descriptor: dict) -> bytes:
    runtime = descriptor["runtime"]
    storage = bytearray(runtime["param_size_bytes"])
    endian = target_endian(descriptor)

    for param in descriptor["metadata"]["params"]:
        scalar = scalar_name(param["type_repr"])
        count = param.get("array_len", 1)
        values = parse_default(param.get("default_repr"), count)
        encoded = struct.pack(endian + SCALAR_FORMATS[scalar] * count, *values)
        if len(encoded) != param["byte_size"]:
            fail(f"encoded size for parameter {param['name']!r} disagrees with sidecar")
        begin = param["byte_offset"]
        end = begin + len(encoded)
        if end > len(storage):
            fail(f"parameter {param['name']!r} lies outside parameter storage")
        storage[begin:end] = encoded

    return bytes(storage)


def encode_event_payload(descriptor: dict, event: dict) -> Optional[bytes]:
    payload_size = event.get("payload_bytes")
    if payload_size is None:
        return None
    if not isinstance(payload_size, int) or payload_size < 0:
        fail(f"event {event['name']!r} has an invalid payload size")

    payload = bytearray(payload_size)
    endian = target_endian(descriptor)
    for param in event["params"]:
        if param.get("is_slice"):
            fail(f"fixed event {event['name']!r} unexpectedly contains a slice")
        scalar = scalar_name(param["type_repr"])
        count = param.get("array_len", 1)
        defaults = param.get("default_reprs")
        if defaults is None:
            if param.get("has_default"):
                fail(f"event parameter {param['name']!r} is missing serialized defaults")
            values = [0] * count
        else:
            if len(defaults) != count:
                fail(f"event parameter {param['name']!r} has the wrong default count")
            values = [parse_scalar(value) for value in defaults]
        encoded = struct.pack(endian + SCALAR_FORMATS[scalar] * count, *values)
        if param.get("byte_size") != len(encoded):
            fail(f"event parameter {param['name']!r} has an inconsistent byte size")
        begin = param["byte_offset"]
        end = begin + len(encoded)
        if end > payload_size:
            fail(f"event parameter {param['name']!r} lies outside its payload")
        payload[begin:end] = encoded
    return bytes(payload)


def flatten_io_kinds(entries: list[dict]) -> list[str]:
    kinds: list[Optional[str]] = [None] * flattened_slots(entries)
    for entry in entries:
        kind = SCALAR_KINDS[scalar_name(entry["type_repr"])]
        for element in range(entry.get("array_len", 1)):
            slot = entry["slot_offset"] + element
            if kinds[slot] is not None:
                fail(f"overlapping flattened I/O slot {slot}")
            kinds[slot] = kind
    if any(kind is None for kind in kinds):
        fail("flattened I/O metadata contains a slot gap")
    return [kind for kind in kinds if kind is not None]


def c_array(values: list[str], fallback: str) -> str:
    return ", ".join(values) if values else fallback


def c_bytes(value: bytes) -> str:
    return ", ".join(f"0x{byte:02x}" for byte in value) or "0x00"


def generated_events(descriptor: dict) -> tuple[str, list[str], list[str], list[str], list[str]]:
    events = descriptor["metadata"]["events"]
    symbols = descriptor["exports"]["events"]
    if len(symbols) != len(events):
        fail("event metadata and export lists have different lengths")

    declarations: list[str] = []
    functions: list[str] = []
    names: list[str] = []
    fixed: list[str] = []
    payload_pointers: list[str] = []
    payload_definitions: list[str] = []
    for index, (event, symbol) in enumerate(zip(events, symbols)):
        if not symbol.isidentifier():
            fail(f"event export {symbol!r} is not a C identifier")
        declarations.append(
            f"extern void {symbol}(const void*, const void*, void*, void* const*, "
            "const int32_t*, const int32_t*, const float*);"
        )
        functions.append(symbol)
        names.append(json.dumps(event["name"]))
        payload = encode_event_payload(descriptor, event)
        if payload is None:
            fixed.append("0")
            payload_pointers.append("NULL")
        elif len(payload) == 0:
            fixed.append("1")
            payload_pointers.append("NULL")
        else:
            variable = f"PROCESSOR_EVENT_{index}_DEFAULT_PAYLOAD"
            payload_definitions.append(
                f"static const unsigned char {variable}[{len(payload)}] = {{ {c_bytes(payload)} }};"
            )
            fixed.append("1")
            payload_pointers.append(variable)

    block = "\n".join(declarations + payload_definitions)
    return block, functions, names, fixed, payload_pointers


def generate(descriptor: dict) -> str:
    if descriptor.get("format") != "onda-processor" or descriptor.get("format_version") != 3:
        fail("expected an onda-processor format-version-3 sidecar")
    if descriptor.get("abi_version") != 1:
        fail("the example implements only processor ABI version 1")
    if descriptor.get("artifact_kind") != "relocatable_object":
        fail("the example requires a relocatable native object")
    if descriptor["target"].get("pointer_model") != "native_address":
        fail("the example requires a native-address target")

    metadata = descriptor["metadata"]
    runtime = descriptor["runtime"]
    compile_info = descriptor["compile"]
    inputs = flatten_io_kinds(metadata["inputs"])
    outputs = flatten_io_kinds(metadata["outputs"])
    buffers = metadata["buffers"]
    buffer_kinds = [SCALAR_KINDS[scalar_name(buffer["type_repr"])] for buffer in buffers]
    buffer_channels = []
    for buffer in buffers:
        channels = buffer.get("channels")
        if channels == "mono":
            buffer_channels.append("1")
        elif channels == "static":
            static_channels = buffer.get("static_channels")
            if not isinstance(static_channels, int) or static_channels <= 0:
                fail(f"buffer {buffer['name']!r} has invalid static channels")
            buffer_channels.append(str(static_channels))
        elif channels == "dynamic":
            buffer_channels.append("2")
        else:
            fail(f"buffer {buffer['name']!r} has unknown channel layout {channels!r}")

    event_block, event_functions, event_names, event_fixed, event_payloads = generated_events(
        descriptor
    )
    defaults = encode_parameter_defaults(descriptor)
    triple = json.dumps(descriptor["target"]["triple"])
    buffer_names = [json.dumps(buffer["name"]) for buffer in buffers]

    return f"""/* Generated from the exact Onda JSON sidecar. Do not edit. */
#ifndef ONDA_PROCESSOR_CONFIG_H
#define ONDA_PROCESSOR_CONFIG_H

enum {{
  PROCESSOR_SCALAR_BOOL = 1,
  PROCESSOR_SCALAR_F32 = 2,
  PROCESSOR_SCALAR_F64 = 3,
  PROCESSOR_SCALAR_I32 = 4,
  PROCESSOR_SCALAR_I64 = 5
}};

#define PROCESSOR_DESCRIPTOR_FORMAT_VERSION {descriptor['format_version']}u
#define PROCESSOR_DESCRIPTOR_ABI_VERSION {descriptor['abi_version']}u
#define PROCESSOR_TARGET_TRIPLE {triple}
#define PROCESSOR_SAMPLE_RATE {compile_info['sample_rate']!r}f
#define PROCESSOR_BLOCK_SIZE {compile_info['block_size']}
#define PROCESSOR_STATE_SIZE {runtime['state_size_bytes']}
#define PROCESSOR_STATE_ALIGN {runtime['state_align_bytes']}
#define PROCESSOR_PARAM_SIZE {runtime['param_size_bytes']}
#define PROCESSOR_PARAM_ALIGN {runtime['param_align_bytes']}
#define PROCESSOR_INPUT_COUNT {len(inputs)}
#define PROCESSOR_OUTPUT_COUNT {len(outputs)}
#define PROCESSOR_BUFFER_COUNT {len(buffers)}
#define PROCESSOR_EVENT_COUNT {len(event_functions)}

static const unsigned char
PROCESSOR_PARAM_DEFAULT_BYTES[(PROCESSOR_PARAM_SIZE > 0) ? PROCESSOR_PARAM_SIZE : 1] = {{
  {c_bytes(defaults)}
}};

static const unsigned char
PROCESSOR_INPUT_KINDS[(PROCESSOR_INPUT_COUNT > 0) ? PROCESSOR_INPUT_COUNT : 1] = {{
  {c_array(inputs, '0')}
}};

static const unsigned char
PROCESSOR_OUTPUT_KINDS[(PROCESSOR_OUTPUT_COUNT > 0) ? PROCESSOR_OUTPUT_COUNT : 1] = {{
  {c_array(outputs, '0')}
}};

static const unsigned char
PROCESSOR_BUFFER_KINDS[(PROCESSOR_BUFFER_COUNT > 0) ? PROCESSOR_BUFFER_COUNT : 1] = {{
  {c_array(buffer_kinds, '0')}
}};

static const int32_t
PROCESSOR_BUFFER_CHANNELS[(PROCESSOR_BUFFER_COUNT > 0) ? PROCESSOR_BUFFER_COUNT : 1] = {{
  {c_array(buffer_channels, '0')}
}};

static const char* const
PROCESSOR_BUFFER_NAMES[(PROCESSOR_BUFFER_COUNT > 0) ? PROCESSOR_BUFFER_COUNT : 1] = {{
  {c_array(buffer_names, 'NULL')}
}};

{event_block}

static const onda_processor_event_fn
PROCESSOR_EVENT_FUNCTIONS[(PROCESSOR_EVENT_COUNT > 0) ? PROCESSOR_EVENT_COUNT : 1] = {{
  {c_array(event_functions, 'NULL')}
}};

static const char* const
PROCESSOR_EVENT_NAMES[(PROCESSOR_EVENT_COUNT > 0) ? PROCESSOR_EVENT_COUNT : 1] = {{
  {c_array(event_names, 'NULL')}
}};

static const unsigned char
PROCESSOR_EVENT_HAS_FIXED_PAYLOAD[(PROCESSOR_EVENT_COUNT > 0) ? PROCESSOR_EVENT_COUNT : 1] = {{
  {c_array(event_fixed, '0')}
}};

static const void* const
PROCESSOR_EVENT_DEFAULT_PAYLOADS[(PROCESSOR_EVENT_COUNT > 0) ? PROCESSOR_EVENT_COUNT : 1] = {{
  {c_array(event_payloads, 'NULL')}
}};

#endif
"""


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate generic C integration tables from an Onda processor sidecar"
    )
    parser.add_argument("descriptor", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()

    try:
        descriptor = json.loads(args.descriptor.read_text(encoding="utf-8"))
        generated = generate(descriptor)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(generated, encoding="utf-8")
    except (OSError, KeyError, TypeError, ValueError, json.JSONDecodeError) as error:
        print(f"generate_config.py: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
