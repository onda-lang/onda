use super::*;

fn lower_test_mir(src: &str) -> onda_mir::Program {
    let parsed = parse_program(src).expect("parse should succeed");
    let typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,
            block_size: 4,
        },
    )
    .expect("analysis should succeed");
    onda_semantics::lower_program_to_optimized_mir(&typed)
        .expect("MIR lowering should succeed")
        .into_program()
}

fn assert_mir_scalar_specialization(
    mir: &onda_mir::Program,
    name_fragment: &str,
    expected: onda_mir::ScalarType,
) {
    let matching = mir
        .functions
        .iter()
        .filter(|function| function.name.contains(name_fragment))
        .collect::<Vec<_>>();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one MIR function containing '{name_fragment}', got {:?}",
        mir.functions
            .iter()
            .map(|function| function.name.as_str())
            .collect::<Vec<_>>()
    );
    let function = matching[0];
    assert_eq!(
        function.params.len(),
        1,
        "'{name_fragment}' should have one scalar parameter"
    );
    assert_eq!(
        function.results.len(),
        1,
        "'{name_fragment}' should have one scalar result"
    );
    assert_eq!(
        mir.types[function.params[0].ty.index()],
        onda_mir::Type::Scalar(expected),
        "'{name_fragment}' parameter must retain its specialized MIR type"
    );
    assert_eq!(
        mir.types[function.results[0].index()],
        onda_mir::Type::Scalar(expected),
        "'{name_fragment}' result must retain its specialized MIR type"
    );
}

// ── Generic Defs ─────────────────────────────────────────────────────────────

#[test]

fn generic_def_identity_inferred_compile_and_run() {
    let src = r#"

outs { out1 }

def identity<T>(x: T) {

  return x

}

sample {

  out1 = identity(3.5)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 3.5, 1e-6);
    }
}

#[test]

fn generic_def_cast_in_body_compile_and_run() {
    let src = r#"

outs { out1 }

def half<T>(x: T) {

  return x * T(0.5)

}

sample {

  out1 = half(6.0)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 3.0, 1e-6);
    }
}

#[test]

fn generic_def_explicit_type_arg_compile_and_run() {
    let src = r#"

outs { out1 }

def make<T>(x: T) {

  return x + T(1)

}

sample {

  out1 = make<f32>(2.0)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 3.0, 1e-6);
    }
}

#[test]

fn generic_def_multiple_type_params_compile_and_run() {
    let src = r#"

outs { out1 }

def combine<T, U>(a: T, b: U) {

  return a + T(b)

}

sample {

  out1 = combine(1.5, f64(2.5))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 4.0, 1e-6);
    }
}

#[test]

fn generic_def_no_arg_requires_explicit_type_compile_and_run() {
    let src = r#"

outs { out1 }

def zero<T>() {

  return T(0)

}

sample {

  out1 = zero<f32>() + 1.0

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 1.0, 1e-6);
    }
}

#[test]

fn generic_def_overload_concrete_wins() {
    let src = r#"

outs { out1 }

def process(x: f32) {

  return x * 2.0

}

def process<T>(x: T) {

  return x * T(3)

}

sample {

  out1 = process(5.0)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6); // concrete f32 version: 5 * 2 = 10
    }
}

#[test]

fn generic_def_multiple_specializations_compile_and_run() {
    let src = r#"

outs { out1 }

def double<T>(x: T) {

  return x + x

}

sample {

  a = double(1.5)

  b = f32(double(f64(2.25)))

  out1 = a + b

}

"#;

    // MIR must retain both concrete specializations without relying on a
    // backend-specific LLVM symbol spelling.

    let mir = lower_test_mir(src);
    assert_mir_scalar_specialization(&mir, "double.__onda_mono__g_f32", onda_mir::ScalarType::F32);
    assert_mir_scalar_specialization(&mir, "double.__onda_mono__g_f64", onda_mir::ScalarType::F64);

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 7.5, 1e-6); // 3.0 + 4.5
    }
}

