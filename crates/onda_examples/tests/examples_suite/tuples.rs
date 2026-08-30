use super::*;

// ─── Tuple tests ──────────────────────────────────────────────────

const TUPLE_RETURN_BASIC: &str = r#"
outs { out1, out2 }

def calcPair(x):
  return (x * 2.0, x + 1.0)

sample {
  (a, b) = calcPair(3.0)
  out1 = a
  out2 = b
}
"#;

#[test]
fn tuple_return_and_destructure_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(TUPLE_RETURN_BASIC, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6); // 3.0 * 2.0
    assert_near(output[1], 4.0, 1e-6); // 3.0 + 1.0
}

#[test]
fn tuple_return_via_variable() {
    let src = r#"
outs { out1, out2 }

def calcPair(x):
  t = (x * 2.0, x + 1.0)
  return t

sample {
  (a, b) = calcPair(3.0)
  out1 = a
  out2 = b
}
"#;
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6); // 3.0 * 2.0
    assert_near(output[1], 4.0, 1e-6); // 3.0 + 1.0
}

#[test]
fn tuple_return_via_variable_chained() {
    let src = r#"
outs { out1, out2 }

def makePair(x):
  return (x * 2.0, x + 1.0)

def forward(x):
  t = makePair(x)
  return t

sample {
  (a, b) = forward(3.0)
  out1 = a
  out2 = b
}
"#;
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6); // 3.0 * 2.0
    assert_near(output[1], 4.0, 1e-6); // 3.0 + 1.0
}

const TUPLE_ELEMENT_ACCESS: &str = r#"
outs { out1 }

def makePair(x):
  return (x, x * 10.0)

def readSecond(x):
  p = makePair(x)
  return p[1]

sample {
  out1 = readSecond(5.0)
}
"#;

#[test]
fn tuple_element_access_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(TUPLE_ELEMENT_ACCESS, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 50.0, 1e-6); // 5.0 * 10.0
}

const TUPLE_LITERAL_ASSIGN: &str = r#"
outs { out1, out2 }

def addPair():
  p = (10.0, 20.0)
  return p[0] + p[1]

sample {
  out1 = addPair()
  (x, y) = (1.0, 2.0)
  out2 = x + y
}
"#;

#[test]
fn tuple_literal_assign_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(TUPLE_LITERAL_ASSIGN, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 30.0, 1e-6); // 10.0 + 20.0
    assert_near(output[1], 3.0, 1e-6); // 1.0 + 2.0
}

const TUPLE_MIXED_TYPES: &str = r#"
outs { out1, out2 }

def calcIdx(pos):
  pos_floor = floor(pos)
  idx = i32(pos_floor)
  t = pos - pos_floor
  return (idx, t)

sample {
  (idx, t) = calcIdx(3.7)
  out1 = f32(idx)
  out2 = t
}
"#;

#[test]
fn tuple_mixed_types_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(TUPLE_MIXED_TYPES, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 3.0, 1e-6); // floor(3.7) = 3
    assert_near(output[1], 0.7, 1e-5); // 3.7 - 3.0 = 0.7
}

const TUPLE_PARAM_BASIC: &str = r#"
outs { out1, out2 }

def sumPair(p: (f32, f32)):
  return p[0] + p[1]

def swapPair(p: (f32, f32)):
  return (p[1], p[0])

sample {
  out1 = sumPair((3.0, 7.0))
  (a, b) = swapPair((10.0, 20.0))
  out2 = a - b
}
"#;

#[test]
fn tuple_param_basic_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(TUPLE_PARAM_BASIC, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 10.0, 1e-6); // 3.0 + 7.0
    assert_near(output[1], 10.0, 1e-6); // 20.0 - 10.0
}

const TUPLE_PARAM_MIXED_TYPES: &str = r#"
outs { out1, out2 }

def extractPair(p: (i32, f32)):
  return (f32(p[0]) * 2.0, p[1] + 1.0)

sample {
  (a, b) = extractPair((3, 7.5))
  out1 = a
  out2 = b
}
"#;

#[test]
fn tuple_param_mixed_types_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_PARAM_MIXED_TYPES, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 6.0, 1e-6); // i32(3) * 2.0
    assert_near(output[1], 8.5, 1e-6); // 7.5 + 1.0
}

const TUPLE_STATE_BASIC: &str = r#"
outs { out1, out2 }

init:
  pair = (10.0, 20.0)

sample {
  out1 = pair[0]
  out2 = pair[1]
}
"#;

#[test]
fn tuple_state_basic_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(TUPLE_STATE_BASIC, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 10.0, 1e-6);
    assert_near(output[1], 20.0, 1e-6);
}

