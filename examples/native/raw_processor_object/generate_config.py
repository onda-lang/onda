#!/usr/bin/env python3

import argparse
import json
import math
import pathlib
import struct
import sys
from dataclasses import dataclass
from typing import Optional


PROCESSOR_ARTIFACT_FORMAT = "onda-processor"
# Synchronized from format-versions.json; do not edit these copies directly.
PROCESSOR_ARTIFACT_FORMAT_VERSION = 4
PROCESSOR_ABI_VERSION = 5
MAX_EXACT_HOST_INTEGER = (1 << 53) - 1

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


@dataclass
class GeneratedParamTables:
    names: list[str]
    kinds: list[str]
    array_lengths: list[str]
    byte_offsets: list[str]
    control_scales: list[str]
    has_curves: list[str]
    curves: list[str]
    range_mins: list[str]
    range_maxes: list[str]
    steps: list[str]
    step_counts: list[str]
    units: list[str]


def fail(message: str) -> None:
    raise ValueError(message)


def flattened_slots(entries: list[dict]) -> int:
    return max(
        (entry["slot_offset"] + entry.get("array_len", 1) for entry in entries),
        default=0,
    )


def scalar_name(type_repr: str) -> str:
    if type_repr.startswith("buffer<") and type_repr.endswith(">"):
        type_repr = type_repr[len("buffer<") : -1]
    scalar = type_repr.split("[", 1)[0]
    if scalar not in SCALAR_FORMATS:
        fail(f"unsupported scalar type {type_repr!r}")
    return scalar


def parse_scalar(text: str) -> object:
    return json.loads(text)


def parse_default(values: Optional[list[str]], count: int) -> list[object]:
    if values is None:
        return [0] * count
    if len(values) != count:
        fail(f"default has {len(values)} elements, expected {count}")
    return [parse_scalar(value) for value in values]


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
        values = parse_default(param.get("default_reprs"), count)
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
    payload_size = event.get("payload_size_bytes")
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


def c_string(value: str, context: str) -> str:
    if "\0" in value:
        fail(f"{context} must not contain a NUL byte")
    escaped = []
    for byte in value.encode("utf-8"):
        if byte == ord('"'):
            escaped.append('\\"')
        elif byte == ord("\\"):
            escaped.append("\\\\")
        elif 0x20 <= byte <= 0x7E:
            escaped.append(chr(byte))
        else:
            # Three-digit octal escapes avoid the look-ahead ambiguity of \xHH.
            escaped.append(f"\\{byte:03o}")
    return '"' + "".join(escaped) + '"'


def c_f64_value(value: object, context: str) -> str:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        fail(f"{context} is not numeric")
    value = float(value)
    if not math.isfinite(value):
        fail(f"{context} must be finite")
    rendered = format(value, ".17g")
    if "." not in rendered and "e" not in rendered:
        rendered += ".0"
    return rendered


def c_host_control_f64(text: str, scalar: str, context: str) -> str:
    value = parse_scalar(text)
    if scalar == "f32":
        value = struct.unpack(">f", struct.pack(">f", float(value)))[0]
    return c_f64_value(value, context)