#[test]

fn generic_struct_method_with_own_type_param_compile_and_run() {
    let src = r#"

outs { out1 }

struct Holder<T> {

  val: T = 0.0



  def scale<U>(self, factor: U) {

    return self.val * T(factor)

  }

}

init {

  h: Holder<f32> = Holder(val = 10.0)

}

sample {

  out1 = h.scale(f64(0.5))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 5.0, 1e-6);
    }
}

#[test]

fn generic_def_shadow_struct_type_param_error() {
    let src = r#"

outs { out1 }

struct Box<T> {

  val: T = 0.0

  def bad<T>(self, x: T) {

    return x

  }

}

sample { out1 = 0.0 }

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "method type param shadowing struct type param should be rejected"
    );

    let errors = result.unwrap_err();

    let msg = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        msg.contains("shadows"),
        "error should mention shadowing, got: {msg}"
    );
}

#[test]

fn generic_def_shadow_proc_type_param_error() {
    let src = r#"

outs { out1 }

proc P<T> {

  outs { out1 }

  def bad<T>(x: T) {

    return x

  }

  sample { out1 = T(0) }

}

init { p = P<f32>() }

sample { out1 = p() }

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "proc-local def type param shadowing proc type param should be rejected"
    );

    let errors = result.unwrap_err();

    let msg = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        msg.contains("shadows"),
        "error should mention shadowing, got: {msg}"
    );
}

#[test]

fn generic_def_duplicate_type_param_error() {
    let src = r#"

outs { out1 }

def bad<T, T>(x: T) {

  return x

}

sample { out1 = 0.0 }

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(result.is_err(), "duplicate type param should be rejected");

    let errors = result.unwrap_err();

    let msg = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        msg.contains("duplicate"),
        "error should mention duplicate, got: {msg}"
    );
}

#[test]

fn generic_def_bool_type_arg_rejected() {
    let src = r#"

outs { out1 }

def identity<T>(x: T) {

  return x

}

sample {

  out1 = f32(identity<bool>(true))

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "bool as generic type argument should be rejected"
    );
}

#[test]

fn generic_def_wrong_type_arg_count_rejected() {
    let src = r#"

outs { out1 }

def identity<T>(x: T) {

  return x

}

sample {

  out1 = identity<f32, f64>(1.0)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "wrong number of type args should be rejected"
    );
}

#[test]

fn generic_def_non_generic_rejects_type_args() {
    let src = r#"

outs { out1 }

def id(x) {

  return x

}

sample {

  out1 = id<f32>(1.0)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "type args on non-generic def should be rejected"
    );
}

#[test]

fn generic_def_proc_local_compile_and_run() {
    let src = r#"

outs { out1 }

proc Scaler<T> {

  outs<T> { out1 }

  def scale<U>(val: T, factor: U) {

    return val * T(factor)

  }

  sample {

    out1 = scale(T(10), f64(0.3))

  }

}

init {

  s = Scaler<f32>()

}

sample {

  out1 = s()

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 3.0, 1e-6);
    }
}

// ── Corner-case tests for generic defs ──────────────────────────────────

#[test]

fn generic_def_to_def_call_compile_and_run() {
    // One generic def calls another generic def (def-to-def mono).

    let src = r#"

outs { out1 }

def double<T>(x: T) {

  return x + x

}

def quad<T>(x: T) {

  return double(double(x))

}

sample {

  out1 = quad(2.5)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6); // 2.5 * 4 = 10.0
    }
}

#[test]

fn generic_def_integer_specialization_compile_and_run() {
    // Specialize a generic def to i32 and i64.

    let src = r#"

outs { out1 }

def triple<T>(x: T) {

  return x + x + x

}

sample {

  a = triple(i32(5))

  b = triple(i64(3))

  out1 = f32(a) + f32(b)

}

"#;

    // The MIR signatures are the backend-neutral proof that both integer
    // specializations use integer values rather than floating-point ones.

    let mir = lower_test_mir(src);
    assert_mir_scalar_specialization(&mir, "triple.__onda_mono__g_i32", onda_mir::ScalarType::I32);
    assert_mir_scalar_specialization(&mir, "triple.__onda_mono__g_i64", onda_mir::ScalarType::I64);

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 24.0, 1e-6); // 15 + 9
    }
}