const TUPLE_STATE_WRITE: &str = r#"
ins { in1 }
outs { out1, out2 }

init:
  pair = (0.0, 0.0)

sample {
  pair[0] = in1
  pair[1] = in1 * 2.0
  out1 = pair[0]
  out2 = pair[1]
}
"#;

#[test]
fn tuple_state_write_compiles_and_runs() {
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(TUPLE_STATE_WRITE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 2);

    let input = vec![5.0_f32];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 5.0, 1e-6);
    assert_near(output[1], 10.0, 1e-6);
}

const TUPLE_STATE_PERSISTENCE: &str = r#"
ins { in1 }
outs { out1, out2 }

init:
  pair = (0.0, 0.0)

sample {
  out1 = pair[0]
  out2 = pair[1]
  pair[0] = pair[0] + in1
  pair[1] = pair[1] + 1.0
}
"#;

#[test]
fn tuple_state_persistence_compiles_and_runs() {
    let frames = 3;
    let (mut instance, in_channels, out_channels) =
        compile_instance(TUPLE_STATE_PERSISTENCE, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 2);

    let input = vec![1.0_f32, 2.0, 3.0];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    // Frame 0: out1=0, out2=0, then pair=(0+1, 0+1) = (1, 1)
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
    // Frame 1: out1=1, out2=1, then pair=(1+2, 1+1) = (3, 2)
    assert_near(output[2], 1.0, 1e-6);
    assert_near(output[3], 1.0, 1e-6);
    // Frame 2: out1=3, out2=2, then pair=(3+3, 2+1) = (6, 3)
    assert_near(output[4], 3.0, 1e-6);
    assert_near(output[5], 2.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Tuple: inferred tuple param (monomorphization)
// ---------------------------------------------------------------------------

// NOTE: Inferred (untyped) tuple params are not yet supported.
// `def sumPair(p)` called with `(3.0, 7.0)` doesn't infer p as a tuple.
// Must use explicit type: `def sumPair(p: (f32, f32))` instead.

// ---------------------------------------------------------------------------
// Tuple: destructure a tuple variable (in def scope)
// NOTE: `a = (1.0, 2.0, 3.0); (b,c,d) = a` only works inside def bodies.
// Sample-scope local tuple variables are not yet supported — use init state
// or a def wrapper instead.
// ---------------------------------------------------------------------------

#[test]
fn tuple_destructure_variable() {
    let src = r#"
outs { out1, out2, out3 }

def unpack():
  a = (1.0, 2.0, 3.0)
  (b, c, d) = a
  return (b, c, d)

sample {
  (x, y, z) = unpack()
  out1 = x
  out2 = y
  out3 = z
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
    assert_near(output[2], 3.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Tuple: larger tuples (4 elements)
// ---------------------------------------------------------------------------

#[test]
fn tuple_four_elements_compiles_and_runs() {
    let src = r#"
outs { out1, out2, out3, out4 }

def makeQuad(x):
  return (x, x * 2.0, x * 3.0, x * 4.0)

sample {
  (a, b, c, d) = makeQuad(5.0)
  out1 = a
  out2 = b
  out3 = c
  out4 = d
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 5.0, 1e-6);
    assert_near(output[1], 10.0, 1e-6);
    assert_near(output[2], 15.0, 1e-6);
    assert_near(output[3], 20.0, 1e-6);
}

#[test]
fn tuple_four_elements_index_access() {
    let src = r#"
outs { out1, out2 }

def makeQuad(x):
  return (x + 1.0, x + 2.0, x + 3.0, x + 4.0)

def firstAndLast(x):
  q = makeQuad(x)
  return (q[0], q[3])

sample {
  (a, b) = firstAndLast(10.0)
  out1 = a
  out2 = b
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 11.0, 1e-6);
    assert_near(output[1], 14.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Tuple: forwarding between defs
// ---------------------------------------------------------------------------

#[test]
fn tuple_param_forwarding_between_defs() {
    let src = r#"
outs { out1, out2 }

def scale(p: (f32, f32), factor):
  return (p[0] * factor, p[1] * factor)

def offset(p: (f32, f32), delta):
  return (p[0] + delta, p[1] + delta)

def scaleAndOffset(p: (f32, f32), factor, delta):
  scaled = scale(p, factor)
  return offset(scaled, delta)

sample {
  (a, b) = scaleAndOffset((2.0, 3.0), 10.0, 1.0)
  out1 = a
  out2 = b
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 21.0, 1e-6); // 2.0 * 10.0 + 1.0
    assert_near(output[1], 31.0, 1e-6); // 3.0 * 10.0 + 1.0
}

// ---------------------------------------------------------------------------
// Tuple: destructuring a tuple param inside a def
// ---------------------------------------------------------------------------

#[test]
fn tuple_param_destructure_inside_def() {
    let src = r#"
outs { out1, out2 }

def unpack(p: (f32, f32)):
  (x, y) = p
  return (y, x)

sample {
  (a, b) = unpack((100.0, 200.0))
  out1 = a
  out2 = b
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 200.0, 1e-6);
    assert_near(output[1], 100.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Tuple: return from if/else branches
// ---------------------------------------------------------------------------

#[test]
fn tuple_return_from_if_else_branches() {
    let src = r#"
outs { out1, out2 }

def pick(flag):
  if (flag > 0.5) { return (1.0, 2.0) } else { return (3.0, 4.0) }

sample {
  (a, b) = pick(1.0)
  out1 = a
  out2 = b
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
}

#[test]
fn tuple_return_from_if_else_branches_false_path() {
    let src = r#"
outs { out1, out2 }

def pick(flag):
  if (flag > 0.5) { return (1.0, 2.0) } else { return (3.0, 4.0) }

sample {
  (a, b) = pick(0.0)
  out1 = a
  out2 = b
}
"#;
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(src, frames);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 3.0, 1e-6);
    assert_near(output[1], 4.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Tuple: state + struct tuple state in the same processor
// ---------------------------------------------------------------------------

#[test]
fn tuple_state_and_struct_tuple_state_coexist() {
    let src = r#"
ins { in1 }
outs { out1, out2, out3, out4 }

struct Accum { pair: (f32, f32) = (0.0, 0.0) }

init:
  raw_pair = (0.0, 0.0)
  acc = Accum()

sample {
  out1 = raw_pair[0]
  out2 = raw_pair[1]
  out3 = acc.pair[0]
  out4 = acc.pair[1]
  raw_pair[0] = raw_pair[0] + in1
  raw_pair[1] = raw_pair[1] + 1.0
  acc.pair[0] = acc.pair[0] + in1 * 2.0
  acc.pair[1] = acc.pair[1] + 10.0
}
"#;
    let frames = 3;
    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);
    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 4);

    let input = vec![1.0_f32, 2.0, 3.0];
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    // Frame 0: raw=(0,0) acc=(0,0) → raw=(1,1) acc=(2,10)
    assert_near(output[0], 0.0, 1e-6);
    assert_near(output[1], 0.0, 1e-6);
    assert_near(output[2], 0.0, 1e-6);
    assert_near(output[3], 0.0, 1e-6);
    // Frame 1: raw=(1,1) acc=(2,10) → raw=(3,2) acc=(6,20)
    assert_near(output[4], 1.0, 1e-6);
    assert_near(output[5], 1.0, 1e-6);
    assert_near(output[6], 2.0, 1e-6);
    assert_near(output[7], 10.0, 1e-6);
    // Frame 2: raw=(3,2) acc=(6,20) → raw=(6,3) acc=(12,30)
    assert_near(output[8], 3.0, 1e-6);
    assert_near(output[9], 2.0, 1e-6);
    assert_near(output[10], 6.0, 1e-6);
    assert_near(output[11], 20.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Tuple: negative / error tests
// ---------------------------------------------------------------------------

#[test]
fn tuple_dynamic_index_rejected() {
    let src = r#"
outs { out1 }
def getPair():
  return (1.0, 2.0)
sample {
  p = getPair()
  i = 0
  out1 = p[i]
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(result.is_err(), "dynamic tuple index should be rejected");
}

#[test]
fn tuple_out_of_bounds_index_rejected() {
    let src = r#"
outs { out1 }
def getPair():
  return (1.0, 2.0)
sample {
  p = getPair()
  out1 = p[2]
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "out-of-bounds tuple index should be rejected"
    );
}

#[test]
fn tuple_destructure_arity_mismatch_rejected() {
    let src = r#"
outs { out1 }
def getPair():
  return (1.0, 2.0)
sample {
  (a, b, c) = getPair()
  out1 = a
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "tuple destructuring arity mismatch should be rejected"
    );
}

// ---------------------------------------------------------------------------
// Phase 4: Local tuple variables in sample/block/init scopes
// ---------------------------------------------------------------------------

const TUPLE_LOCAL_SAMPLE_LITERAL: &str = r#"
outs { out1, out2 }
sample {
  a = (3.0, 7.0)
  out1 = a[0]
  out2 = a[1]
}
"#;

#[test]
fn tuple_local_sample_literal() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(TUPLE_LOCAL_SAMPLE_LITERAL, frames);
    assert_eq!(out_channels, 2);
    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 3.0, 1e-6);
    assert_near(output[1], 7.0, 1e-6);
}

const TUPLE_LOCAL_SAMPLE_COPY: &str = r#"
outs { out1, out2 }
sample {
  a = (10.0, 20.0)
  b = a
  out1 = b[0]
  out2 = b[1]
}
"#;

#[test]
fn tuple_local_sample_copy() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(TUPLE_LOCAL_SAMPLE_COPY, frames);
    assert_eq!(out_channels, 2);
    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 10.0, 1e-6);
    assert_near(output[1], 20.0, 1e-6);
}

const TUPLE_LOCAL_SAMPLE_FROM_CALL: &str = r#"
outs { out1, out2 }
def makePair(x):
  return (x, x * 2.0)
sample {
  p = makePair(5.0)
  out1 = p[0]
  out2 = p[1]
}
"#;

#[test]
fn tuple_local_sample_from_call() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(TUPLE_LOCAL_SAMPLE_FROM_CALL, frames);
    assert_eq!(out_channels, 2);
    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 5.0, 1e-6);
    assert_near(output[1], 10.0, 1e-6);
}

#[test]
fn typed_tuple_state_and_locals_use_declared_element_types() {
    let src = r#"
outs:
  out1

def pair() -> (f32, i32):
  return (1.0, 2)

init:
  state: (f64, i64) = pair()

sample:
  local: (f64, i64) = pair()
  out1 = f32(state[0]) + f32(state[1]) + f32(local[0]) + f32(local[1])
"#;
    let (mut instance, _, out_channels) = compile_instance(src, 1);
    assert_eq!(out_channels, 1);
    let mut output = vec![0.0_f32; 1];
    process_interleaved(&mut instance, &[], &mut output, 1).expect("process should succeed");
    assert_near(output[0], 6.0, 1e-6);
}

const TUPLE_LOCAL_SAMPLE_DESTRUCTURE: &str = r#"
outs { out1, out2, out3 }
sample {
  a = (1.0, 2.0, 3.0)
  (x, y, z) = a
  out1 = x
  out2 = y
  out3 = z
}
"#;

#[test]
fn tuple_local_sample_destructure() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(TUPLE_LOCAL_SAMPLE_DESTRUCTURE, frames);
    assert_eq!(out_channels, 3);
    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 1.0, 1e-6);
    assert_near(output[1], 2.0, 1e-6);
    assert_near(output[2], 3.0, 1e-6);
}

const TUPLE_LOCAL_BLOCK: &str = r#"
outs { out1, out2 }
block {
  sample {
    pair = (100.0, 200.0)
    out1 = pair[0]
    out2 = pair[1]
  }
}
"#;

#[test]
fn tuple_local_block() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(TUPLE_LOCAL_BLOCK, frames);
    assert_eq!(out_channels, 2);
    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 100.0, 1e-6);
    assert_near(output[1], 200.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Phase 4: Inferred (untyped) tuple parameters via monomorphization
// ---------------------------------------------------------------------------

const TUPLE_INFERRED_PARAM: &str = r#"
outs { out1, out2 }
def swapPair(p):
  return (p[1], p[0])
sample {
  (a, b) = swapPair((3.0, 7.0))
  out1 = a
  out2 = b
}
"#;

#[test]
fn tuple_inferred_param() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(TUPLE_INFERRED_PARAM, frames);
    assert_eq!(out_channels, 2);
    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames * out_channels];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 7.0, 1e-6);
    assert_near(output[1], 3.0, 1e-6);
}

const TUPLE_INFERRED_PARAM_SUM: &str = r#"
outs { out1 }
def sumPair(p):
  return p[0] + p[1]
sample {
  out1 = sumPair((10.0, 25.0))
}
"#;

#[test]
fn tuple_inferred_param_sum() {
    let frames = 1;
    let (mut instance, _, out_channels) = compile_instance(TUPLE_INFERRED_PARAM_SUM, frames);
    assert_eq!(out_channels, 1);
    let input = Vec::<f32>::new();
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_near(output[0], 35.0, 1e-6);
}

// ---------------------------------------------------------------------------
// Phase 4: Tuple index validation for local vars
// ---------------------------------------------------------------------------

#[test]
fn tuple_local_dynamic_index_rejected() {
    let src = r#"
outs { out1 }
sample {
  a = (1.0, 2.0)
  i = 0
  out1 = a[i]
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "dynamic index on local tuple should be rejected"
    );
}

#[test]
fn tuple_local_out_of_bounds_rejected() {
    let src = r#"
outs { out1 }
sample {
  a = (1.0, 2.0)
  out1 = a[2]
}
"#;
    let parsed = parse_program(src).expect("parse should succeed");
    let result = analyze(parsed);
    assert!(
        result.is_err(),
        "out-of-bounds index on local tuple should be rejected"
    );
}