def generated_params(descriptor: dict) -> GeneratedParamTables:
    names: list[str] = []
    kinds: list[str] = []
    array_lengths: list[str] = []
    byte_offsets: list[str] = []
    control_scales: list[str] = []
    has_curves: list[str] = []
    curves: list[str] = []
    range_mins: list[str] = []
    range_maxes: list[str] = []
    steps: list[str] = []
    step_counts: list[str] = []
    units: list[str] = []

    for param in descriptor["metadata"]["params"]:
        name = param["name"]
        scalar = scalar_name(param["type_repr"])
        array_len = param.get("array_len", 1)
        byte_offset = param.get("byte_offset")
        if not isinstance(array_len, int) or array_len <= 0:
            fail(f"parameter {name!r} has an invalid array length")
        if not isinstance(byte_offset, int) or byte_offset < 0:
            fail(f"parameter {name!r} has an invalid byte offset")

        control = param.get("param_control")
        if control is None:
            control_scale = "ONDA_PROCESSOR_PARAM_SCALE_NONE"
            has_curve = "0"
            curve = "0.0"
            range_min = "0.0"
            range_max = "0.0"
            step = "0.0"
            step_count = "0"
            unit = "NULL"
        else:
            scale = control.get("scale")
            if scale == "linear":
                control_scale = "ONDA_PROCESSOR_PARAM_SCALE_LINEAR"
            elif scale == "log":
                control_scale = "ONDA_PROCESSOR_PARAM_SCALE_LOG"
            else:
                fail(f"parameter {name!r} has an invalid control scale")
            if "curve" not in control:
                fail(f"parameter {name!r} control metadata is missing its curve")
            curve_value = control["curve"]
            if curve_value is None:
                has_curve = "0"
                curve = "0.0"
            else:
                if scale == "log":
                    fail(
                        f"parameter {name!r} cannot combine logarithmic scale "
                        "with curve"
                    )
                has_curve = "1"
                curve = c_f64_value(curve_value, f"parameter {name!r} curve")
            range_min_repr = param.get("range_min_repr")
            range_max_repr = param.get("range_max_repr")
            if not isinstance(range_min_repr, str) or not isinstance(range_max_repr, str):
                fail(f"parameter {name!r} control metadata is missing its range")
            if scalar == "i64":
                range_min_value = parse_scalar(range_min_repr)
                range_max_value = parse_scalar(range_max_repr)
                if (
                    isinstance(range_min_value, bool)
                    or isinstance(range_max_value, bool)
                    or not isinstance(range_min_value, int)
                    or not isinstance(range_max_value, int)
                    or abs(range_min_value) > MAX_EXACT_HOST_INTEGER
                    or abs(range_max_value) > MAX_EXACT_HOST_INTEGER
                    or range_max_value - range_min_value > MAX_EXACT_HOST_INTEGER
                ):
                    fail(
                        f"parameter {name!r} i64 control range is not exactly "
                        "representable by the host API"
                    )
            range_min = c_host_control_f64(
                range_min_repr, scalar, f"parameter {name!r} range minimum"
            )
            range_max = c_host_control_f64(
                range_max_repr, scalar, f"parameter {name!r} range maximum"
            )
            step_repr = control.get("step_repr")
            step_count_value = control.get("step_count")
            if step_repr is None:
                if step_count_value is not None:
                    fail(f"parameter {name!r} has a step count without a step")
                step = "0.0"
                step_count = "0"
            else:
                if not isinstance(step_repr, str):
                    fail(f"parameter {name!r} has an invalid step")
                if not isinstance(step_count_value, int) or step_count_value <= 0:
                    fail(f"parameter {name!r} has an invalid step count")
                step = c_host_control_f64(
                    step_repr, scalar, f"parameter {name!r} step"
                )
                step_count = str(step_count_value)
            unit_value = control.get("unit")
            if unit_value is not None and not isinstance(unit_value, str):
                fail(f"parameter {name!r} has an invalid unit")
            unit = (
                "NULL"
                if unit_value is None
                else c_string(unit_value, f"parameter {name!r} unit")
            )

        names.append(c_string(name, "parameter name"))
        kinds.append(SCALAR_KINDS[scalar])
        array_lengths.append(str(array_len))
        byte_offsets.append(str(byte_offset))
        control_scales.append(control_scale)
        has_curves.append(has_curve)
        curves.append(curve)
        range_mins.append(range_min)
        range_maxes.append(range_max)
        steps.append(step)
        step_counts.append(step_count)
        units.append(unit)

    return GeneratedParamTables(
        names=names,
        kinds=kinds,
        array_lengths=array_lengths,
        byte_offsets=byte_offsets,
        control_scales=control_scales,
        has_curves=has_curves,
        curves=curves,
        range_mins=range_mins,
        range_maxes=range_maxes,
        steps=steps,
        step_counts=step_counts,
        units=units,
    )


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
            f"extern uint32_t {symbol}(const void*, const void*, void*, void* const*, "
            "const int32_t*, const int32_t*, const float*);"
        )
        functions.append(symbol)
        names.append(c_string(event["name"], "event name"))
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
    if (
        descriptor.get("format") != PROCESSOR_ARTIFACT_FORMAT
        or descriptor.get("format_version") != PROCESSOR_ARTIFACT_FORMAT_VERSION
    ):
        fail(
            f"expected an {PROCESSOR_ARTIFACT_FORMAT} "
            f"format-version-{PROCESSOR_ARTIFACT_FORMAT_VERSION} sidecar"
        )
    if descriptor.get("abi_version") != PROCESSOR_ABI_VERSION:
        fail(
            "the example implements only processor ABI version "
            f"{PROCESSOR_ABI_VERSION}"
        )
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
    params = generated_params(descriptor)
    triple = c_string(descriptor["target"]["triple"], "target triple")
    buffer_names = [c_string(buffer["name"], "buffer name") for buffer in buffers]

    return f"""/* Generated from the exact Onda JSON sidecar. Do not edit. */
#ifndef ONDA_PROCESSOR_CONFIG_H
#define ONDA_PROCESSOR_CONFIG_H

#include <math.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "onda_processor_abi.h"

#if defined(_MSC_VER)
#define PROCESSOR_STATIC_INLINE static __inline
#else
#define PROCESSOR_STATIC_INLINE static inline
#endif

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
#define PROCESSOR_PARAM_COUNT {len(params.names)}
#define PROCESSOR_INPUT_COUNT {len(inputs)}
#define PROCESSOR_OUTPUT_COUNT {len(outputs)}
#define PROCESSOR_BUFFER_COUNT {len(buffers)}
#define PROCESSOR_EVENT_COUNT {len(event_functions)}

static const unsigned char
PROCESSOR_PARAM_DEFAULT_BYTES[(PROCESSOR_PARAM_SIZE > 0) ? PROCESSOR_PARAM_SIZE : 1] = {{
  {c_bytes(defaults)}
}};

static const char* const
PROCESSOR_PARAM_NAMES[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.names, 'NULL')}
}};

static const unsigned char
PROCESSOR_PARAM_KINDS[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.kinds, '0')}
}};

static const size_t
PROCESSOR_PARAM_ARRAY_LENGTHS[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.array_lengths, '0')}
}};

static const size_t
PROCESSOR_PARAM_BYTE_OFFSETS[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.byte_offsets, '0')}
}};

static const unsigned char
PROCESSOR_PARAM_CONTROL_SCALES[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.control_scales, '0')}
}};

static const unsigned char
PROCESSOR_PARAM_HAS_CURVES[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.has_curves, '0')}
}};

static const double
PROCESSOR_PARAM_CURVES[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.curves, '0.0')}
}};

static const double
PROCESSOR_PARAM_RANGE_MINS[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.range_mins, '0.0')}
}};

static const double
PROCESSOR_PARAM_RANGE_MAXES[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.range_maxes, '0.0')}
}};

static const double
PROCESSOR_PARAM_STEPS[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.steps, '0.0')}
}};

static const uint32_t
PROCESSOR_PARAM_STEP_COUNTS[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.step_counts, '0')}
}};

static const char* const
PROCESSOR_PARAM_UNITS[(PROCESSOR_PARAM_COUNT > 0) ? PROCESSOR_PARAM_COUNT : 1] = {{
  {c_array(params.units, 'NULL')}
}};

PROCESSOR_STATIC_INLINE int processor_param_is_scalar(int index) {{
  return index >= 0 && index < PROCESSOR_PARAM_COUNT &&
    PROCESSOR_PARAM_ARRAY_LENGTHS[index] == 1;
}}

PROCESSOR_STATIC_INLINE onda_processor_param_domain processor_param_domain(int index) {{
  onda_processor_param_domain domain;
  domain.minimum = PROCESSOR_PARAM_RANGE_MINS[index];
  domain.maximum = PROCESSOR_PARAM_RANGE_MAXES[index];
  domain.step = PROCESSOR_PARAM_STEPS[index];
  domain.curve = PROCESSOR_PARAM_CURVES[index];
  domain.step_count = PROCESSOR_PARAM_STEP_COUNTS[index];
  domain.scale = (onda_processor_param_scale)PROCESSOR_PARAM_CONTROL_SCALES[index];
  switch (PROCESSOR_PARAM_KINDS[index]) {{
    case PROCESSOR_SCALAR_F32:
      domain.scalar = ONDA_PROCESSOR_PARAM_SCALAR_F32;
      break;
    case PROCESSOR_SCALAR_F64:
      domain.scalar = ONDA_PROCESSOR_PARAM_SCALAR_F64;
      break;
    case PROCESSOR_SCALAR_I32:
      domain.scalar = ONDA_PROCESSOR_PARAM_SCALAR_I32;
      break;
    case PROCESSOR_SCALAR_I64:
      domain.scalar = ONDA_PROCESSOR_PARAM_SCALAR_I64;
      break;
    default:
      domain.scalar = (onda_processor_param_scalar)-1;
      break;
  }}
  domain.has_curve = PROCESSOR_PARAM_HAS_CURVES[index];
  domain.unit = PROCESSOR_PARAM_UNITS[index];
  return domain;
}}

PROCESSOR_STATIC_INLINE double processor_param_constrain_plain(int index, double plain) {{
  if (!processor_param_is_scalar(index)) {{
    return NAN;
  }}
  if (PROCESSOR_PARAM_KINDS[index] == PROCESSOR_SCALAR_BOOL) {{
    return plain >= 0.5 ? 1.0 : 0.0;
  }}
  const onda_processor_param_domain domain = processor_param_domain(index);
  return onda_processor_param_constrain_plain(&domain, plain);
}}

PROCESSOR_STATIC_INLINE double processor_param_normalized_to_plain(int index, double normalized) {{
  if (!processor_param_is_scalar(index)) {{
    return NAN;
  }}
  if (PROCESSOR_PARAM_KINDS[index] == PROCESSOR_SCALAR_BOOL) {{
    return normalized >= 0.5 ? 1.0 : 0.0;
  }}
  const onda_processor_param_domain domain = processor_param_domain(index);
  return onda_processor_param_normalized_to_plain(&domain, normalized);
}}

PROCESSOR_STATIC_INLINE double processor_param_plain_to_normalized(int index, double plain) {{
  if (!processor_param_is_scalar(index)) {{
    return NAN;
  }}
  if (PROCESSOR_PARAM_KINDS[index] == PROCESSOR_SCALAR_BOOL) {{
    return plain >= 0.5 ? 1.0 : 0.0;
  }}
  const onda_processor_param_domain domain = processor_param_domain(index);
  return onda_processor_param_plain_to_normalized(&domain, plain);
}}

PROCESSOR_STATIC_INLINE double processor_param_read_plain(const void* params, int index) {{
  if (params == NULL || !processor_param_is_scalar(index)) {{
    return NAN;
  }}
  const unsigned char* source =
    (const unsigned char*)params + PROCESSOR_PARAM_BYTE_OFFSETS[index];
  switch (PROCESSOR_PARAM_KINDS[index]) {{
    case PROCESSOR_SCALAR_BOOL: {{
      uint8_t value;
      memcpy(&value, source, sizeof(value));
      return value == 0 ? 0.0 : 1.0;
    }}
    case PROCESSOR_SCALAR_F32: {{
      float value;
      memcpy(&value, source, sizeof(value));
      return (double)value;
    }}
    case PROCESSOR_SCALAR_F64: {{
      double value;
      memcpy(&value, source, sizeof(value));
      return value;
    }}
    case PROCESSOR_SCALAR_I32: {{
      int32_t value;
      memcpy(&value, source, sizeof(value));
      return (double)value;
    }}
    case PROCESSOR_SCALAR_I64: {{
      int64_t value;
      memcpy(&value, source, sizeof(value));
      return (double)value;
    }}
    default:
      return NAN;
  }}
}}

PROCESSOR_STATIC_INLINE int processor_param_store_plain(
  void* params,
  int index,
  double plain
) {{
  if (params == NULL || !processor_param_is_scalar(index)) {{
    return -1;
  }}
  if (isnan(plain)) {{
    return -1;
  }}
  unsigned char* destination =
    (unsigned char*)params + PROCESSOR_PARAM_BYTE_OFFSETS[index];
  switch (PROCESSOR_PARAM_KINDS[index]) {{
    case PROCESSOR_SCALAR_BOOL: {{
      const uint8_t value = plain != 0.0;
      memcpy(destination, &value, sizeof(value));
      return 0;
    }}
    case PROCESSOR_SCALAR_F32: {{
      const float value = (float)plain;
      memcpy(destination, &value, sizeof(value));
      return 0;
    }}
    case PROCESSOR_SCALAR_F64:
      memcpy(destination, &plain, sizeof(plain));
      return 0;
    case PROCESSOR_SCALAR_I32: {{
      const int32_t value = (int32_t)round(plain);
      memcpy(destination, &value, sizeof(value));
      return 0;
    }}
    case PROCESSOR_SCALAR_I64: {{
      const int64_t value = (int64_t)round(plain);
      memcpy(destination, &value, sizeof(value));
      return 0;
    }}
    default:
      return -1;
  }}
}}

PROCESSOR_STATIC_INLINE int processor_param_set_plain(void* params, int index, double plain) {{
  const double constrained = processor_param_constrain_plain(index, plain);
  return isnan(constrained)
    ? -1
    : processor_param_store_plain(params, index, constrained);
}}

PROCESSOR_STATIC_INLINE int processor_param_set_normalized(
  void* params,
  int index,
  double normalized
) {{
  const double plain = processor_param_normalized_to_plain(index, normalized);
  return isnan(plain) ? -1 : processor_param_store_plain(params, index, plain);
}}

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

#undef PROCESSOR_STATIC_INLINE

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