#[test]

fn generic_def_inference_from_cast_compile_and_run() {
    // Type inference from a cast expression: id(f64(x)) should infer T = f64.

    let src = r#"

outs { out1 }

def id<T>(x: T) {

  return x

}

sample {

  out1 = f32(id(f64(7.5)))

}

"#;

    // MIR must show the f64 specialization inferred from the f64() cast.

    let mir = lower_test_mir(src);
    assert_mir_scalar_specialization(&mir, "id.__onda_mono__g_f64", onda_mir::ScalarType::F64);

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 7.5, 1e-6);
    }
}

#[test]

fn generic_def_explicit_overrides_inference_compile_and_run() {
    // Explicit <f64> overrides what inference (f32) would give.

    let src = r#"

outs { out1 }

def id<T>(x: T) {

  return x

}

sample {

  out1 = f32(id<f64>(3.25))

}

"#;

    // MIR must show f64 specialization from explicit <f64>, not f32 from
    // default literal inference.

    let mir = lower_test_mir(src);
    assert_mir_scalar_specialization(&mir, "id.__onda_mono__g_f64", onda_mir::ScalarType::F64);

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 3.25, 1e-6);
    }
}

#[test]

fn generic_def_chained_calls_compile_and_run() {
    // Chained generic calls: id(id(1.0)).

    let src = r#"

outs { out1 }

def id<T>(x: T) {

  return x

}

sample {

  out1 = id(id(4.0))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 4.0, 1e-6);
    }
}

#[test]

fn generic_def_called_from_init_scope_compile_and_run() {
    // Generic def called from init scope (not just sample).

    let src = r#"

outs { out1 }

def make<T>(x: T) {

  return x * x

}

init {

  val = make(3.0)

}

sample {

  out1 = val

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 9.0, 1e-6);
    }
}

#[test]

fn generic_def_non_generic_struct_with_generic_method_compile_and_run() {
    // Non-generic struct with a generic method.

    let src = r#"

outs { out1 }

struct Adder {

  base: f32 = 0.0



  def add<U>(self, x: U) {

    return self.base + f32(x)

  }

}

init {

  a = Adder(base = 10.0)

}

sample {

  out1 = a.add(f64(2.5))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 12.5, 1e-6);
    }
}

#[test]

fn generic_def_mixed_generic_and_untyped_param_compile_and_run() {
    // Mixed generic type param + untyped param (Phase 3 interaction).

    let src = r#"

outs { out1 }

def apply<T>(scale: T, x) {

  return T(x) * scale

}

sample {

  out1 = f32(apply(f64(2.0), 3.0))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 6.0, 1e-6);
    }
}

#[test]

fn generic_def_same_type_param_two_args_compile_and_run() {
    // Two params with same type param T, both inferred from same-type args.

    let src = r#"

outs { out1 }

def add<T>(a: T, b: T) {

  return a + b

}

sample {

  out1 = add(2.5, 3.5)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 6.0, 1e-6);
    }
}

#[test]

fn generic_def_with_default_param_compile_and_run() {
    // Generic def with a default parameter value using T cast.

    let src = r#"

outs { out1 }

def inc<T>(x: T, step: T = T(1)) {

  return x + step

}

sample {

  out1 = inc(4.0)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 5.0, 1e-6);
    }
}

// ── Generic Def: T[] param ───────────────────────────────────────────────────

#[test]

