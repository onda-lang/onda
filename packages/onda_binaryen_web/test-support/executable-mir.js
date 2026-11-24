import { SUPPORTED_MIR_SCHEMA_VERSION } from "../src/constants.js";

export const unknownSource = {
  file: null,
  line: 0,
  column: 0,
  end_line: 0,
  end_column: 0,
};

export const type = (kind, data) => ({ kind, data });
export const value = (kind, data) => ({ kind, data });
export const local = (id) => value("local", id);
export const constant = (scalar, data) =>
  value("constant", { type: scalar, value: data });
export const place = (kind, id) => ({
  base: { kind, data: id },
  projections: [],
});
export const statement = (kind, data) => ({
  kind: data === undefined ? { kind } : { kind, data },
  source: unknownSource,
});
export const attributes = (origin = "source", inline = "auto") => ({
  origin,
  inline,
  runtime_context: true,
});
export const assign = (destination, rvalue) =>
  statement("assign", { destination, value: rvalue });

export function executableMir() {
  const processLoop = statement("loop", {
    body: {
      statements: [
        assign(place("local", 1), {
          kind: "compare",
          data: { op: "less", lhs: local(0), rhs: local(4) },
        }),
        statement("if", {
          condition: local(1),
          then_block: {
            statements: [
              assign(place("local", 2), {
                kind: "load",
                data: place("state", 0),
              }),
              assign(place("local", 3), {
                kind: "load",
                data: place("param", 0),
              }),
              assign(place("local", 2), {
                kind: "binary",
                data: { op: "add", lhs: local(2), rhs: local(3) },
              }),
              assign(place("state", 0), { kind: "use", data: local(2) }),
              assign(place("local", 5), {
                kind: "process_frame",
                data: { offset: local(0) },
              }),
              statement("output_store", {
                output: 0,
                element: null,
                bounds: "unchecked",
                frame: local(5),
                value: local(2),
              }),
              assign(place("local", 0), {
                kind: "binary",
                data: { op: "add", lhs: local(0), rhs: constant("i32", 1) },
              }),
            ],
          },
          else_block: { statements: [statement("break")] },
        }),
      ],
    },
  });

  return {
    schema_version: SUPPORTED_MIR_SCHEMA_VERSION,
    config: { sample_rate: 48_000, block_size: 4 },
    source_files: [],
    log_sites: [],
    types: [type("scalar", "f32"), type("scalar", "bool"), type("scalar", "i32")],
    structs: [],
    interface: {
      inputs: [],
      outputs: [{ name: "out1", ty: 0 }],
      control_outputs: [],
      params: [
        {
          name: "step",
          ty: 0,
          default: {
            kind: "scalar",
            data: { type: "f32", value: 0.25 },
          },
          range: null,
          control: {
            scale: "linear",
            curve: null,
            unit: null,
            step: null,
            step_count: null,
          },
        },
      ],
      buffers: [],
      events: [],
      delegates: [],
    },
    state: [{ name: "phase", ty: 0, persistence: "snapshot", authored: true }],
    const_data: [],
    functions: [
      {
        name: "onda_processor_init",
        kind: { kind: "init" },
        attributes: attributes("compiler_generated", "always"),
        params: [],
        results: [],
        locals: [],
        body: {
          statements: [
            assign(place("state", 0), {
              kind: "use",
              data: constant("f32", 0),
            }),
          ],
        },
        source: unknownSource,
      },
      {
        name: "onda_process",
        kind: { kind: "process" },
        attributes: attributes("compiler_generated", "always"),
        params: [
          { name: "start_frame", ty: 2, mode: "value" },
          { name: "frames", ty: 2, mode: "value" },
          { name: "flags", ty: 2, mode: "value" },
        ],
        results: [],
        locals: [
          { name: "$frame", ty: 2 },
          { name: null, ty: 1 },
          { name: null, ty: 0 },
          { name: null, ty: 0 },
          { name: "$end_frame", ty: 2 },
          { name: "$logical_frame", ty: 2 },
        ],
        body: {
          statements: [
            assign(place("local", 0), {
              kind: "use",
              data: constant("i32", 0),
            }),
            assign(place("local", 4), {
              kind: "load",
              data: place("parameter", 1),
            }),
            assign(place("local", 4), {
              kind: "binary",
              data: {
                op: "add",
                lhs: local(4),
                rhs: constant("i32", 0),
              },
            }),
            processLoop,
          ],
        },
        source: unknownSource,
      },
    ],
    entry_points: { init: 0, process: 1 },
  };
}
