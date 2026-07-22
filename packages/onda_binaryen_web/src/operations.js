const NUMERIC = Object.freeze(["f32", "f64", "i32", "i64"]);
const INTEGER = Object.freeze(["i32", "i64"]);

// Binaryen backend capability table for the complete scalar MIR operation
// surface. Lowering consults this table before dispatch, and tests assert its
// exact operation set so a MIR schema addition cannot silently become an
// undocumented backend gap.
export const MIR_OPERATION_CAPABILITIES = Object.freeze({
  unary: Object.freeze({
    negate: NUMERIC,
    logical_not: Object.freeze(["bool"]),
    bit_not: INTEGER,
  }),
  binary: Object.freeze({
    add: NUMERIC,
    subtract: NUMERIC,
    multiply: NUMERIC,
    divide: NUMERIC,
    remainder: NUMERIC,
    bit_and: INTEGER,
    bit_or: INTEGER,
    bit_xor: INTEGER,
    shift_left: INTEGER,
    shift_right: INTEGER,
  }),
  compare: Object.freeze({
    equal: Object.freeze([...NUMERIC, "bool"]),
    not_equal: Object.freeze([...NUMERIC, "bool"]),
    less: NUMERIC,
    less_equal: NUMERIC,
    greater: NUMERIC,
    greater_equal: NUMERIC,
  }),
});

export function supportsMirOperation(kind, operation, scalar) {
  return MIR_OPERATION_CAPABILITIES[kind]?.[operation]?.includes(scalar) === true;
}