fn generic_def_t_slice_param_compile_and_run() {
    let src = r#"

outs { out1 }

init {

  data: f32[4] = [1.0, 2.0, 3.0, 4.0]

}

def sum<T>(arr: T[]) {

  result = T(0)

  for i in 0..(arr.len()) {

    result = result + arr[i]

  }

  return result

}

sample {

  out1 = sum(data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_t_slice_param_i32_compile_and_run() {
    let src = r#"

outs { out1 }

init {

  data: i32[3] = [10, 20, 30]

}

def first<T>(arr: T[]) {

  return arr[0]

}

sample {

  out1 = f32(first(data))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_t_slice_infers_from_slice_expression_compile_and_run() {
    let src = r#"

outs:

  out1: i64



def first<T>(arr: T[]):

  return arr[0]



sample:

  vals: i64[3] = [11, 22, 33]

  out1 = first(vals[1:])

"#;

    let frames = 4;

    let (mut instance, _, out_channels) = compile_instance(src, frames);

    assert_eq!(out_channels, 1);

    let mut out_i64_bytes = vec![0_u8; frames * std::mem::size_of::<i64>()];

    bind_output(
        &mut instance,
        0,
        out_i64_bytes.as_mut_ptr(),
        out_i64_bytes.len(),
    )
    .expect("bind i64 output");

    process_checked(&mut instance, frames).expect("process checked");

    for sample in decode_planar_i64(&out_i64_bytes) {
        assert_eq!(sample, 22);
    }
}

#[test]

fn generic_def_t_slice_with_scalar_t_param_compile_and_run() {
    let src = r#"

outs { out1 }

init {

  data: f32[3] = [2.0, 4.0, 6.0]

}

def scale_sum<T>(arr: T[], factor: T) {

  result = T(0)

  for i in 0..(arr.len()) {

    result = result + arr[i] * factor

  }

  return result

}

sample {

  out1 = scale_sum(data, 0.5)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 6.0, 1e-6);
    }
}

// ── Generic Def: buffer<T> param ────────────────────────────────────────────

#[test]

fn generic_def_buffer_t_param_compile_and_run() {
    // Test that def with buffer<T> param compiles and monomorphizes correctly.

    // We only test compilation, not runtime, since buffer binding in tests is complex.

    let src = r#"

outs { out1 }

buffers {

  ext: buffer<f32>

}

def read_first<T>(buf: buffer<T>) {

  return buf[0]

}

sample {

  out1 = read_first(ext)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let _typed = analyze_with_options(parsed, AnalysisOptions::default())
        .expect("semantic analysis should succeed");
}

#[test]

fn generic_def_buffer_t_explicit_compile_and_run() {
    // Test with explicit type arg.

    let src = r#"

outs { out1 }

buffers {

  ext: buffer<f32>

}

def read_first<T>(buf: buffer<T>) {

  return buf[0]

}

sample {

  out1 = read_first<f32>(ext)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let _typed = analyze_with_options(parsed, AnalysisOptions::default())
        .expect("semantic analysis should succeed");
}

// ── Generic Def: buffer<T[N]> param ─────────────────────────────────────────

#[test]

fn generic_def_buffer_t_stereo_compile_and_run() {
    // buffer<T[2]> — generic element type with explicit stereo channels.

    // Mirrors DEF_BUFFER_STEREO_PARAM_EXAMPLE but with generic T.

    let src = r#"

buffers {

  buf1: buffer<f32[2]>

}

outs {

  out1

}

def read_r<T>(b: buffer<T[2]>, i: i32) {

  return b[1, i]

}

init {

  idx: i32 = 0

}

sample {

  out1 = read_r(buf1, idx)

  idx = idx + 1

}

"#;

    let frames = 6;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    let expected = [10.0_f32, 20.0, 30.0, 40.0, 40.0, 40.0];

    for (sample, target) in out.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn generic_def_buffer_t_mono_inferred_compile_and_run() {
    // buffer<T> with mono buffer — T inferred from argument.

    let src = r#"

buffers {

  buf1: buffer<f32>

}

outs {

  out1

}

def read_first<T>(b: buffer<T>) {

  return b[0]

}

sample {

  out1 = read_first(buf1)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf = vec![10.0_f32, 20.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    // read_first reads buf[0] = 10.0 for every sample

    for sample in &out {
        assert_near(*sample, 10.0, 1e-6);
    }
}

// ── Generic Def: T[N] param ─────────────────────────────────────────────────

#[test]

fn generic_def_t_sized_array_param_compile_and_run() {
    let src = r#"

outs { out1 }

init {

  data: f32[4] = [1.0, 2.0, 3.0, 4.0]

}

def sum4<T>(arr: T[4]) {

  return arr[0] + arr[1] + arr[2] + arr[3]

}

sample {

  out1 = sum4(data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_t_sized_array_with_cast_compile_and_run() {
    let src = r#"

outs { out1 }

init {

  data: f64[3] = [f64(1.0), f64(2.0), f64(3.0)]

}

def avg3<T>(arr: T[3]) {

  return (arr[0] + arr[1] + arr[2]) * T(1.0) / T(3.0)

}

sample {

  out1 = f32(avg3(data))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 2.0, 1e-6);
    }
}

#[test]

fn generic_def_concrete_sized_array_param_compile_and_run() {
    let src = r#"

outs { out1 }

init {

  data: f32[2] = [3.0, 7.0]

}

def add_pair(arr: f32[2]) {

  return arr[0] + arr[1]

}

sample {

  out1 = add_pair(data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6);
    }
}

// ── Generic Def: non-f32 element types ──────────────────────────────────────

#[test]

fn generic_def_t_slice_f64_compile_and_run() {
    // T[] with f64 arrays — verifies non-f32 element type inference for slices.

    let src = r#"

outs { out1 }

init {

  data: f64[3]

  data[0] = f64(10)

  data[1] = f64(20)

  data[2] = f64(30)

}

def first<T>(arr: T[]) {

  return arr[0]

}

sample {

  out1 = f32(first(data))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_t_sized_array_i32_compile_and_run() {
    // T[N] with i32 — verifies non-f32 element type for sized arrays.

    let src = r#"

outs { out1 }

init {

  data: i32[4]

  data[0] = i32(10)

  data[1] = i32(20)

  data[2] = i32(30)

  data[3] = i32(40)

}

def sum4<T>(arr: T[4]) {

  return arr[0] + arr[1] + arr[2] + arr[3]

}

sample {

  out1 = f32(sum4(data))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 100.0, 1e-6);
    }
}

#[test]

fn generic_def_buffer_f64_compile_and_run() {
    // buffer<T> with f64 buffer — verifies non-f32 buffer element type inference.

    let src = r#"

outs { out1 }

buffers {

  ext: buffer<f64>

}

def read_first<T>(buf: buffer<T>) {

  return buf[0]

}

sample {

  out1 = f32(read_first(ext))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf = vec![42.0_f64, 99.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F64,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in &out {
        assert_near(*sample, 42.0, 1e-6);
    }
}

#[test]

fn generic_def_buffer_i32_compile_and_run() {
    // buffer<T> with i32 buffer.

    let src = r#"

outs { out1 }

buffers {

  ext: buffer<i32>

}

def read_first<T>(buf: buffer<T>) {

  return f32(buf[0])

}

sample {

  out1 = read_first(ext)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf = vec![7_i32, 13];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::I32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in &out {
        assert_near(*sample, 7.0, 1e-6);
    }
}

// ── Generic Def: interaction scenarios ──────────────────────────────────────

#[test]

fn generic_def_scalar_t_and_t_slice_compile_and_run() {
    // T used as both scalar param and T[] array param in same def.

    let src = r#"

outs { out1 }

init {

  data: f32[3]

  data[0] = 1.0

  data[1] = 2.0

  data[2] = 3.0

}

def scale_sum<T>(scale: T, arr: T[]) {

  result: T = T(0)

  for i in 0..(arr.len()) {

    result = result + arr[i] * scale

  }

  return result

}

sample {

  out1 = scale_sum(0.5, data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    // (1.0 + 2.0 + 3.0) * 0.5 = 3.0

    for s in &output {
        assert_near(*s, 3.0, 1e-6);
    }
}

#[test]

fn generic_def_scalar_t_and_buffer_t_compile_and_run() {
    // T used as both scalar param and buffer<T> in same def.

    let src = r#"

outs { out1 }

buffers {

  ext: buffer<f32>

}

def weighted_read<T>(scale: T, buf: buffer<T>) {

  return buf[0] * scale

}

sample {

  out1 = weighted_read(2.0, ext)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf = vec![5.0_f32, 10.0];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        buf.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in &out {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_multiple_buffers_compile_and_run() {
    // Two buffer<T> params in the same generic def.

    let src = r#"

outs { out1 }

buffers {

  a: buffer<f32>

  b: buffer<f32>

}

def add_bufs<T>(b1: buffer<T>, b2: buffer<T>) {

  return b1[0] + b2[0]

}

sample {

  out1 = add_bufs(a, b)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf_a = vec![3.0_f32, 6.0];

    let mut buf_b = vec![7.0_f32, 14.0];

    bind_buffer(
        &mut instance,
        0,
        buf_a.as_mut_ptr().cast::<u8>(),
        buf_a.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer a");

    bind_buffer(
        &mut instance,
        1,
        buf_b.as_mut_ptr().cast::<u8>(),
        buf_b.len(),
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer b");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    for sample in &out {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_def_to_def_with_t_slice_compile_and_run() {
    // Generic def calling another generic def, both with T[] params.

    let src = r#"

outs { out1 }

init {

  data: f32[4]

  data[0] = 1.0

  data[1] = 2.0

  data[2] = 3.0

  data[3] = 4.0

}

def sum<T>(arr: T[]) {

  result: T = T(0)

  for i in 0..(arr.len()) {

    result = result + arr[i]

  }

  return result

}

def avg<T>(arr: T[]) {

  return sum(arr) / T(arr.len())

}

sample {

  out1 = avg(data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    // (1+2+3+4)/4 = 2.5

    for s in &output {
        assert_near(*s, 2.5, 1e-6);
    }
}

// ── Generic Def: T[N] / buffer<T[N]> with namespace params and consts ───────

#[test]

fn generic_def_t_sized_array_namespace_param_compile_and_run() {
    // T[N] where N is a namespace generic parameter.

    let src = r#"

namespace DSP<Size = 4> {

  def sum_all<T>(arr: T[Size]) {

    result: T = T(0)

    for i in 0..Size {

      result = result + arr[i]

    }

    return result

  }

}

outs { out1 }

init {

  data: f32[4]

  data[0] = 1.0

  data[1] = 2.0

  data[2] = 3.0

  data[3] = 4.0

}

sample {

  out1 = DSP<4>::sum_all(data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_t_sized_array_const_size_compile_and_run() {
    // T[N] where N is a const.

    let src = r#"

const LEN = 3

outs { out1 }

init {

  data: f32[LEN]

  data[0] = 5.0

  data[1] = 10.0

  data[2] = 15.0

}

def first<T>(arr: T[LEN]) {

  return arr[0]

}

sample {

  out1 = first(data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 5.0, 1e-6);
    }
}

#[test]

fn generic_def_t_sized_array_namespace_const_compile_and_run() {
    // T[N] where N is a namespace-level const.

    let src = r#"

namespace Filter<Order = 2> {

  const TAPS = Order + 1



  def dot<T>(coeffs: T[TAPS], state: T[TAPS]) {

    result: T = T(0)

    for i in 0..TAPS {

      result = result + coeffs[i] * state[i]

    }

    return result

  }

}

outs { out1 }

init {

  c: f32[3]

  c[0] = 0.25

  c[1] = 0.5

  c[2] = 0.25

  s: f32[3]

  s[0] = 1.0

  s[1] = 2.0

  s[2] = 3.0

}

sample {

  out1 = Filter<2>::dot(c, s)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    // 0.25*1 + 0.5*2 + 0.25*3 = 0.25 + 1.0 + 0.75 = 2.0

    for s in &output {
        assert_near(*s, 2.0, 1e-6);
    }
}

#[test]

fn generic_def_t_sized_array_namespace_expr_compile_and_run() {
    // T[N*2] where N is a namespace param used in an expression.

    let src = r#"

namespace Block<N = 2> {

  def sum_all<T>(arr: T[N * 2]) {

    result: T = T(0)

    for i in 0..(N * 2) {

      result = result + arr[i]

    }

    return result

  }

}

outs { out1 }

init {

  data: f32[4]

  data[0] = 1.0

  data[1] = 2.0

  data[2] = 3.0

  data[3] = 4.0

}

sample {

  out1 = Block<2>::sum_all(data)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_buffer_t_namespace_channels_compile_and_run() {
    // buffer<T[N]> where N is a namespace generic parameter for channel count.

    let src = r#"

namespace IO<Channels = 2> {

  def read_ch<T>(buf: buffer<T[Channels]>, ch: i32) {

    return buf[ch, 0]

  }

}

outs { out1 }

buffers {

  ext: buffer<f32[2]>

}

sample {

  out1 = IO<2>::read_ch(ext, 1)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    // buf[1, 0] = right channel of frame 0 = 10.0 for every sample

    for sample in &out {
        assert_near(*sample, 10.0, 1e-6);
    }
}

#[test]

fn generic_def_buffer_t_const_channels_compile_and_run() {
    // buffer<T[CH]> where CH is a const.

    let src = r#"

const CH = 2

outs { out1 }

buffers {

  ext: buffer<f32[CH]>

}

def sum_channels<T>(buf: buffer<T[CH]>) {

  return buf[0, 0] + buf[1, 0]

}

sample {

  out1 = sum_channels(ext)

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut buf = vec![
        3.0_f32, 7.0, //
        6.0, 14.0, //
        9.0, 21.0, //
        12.0, 28.0,
    ];

    bind_buffer(
        &mut instance,
        0,
        buf.as_mut_ptr().cast::<u8>(),
        4,
        2,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind buffer");

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f32>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f32(&out_bytes);

    // buf[0, 0] + buf[1, 0] = left ch + right ch of frame 0 = 3.0 + 7.0 = 10.0 for every sample

    for sample in &out {
        assert_near(*sample, 10.0, 1e-6);
    }
}

// --- Struct array root field access error tests ---

const STRUCT_ARRAY_ROOT_FIELD_ACCESS_ERROR: &str = r#"

namespace multichannel<N = 1>:

  struct Audio<T>:

    data: T[N]



const Chans = 10



outs Chans



init:

  data: multichannel<Chans>::Audio<f32>[2]



sample:

  for i in 0..Chans:

    outs[i] = data.data[i]

"#;

#[test]

fn struct_array_root_field_access_flattens_to_field_array() {
    let parsed = parse_program(STRUCT_ARRAY_ROOT_FIELD_ACCESS_ERROR).expect("parse should succeed");

    let typed = analyze(parsed).expect("field access on struct array root should analyze");

    assert!(
        typed
            .array_vars
            .iter()
            .any(|var| var.name == "data.data" && var.len == 20),
        "expected flattened struct field array metadata, got {:?}",
        typed.array_vars
    );
}

// --- Generic def default type param (f32) ---

#[test]

fn generic_def_no_arg_defaults_to_f32() {
    // zero<T>() with no explicit type arg should default T to f32.

    let src = r#"

outs { out1 }

def zero<T>():

  return T(0)

sample:

  out1 = zero()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 0.0, 1e-6);
    }
}

// --- Generic def: same type param, mismatched arg types (first arg wins) ---

#[test]

fn generic_def_same_type_param_first_arg_wins_compile_and_run() {
    // def add<T>(a: T, b: T) called with (f32, i32) — T inferred as f32 from first arg.

    let src = r#"

outs { out1 }

def add<T>(a: T, b: T) {

  return a + b

}

sample {

  out1 = add(1.5, i32(3))

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 4.5, 1e-6);
    }
}

// --- Generic def in non-generic proc ---

#[test]

fn generic_def_in_non_generic_proc_compile_and_run() {
    // Proc-local generic def inside a non-generic proc.

    let src = r#"

outs { out1 }

proc Gain {

  ins { in1 }

  outs { out1 }



  def scale<T>(x: T, factor: T) {

    return x * factor

  }



  sample {

    out1 = scale(in1, 0.5)

  }

}

init {

  g = Gain()

}

sample {

  g(4.0)

  out1 = g.out1

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 2.0, 1e-6);
    }
}

// --- Generic method on non-generic struct inside non-generic proc ---

#[test]

fn generic_struct_method_in_non_generic_proc_compile_and_run() {
    // Non-generic struct with generic method, used inside a non-generic proc.

    let src = r#"

outs { out1 }

struct Scaler {

  factor: f32 = 1.0



  def apply<T>(self, x: T) {

    return self.factor * f32(x)

  }

}

proc Fx {

  ins { in1 }

  outs { out1 }



  init {

    s = Scaler(factor = 0.25)

  }

  sample {

    out1 = s.apply(in1)

  }

}

init {

  fx = Fx()

}

sample {

  fx(8.0)

  out1 = fx.out1

}

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 2.0, 1e-6);
    }
}

// --- Explicit type arg overrides cast on argument ---

#[test]

fn generic_def_explicit_type_arg_overrides_arg_cast_compile_and_run() {
    // foo<f64>(f32(1.0)) — explicit <f64> wins over f32 cast on the argument.

    // T is instantiated as f64; the f32 value is promoted to f64 inside the body.

    // We verify at three levels:

    //   1. Semantic: monomorphized def name contains f64, not f32.

    //   2. MIR: the generated function uses f64 parameters/results, not f32.

    //   3. Runtime: correct output value.

    let src = r#"

outs { out1 }

def double<T>(x: T) {

  return x + x

}

sample {

  out1 = f32(double<f64>(f32(1.5)))

}

"#;

    // Semantic: monomorphized def name contains f64, not f32.

    let parsed = parse_program(src).expect("parse should succeed");

    let typed = analyze(parsed).expect("analysis should succeed");

    let has_f64_def = typed.defs.iter().any(|d| d.name.contains("__g_f64"));

    let has_f32_def = typed.defs.iter().any(|d| d.name.contains("__g_f32"));

    assert!(
        has_f64_def,
        "expected f64 monomorphized def, got: {:?}",
        typed.defs.iter().map(|d| &d.name).collect::<Vec<_>>()
    );

    assert!(
        !has_f32_def,
        "should NOT have f32 monomorphized def when explicit <f64> is used, got: {:?}",
        typed.defs.iter().map(|d| &d.name).collect::<Vec<_>>()
    );

    // MIR: the monomorphized def must retain an f64 parameter and result.

    let mir = lower_test_mir(src);
    assert_mir_scalar_specialization(&mir, "double.__onda_mono__g_f64", onda_mir::ScalarType::F64);

    // Runtime output.

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for s in &output {
        assert_near(*s, 3.0, 1e-6);
    }
}

#[test]

fn generic_def_unresolved_type_arg_error() {
    // Using an undefined generic name as explicit type arg should error.

    let src = r#"

outs { out1 }

def identity<T>(x: T):

  return x

def caller(x: f32):

  return identity<U>(x)

sample { out1 = caller(1.0) }

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "unresolved generic type argument 'U' should be rejected"
    );

    let errors = result.unwrap_err();

    let msg = errors
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        msg.contains("unknown generic type argument") || msg.contains("unresolved"),
        "error should mention unknown/unresolved generic type arg, got: {msg}"
    );
}
