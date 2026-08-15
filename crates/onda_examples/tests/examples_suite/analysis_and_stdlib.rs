use super::*;

// ---- Phase 0: Namespace Const Fixes — Integration tests ----

#[test]

fn nested_namespace_template_compiles_and_runs() {
    let src = r#"

namespace Outer<A = 1>:

  namespace Inner<B = 2>:

    struct S:

      x: f32

      def val(self):

        return f32(A + B)

outs 1

init:

  s = Outer<10>::Inner<20>::S()

sample:

  out1 = s.val()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 30.0, 1e-6);
    }
}

#[test]

fn three_level_nested_namespace_template_compiles_and_runs() {
    let src = r#"

namespace L1<A = 1>:

  namespace L2<B = 2>:

    namespace L3<C = 3>:

      def sum():

        return f32(A + B + C)

outs 1

sample:

  out1 = L1<10>::L2<20>::L3<30>::sum()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 60.0, 1e-6);
    }
}

#[test]

fn generic_struct_inside_namespace_template_t_s_pattern_compiles_and_runs() {
    let src = r#"

namespace Data<S = SR>:

  struct Store<T>:

    buf: T[S]

    def write_first(self, v: T):

      self.buf[0] = v

    def read_first(self):

      return self.buf[0]

outs 1

init:

  s = Data<4>::Store<f32>()

sample:

  s.write_first(0.75)

  out1 = s.read_first()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.75, 1e-6);
    }
}

#[test]

fn generic_struct_method_typed_array_param_compile_and_run() {
    let src = r#"

namespace NS:

  struct Store<T>:

    buf: T[2]

    def load(self, input: T[]):

      self.buf[0] = input[0]

      self.buf[1] = input[1]

    def sum(self):

      return self.buf[0] + self.buf[1]

outs 1

init:

  input: f64[2] = [1.25, 0.75]

  s = NS::Store<f64>()

sample:

  s.load(input)

  out1 = f32(s.sum())

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn generic_proc_t_cast_integer_specialization_compile_and_run() {
    let src = r#"

proc ConstVal<T>:

  outs<T> 1

  init:

    v: T = T(3)

  sample:

    out1 = v

outs 1

init:

  p = ConstVal<i64>()

sample:

  out1 = f32(p())

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn generic_proc_inside_namespace_template_compiles_and_runs() {
    let src = r#"

namespace FX:

  proc Gain<T>:

    ins<T> 1

    outs<T> 1

    params<T>:

      g = 1.0

    sample:

      out1 = in1 * g

outs 1

init:

  g = FX::Gain<f64>(g = f64(0.5))

sample:

  out1 = f32(g(f64(2.0)))

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_inside_namespace_template_preserves_typed_f64_const_defaults() {
    // Verify that f64-typed const defaults propagate through the param pipeline.

    let src = r#"

namespace FX<N = 1>:

  const EXACT: f64 = 1.234567890123

  proc Gain<T>:

    outs<T> 1

    params<T>:

      g = EXACT

    sample:

      out1 = g

outs { out1: f64 }

init:

  g = FX<1>::Gain<f64>()

sample:

  out1 = g()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut out_bytes = vec![0_u8; frames * std::mem::size_of::<f64>()];

    bind_output(&mut instance, 0, out_bytes.as_mut_ptr(), out_bytes.len()).expect("bind output");

    process_checked(&mut instance, frames).expect("process checked");

    let out = decode_planar_f64(&out_bytes);

    for sample in &out {
        let delta = (*sample - 1.234567890123_f64).abs();

        assert!(
            delta <= 1e-12,
            "expected exact f64 value: {sample} ~= 1.234567890123, delta={delta}"
        );
    }
}

#[test]

fn generic_proc_inside_namespace_template_preserves_typed_i32_const_defaults() {
    let src = r#"

namespace FX<N = 1>:

  const EXACT: i32 = 1234567890

  proc Gain<T>:

    outs<T> 1

    params<T>:

      g = EXACT

    sample:

      out1 = g

outs 1

init:

  g = FX<1>::Gain<i32>()

sample:

  if (g() == i32(1234567890)):

    out1 = 1.0

  else:

    out1 = 0.0

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn generic_proc_inside_namespace_template_preserves_typed_i64_const_defaults() {
    let src = r#"

namespace FX<N = 1>:

  const EXACT: i64 = 9007199254740993

  proc Gain<T>:

    outs 1

    params<T>:

      g = EXACT

    sample:

      if (g == i64(9007199254740993)):

        out1 = 1.0

      else:

        out1 = 0.0

outs 1

init:

  g = FX<1>::Gain<i64>()

sample:

  out1 = g()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn namespace_qualified_generic_type_from_enclosing_generic_owner_compiles_and_runs() {
    let src = r#"

namespace NS:

  struct Pair:

    a: f64

    b: f64

proc Container:

  outs 1

  init:

    p = NS::Pair(f64(1.0), f64(2.0))

  sample:

    out1 = f32(p.a + p.b)

outs 1

init:

  c = Container()

sample:

  out1 = c()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn multiple_specializations_of_generic_struct_inside_namespace_template_compiles_and_runs() {
    let src = r#"

namespace Data<S = 4>:

  struct Store<T>:

    buf: T[S]

    def first(self):

      return self.buf[0]

outs 1

init:

  sf = Data<4>::Store<f32>()

  sd = Data<4>::Store<f64>()

sample:

  out1 = sf.first() + f32(sd.first())

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn nested_template_with_alias_compiles_and_runs() {
    let src = r#"

namespace Outer<S = SR>:

  namespace Inner<C = 1>:

    struct Buf:

      data: f32[S * C]

      def capacity(self):

        return i32(S * C)

namespace MyBuf = Outer<100>::Inner<2>

outs 1

init:

  b = MyBuf::Buf()

sample:

  out1 = f32(b.capacity())

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 200.0, 1e-6);
    }
}

#[test]

fn namespace_local_alias_to_generic_struct_runtime_compile_and_run() {
    let src = r#"

namespace A:

  namespace Data<S = 4>:

    struct Store<T>:

      storage: T[S]

  namespace D = Data<4>

  proc Runner:

    outs 1

    init:

      s = D::Store<f32>()

    sample:

      s.storage[0] = 1.25

      out1 = s.storage[0]

outs 1

init:

  r = A::Runner()

sample:

  out1 = r()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn namespace_local_alias_to_generic_struct_proc_method_sugar_compile_and_run() {
    let src = r#"

namespace A:

  namespace Data<S = 4>:

    struct Store<T>:

      storage: T[S]

      def write_first(self, v: T):

        self.storage[0] = v

      def read_first(self):

        return self.storage[0]

  namespace D = Data<4>

  proc Runner:

    outs 1

    init:

      s = D::Store<f32>()

    sample:

      s.write_first(1.25)

      out1 = s.read_first()

outs 1

init:

  r = A::Runner()

sample:

  out1 = r()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.25, 1e-6);
    }
}

#[test]

fn proc_level_consts_using_namespace_consts_compile_and_run() {
    let src = r#"

namespace Synth<N = 2>:

  const Base = N + 1



  proc Voice:

    const Count = Base + 1

    ins Count

    outs Count



    sample:

      out1 = in1

      out2 = in2

      out3 = f32(Count)

      out4 = f32(Base)



outs { out1, out2, out3, out4 }

init:

  v = Synth<2>::Voice()

sample:

  v(1.0, 2.0, 3.0, 4.0)

  out1 = v.out1

  out2 = v.out2

  out3 = v.out3

  out4 = v.out4

"#;

    let frames = 1;

    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    assert_near(output[1], 2.0, 1e-6);

    assert_near(output[2], 4.0, 1e-6);

    assert_near(output[3], 3.0, 1e-6);
}

#[test]

fn nested_namespace_instantiated_generic_struct_with_nested_struct_field_compiles_and_runs() {
    let src = r#"

namespace Outer<S = 3>:

  namespace Inner<C = 2>:

    struct Pair<T>:

      values: T[S * C]

      def write_ends(self, a: T, b: T):

        self.values[0] = a

        self.values[S * C - 1] = b

      def sum_ends(self):

        return self.values[0] + self.values[S * C - 1]



    struct Wrap<T>:

      pair: Pair<T>

      def init_vals(self, a: T, b: T):

        self.pair.write_ends(a, b)

      def sum_vals(self):

        return self.pair.sum_ends()



outs 1

init:

  w = Outer<3>::Inner<2>::Wrap<f32>()

sample:

  w.init_vals(1.0, 2.5)

  out1 = w.sum_vals()

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]

fn namespace_param_return_without_i32_cast_infers_i32() {
    let src = r#"

namespace Outer<S = SR>:

  struct Buf:

    def capacity(self):

      return S

outs 1

init:

  b = Outer<200>::Buf()

sample:

  frames: i32 = b.capacity()

  out1 = f32(frames)

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 200.0, 1e-6);
    }
}

#[test]

fn generic_typed_decl_in_proc_sample_block() {
    let parsed = parse_program(
        r#"

proc Gen<T> {

  outs { out1: T }

  init {

    v: T = 1.0

  }

  sample {

    tmp: T = v + 1.0

    out1 = tmp

  }

}

outs { out1 }

init {

  g = Gen<f64>()

}

sample {

  out1 = f32(g.out1)

}

"#,
    )
    .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "semantic analysis should accept generic typed declarations in proc sample blocks: {:?}",
        result.err()
    );
}

#[test]

fn generic_typed_decl_in_struct_method() {
    let src = r#"

namespace NS:

  struct Pair<T>:

    a: T

    b: T

    def sum(self):

      tmp: T = self.a + self.b

      return tmp

outs 1

init:

  p = NS::Pair<f64>(3.0, 4.0)

sample:

  out1 = f32(p.sum())

"#;

    let frames = 4;

    let (mut instance, _, _) = compile_instance(src, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn generic_typed_decl_in_proc_event() {
    let parsed = parse_program(
        r#"

proc Gen<T> {

  outs { out1: T }

  events {

    reset() {

      tmp: T = 42.0

      v = tmp

    }

  }

  init {

    v: T = 0.0

  }

  sample {

    out1 = v

  }

}

outs { out1 }

init {

  g = Gen<f64>()

}

sample {

  out1 = f32(g.out1)

}

"#,
    )
    .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "semantic analysis should accept generic typed declarations in proc event blocks: {:?}",
        result.err()
    );
}

#[test]

fn bool_type_arg_rejected_for_struct() {
    let parsed = parse_program(
        r#"

outs { out1 }

struct Box<T> { v: T }

init {

  b = Box<bool>(true)

}

sample {

  out1 = 0.0

}

"#,
    )
    .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject 'bool' as a generic type argument for structs"
    );
}

#[test]

fn bool_type_arg_rejected_for_proc() {
    let parsed = parse_program(
        r#"

proc Gen<T> {

  outs { out1: T }

  sample { out1 = 0.0 }

}

outs { out1 }

init {

  g = Gen<bool>()

}

sample {

  out1 = 0.0

}

"#,
    )
    .expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject 'bool' as a generic type argument for processors"
    );
}

// ---------------------------------------------------------------------------

// Phase 3: Generic def parameters — typed array, untyped array, bare buffer

// ---------------------------------------------------------------------------

// Typed array param analysis/parsing coverage.

// Compile-and-run coverage for typed/untyped array params appears below.

const DEF_TYPED_BUFFER_PARAM: &str = r#"

ins { in1 }

outs { out1 }

buffers { buf }

def read_buf(b: buffer<f32>, idx: i32) {

  return b[idx]

}

sample {

  buf[0] = in1

  out1 = read_buf(buf, 0)

}

"#;

#[test]

fn def_typed_buffer_param_baseline() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(DEF_TYPED_BUFFER_PARAM, frames);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32 * 0.25).collect();

    let mut output = vec![0.0_f32; frames];

    let buf_idx = instance.buffer_index("buf").expect("buf");

    let buf_data = vec![0.0_f32; frames];

    bind_buffer(
        &mut instance,
        buf_idx,
        buf_data.as_ptr() as *mut u8,
        frames,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind");

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        assert_near(*sample, input[idx], 1e-6);
    }
}

// Parse-only tests for new syntax

#[test]

fn parse_typed_array_param_syntax() {
    let src = r#"

outs { out1 }

def foo(arr: f32[]) {

  return arr[0]

}

sample { out1 = 0.0 }

"#;

    let parsed = parse_program(src);

    assert!(
        parsed.is_ok(),
        "should parse f32[] param type: {:?}",
        parsed.err()
    );
}

#[test]

fn parse_untyped_array_param_syntax() {
    let src = r#"

outs { out1 }

def foo(arr: []) {

  return arr[0]

}

sample { out1 = 0.0 }

"#;

    let parsed = parse_program(src);

    assert!(
        parsed.is_ok(),
        "should parse [] param type: {:?}",
        parsed.err()
    );
}

#[test]

fn parse_bare_buffer_param_syntax() {
    let src = r#"

outs { out1 }

def foo(b: buffer) {

  return b[0]

}

sample { out1 = 0.0 }

"#;

    let parsed = parse_program(src);

    assert!(
        parsed.is_ok(),
        "should parse bare buffer param type: {:?}",
        parsed.err()
    );
}

#[test]

fn parse_mixed_new_param_types() {
    let src = r#"

outs { out1 }

def process(arr: f32[], b: buffer, x: f32) {

  return x

}

sample { out1 = 0.0 }

"#;

    let parsed = parse_program(src);

    assert!(
        parsed.is_ok(),
        "should parse mixed new param types: {:?}",
        parsed.err()
    );
}

// Test that typed array param is correctly analyzed

#[test]

fn analyze_typed_array_param() {
    let src = r#"

outs { out1 }

def sum_arr(arr: f32[], n: i32) {

  result = 0.0

  for i in 0..n {

    result = result + arr[i]

  }

  return result

}

init {

  data: f32[4] = [1.0, 2.0, 3.0, 4.0]

}

sample {

  out1 = sum_arr(data, 4)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "analysis should succeed: {:?}",
        result.err()
    );
}

// Test that bare buffer param is correctly analyzed

#[test]

fn analyze_bare_buffer_param() {
    let src = r#"

ins { in1 }

outs { out1 }

buffers { buf }

def read_first(b: buffer) {

  return b[0]

}

sample {

  buf[0] = in1

  out1 = read_first(buf)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "analysis should succeed: {:?}",
        result.err()
    );
}

#[test]

fn bitwise_ops_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) = compile_instance(BITWISE_OPS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 24.0, 1e-6);
    }
}

#[test]

fn bitwise_ops_reject_float_operands() {
    let parsed = parse_program(BITWISE_FLOAT_OPERAND_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject float operands for bitwise ops"
    );
}

#[test]

fn assert_compile_time_check_passes() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) = compile_instance(ASSERT_PASSES_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn assert_rejects_false_namespace_compile_time_condition() {
    let parsed =
        parse_program(ASSERT_NAMESPACE_POWER_OF_TWO_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject false compile-time assert conditions"
    );
}

#[test]

fn stdlib_decay_env_start_preserves_level_while_trigger_is_held() {
    let source = r#"
import std/env

events:
  restart():
    decay.start(0.25)

init:
  decay = std::env::DecayEnv(decay_s = 1.0, trigger = 1.0)

sample:
  out1 = decay(trigger = 1.0)
"#;
    let frames = 1;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);
    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = [0.0_f32; 1];
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("initial processing should succeed");

    let restart = instance
        .event_index("restart")
        .expect("restart event should exist");
    trigger_event_by_index(&mut instance, restart, &[]).expect("restart should succeed");
    process_interleaved(&mut instance, &[], &mut output, frames)
        .expect("processing after restart should succeed");

    let coefficient = (-1.0_f32 / 48_000.0).exp();
    assert_near(output[0], 0.25 * coefficient, 1e-6);
}

#[test]

fn stdlib_fft_impulse_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_IMPULSE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-4);
    }
}

#[test]

fn stdlib_complex_struct_compile_and_run() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_COMPLEX_STRUCT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 11.0, 1e-6);

    assert_near(output[1], 2.0, 1e-6);

    assert_near(output[2], 125.0_f32.sqrt(), 1e-5);

    assert_near(output[3], 2.0_f32.atan2(11.0), 1e-5);
}

#[test]

fn stdlib_complex_polar_f64_compile_and_run() {
    let frames = 1;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_COMPLEX_POLAR_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 0.5_f32.cos(), 1e-5);

    assert_near(output[1], -0.5_f32.sin(), 1e-5);

    assert_near(output[2], 1.0, 1e-6);
}

#[test]

fn stdlib_fft_f64_impulse_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_IMPULSE_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-4);
    }
}

#[test]

fn stdlib_fft_real_packed_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_REAL_PACKED_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-4);
    }
}

#[test]

fn stdlib_fft_real_packed_roundtrip_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_REAL_PACKED_ROUNDTRIP_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 1.0, 1e-4);

        assert_near(output[base + 1], 2.0, 1e-4);

        assert_near(output[base + 2], 3.0, 1e-4);

        assert_near(output[base + 3], 4.0, 1e-4);
    }
}

#[test]
fn stdlib_fft_external_namespace_generic_field_method_compile_and_run() {
    let src = r#"
import std/fft

namespace Wrap<N = 8>:
  struct Real<T>:
    fft: std::fft<N>::FFT<T>

    def forward(self, input: T[], packed: T[]):
      self.fft.forward_real_packed(input, packed)

outs 1

init:
  w: Wrap<8>::Real<f32>
  input: f32[8]
  packed: f32[8]
  input[0] = 1.0
  input[1] = 0.0
  input[2] = 0.0
  input[3] = 0.0
  input[4] = 0.0
  input[5] = 0.0
  input[6] = 0.0
  input[7] = 0.0

sample:
  w.forward(input, packed)
  out1 = packed[0] + packed[1] + packed[2] + packed[3] + packed[4] + packed[5] + packed[6] + packed[7]
"#;

    let frames = 2;
    let (mut instance, in_channels, out_channels) = compile_instance(src, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn stdlib_fft_real_spectrum_helpers_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_FFT_REAL_SPECTRUM_HELPERS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 5.0, 1e-4);

        assert_near(output[base + 1], 5.0, 1e-4);

        assert_near(output[base + 2], -0.5 * std::f32::consts::PI, 1e-4);

        assert_near(output[base + 3], 13.0, 1e-6);
    }
}

#[test]

fn stdlib_stft_hann_window_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_STFT_HANN_WINDOW_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 0.941_275_5, 1e-4);

        assert_near(output[base + 1], 0.376_510_2, 1e-4);

        assert_near(output[base + 2], 0.188_255_1, 1e-4);

        assert_near(output[base + 3], 13.0, 1e-6);
    }
}

#[test]

fn stdlib_stft_default_hann_window_compile_and_run() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_STFT_DEFAULT_HANN_WINDOW_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 1.5, 1e-6);

        assert_near(output[base + 1], 2.25, 1e-6);
    }
}

#[test]

fn stdlib_realfft_struct_compile_and_run() {
    let frames = 128;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALFFT_STRUCT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|v| v.abs() > 1e-4),
        "real fft proc should produce non-zero output after its frame latency"
    );
}

#[test]

fn stdlib_realfft_matches_full_complex_reference() {
    let frames = 64;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALFFT_REFERENCE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[frames - 1], 0.0, 2e-4);
}

#[test]

fn stdlib_realfft_namespaced_proc_compile_and_run() {
    let frames = 128;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALFFT_NAMESPACED_PROC_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output.iter().any(|v| v.abs() > 1e-4),
        "namespaced real fft proc should produce non-zero output after its frame latency"
    );
}

#[test]

fn stdlib_realfft_hann_ola_passthrough_compile_and_run() {
    let frames = 2048;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALFFT_HANN_OLA_PASSTHROUGH_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let mut peak = 0.0_f32;

    for frame in 256..frames {
        let base = frame * out_channels;

        peak = peak.max(output[base].abs());

        assert_near(output[base + 1], 32.0, 1e-6);

        assert_near(output[base + 2], 32.0, 1e-6);
    }

    assert!(

        peak < 5e-4,

        "hann overlap-add passthrough should reconstruct the delayed signal closely, peak error was {peak}"

    );
}

#[test]
fn stdlib_realifft_hann_waits_for_stable_overlap_normalization() {
    let frames = 64;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALIFFT_HANN_PRIMING_EXAMPLE, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(output[..32].iter().all(|sample| *sample == 0.0));
    assert!(output[32..].iter().any(|sample| sample.abs() > 1e-4));
    assert!(output.iter().all(|sample| sample.is_finite()));
    let peak = output
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    assert!(peak <= 2.0, "primed overlap-add peak was {peak}");
}

#[test]
fn stdlib_realifft_hann_reprimes_after_becoming_inactive() {
    let frames = 160;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALIFFT_HANN_PRIMING_EXAMPLE, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert!(
        output[128..].iter().all(|sample| *sample == 0.0),
        "an isolated Hann frame after inactivity must wait for a new overlapping frame"
    );
}

#[test]
fn stdlib_realifft_first_frame_uses_prepared_twiddles() {
    let frames = 64;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_REALIFFT_FIRST_FRAME_EXAMPLE, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let peak_error = output
        .iter()
        .copied()
        .fold(0.0_f32, |peak, error| peak.max(error));
    assert!(
        peak_error < 2e-5,
        "first-frame inverse error was {peak_error}"
    );
}

#[test]

fn stdlib_convolution_time_domain_event_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_TIME_DOMAIN_EVENT_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let idx = instance
        .event_index("set_ir")
        .expect("set_ir event must exist");

    let mut payload = Vec::new();

    payload.extend_from_slice(&1.0_f32.to_ne_bytes());

    payload.extend_from_slice(&0.5_f32.to_ne_bytes());

    payload.extend_from_slice(&0.25_f32.to_ne_bytes());

    payload.extend_from_slice(&0.0_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, idx, &payload).expect("event trigger should succeed");

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    assert_near(output[1], 0.5, 1e-6);

    assert_near(output[2], 0.25, 1e-6);

    assert_near(output[3], 0.0, 1e-6);

    for sample in output.iter().skip(4) {
        assert_near(*sample, 0.0, 1e-6);
    }
}

#[test]

fn stdlib_convolution_time_domain_mirrored_history_wraps_at_capacity() {
    let frames = 12;
    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_TIME_DOMAIN_WRAP_EXAMPLE, frames);

    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let input = [
        1.0_f32, -0.5, 0.25, 2.0, -1.0, 0.75, 0.125, -0.25, 1.5, -2.0, 0.5, 1.0,
    ];
    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for frame in 0..frames {
        let previous = frame.checked_sub(1).map_or(0.0, |previous| input[previous]);
        let expected = input[frame] * 0.75 - previous * 0.25;
        assert_near(output[frame], expected, 1e-6);
    }
}

#[test]

fn stdlib_convolution_block_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_BLOCK_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 0.0, 1e-5);

    assert_near(output[1], 0.0, 1e-5);

    assert_near(output[2], 0.0, 1e-5);

    assert_near(output[3], 0.0, 1e-5);

    assert_near(output[4], 1.0, 1e-4);

    assert_near(output[5], 0.5, 1e-4);

    assert_near(output[6], 0.25, 1e-4);

    assert_near(output[7], 0.0, 1e-4);
}

#[test]

fn stdlib_convolution_block_spreads_multi_partition_work_without_changing_output() {
    let frames = 40;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_BLOCK_SPREAD_EXAMPLE, frames);

    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let mut input = vec![0.0_f32; frames];
    input[..8].copy_from_slice(&[0.75, -0.25, 0.5, 0.125, -0.4, 0.2, 0.1, -0.05]);

    let impulse = [
        1.0_f32, -0.5, 0.25, 0.125, -0.75, 0.3, -0.2, 0.1, 0.05, -0.04, 0.03, -0.02, 0.01,
    ];
    let mut expected = vec![0.0_f32; frames];
    let latency = 4;
    for (output_frame, expected_sample) in expected.iter_mut().enumerate().skip(latency) {
        let convolution_frame = output_frame - latency;
        for (tap, coefficient) in impulse.iter().copied().enumerate() {
            if tap <= convolution_frame {
                *expected_sample += input[convolution_frame - tap] * coefficient;
            }
        }
    }

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (frame, (actual, expected)) in output
        .iter()
        .copied()
        .zip(expected.iter().copied())
        .enumerate()
    {
        assert_near(actual, expected, 2e-4);
        assert!(actual.is_finite(), "frame {frame} must be finite");
    }
}

#[test]

fn stdlib_convolution_zero_latency_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_ZERO_LATENCY_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-4);

    assert_near(output[1], 0.5, 1e-4);

    assert_near(output[2], 0.25, 1e-4);

    assert_near(output[3], 0.0, 1e-4);

    assert_near(output[4], 0.125, 1e-4);

    for sample in output.iter().skip(5) {
        assert_near(*sample, 0.0, 1e-4);
    }
}

#[test]

fn stdlib_convolution_zero_latency_aligns_every_non_uniform_stage() {
    let frames = 8_304;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_ZERO_LATENCY_MULTISTAGE_EXAMPLE, frames);

    assert_eq!(in_channels, 1);
    assert_eq!(out_channels, 1);

    let mut input = vec![0.0_f32; frames];
    input[0] = 1.0;

    let mut expected = vec![0.0_f32; frames];
    for (frame, value) in [
        (0, 0.75),
        (127, -0.5),
        (128, 0.375),
        (511, -0.25),
        (512, 0.2),
        (2_047, -0.15),
        (2_048, 0.125),
        (8_191, -0.1),
        (8_192, 0.075),
        (8_199, -0.05),
    ] {
        expected[frame] = value;
    }

    let assert_matches = |output: &[f32]| {
        for (frame, (actual, expected)) in output
            .iter()
            .copied()
            .zip(expected.iter().copied())
            .enumerate()
        {
            let error = (actual - expected).abs();
            assert!(
                error < 2e-3,
                "frame {frame}: expected {expected}, got {actual}, error {error}"
            );
        }
    };

    let mut output = vec![0.0_f32; frames];
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");
    assert_matches(&output);

    let reset = instance
        .event_index("reset_conv")
        .expect("reset_conv event must exist");
    trigger_event_by_index(&mut instance, reset, &[]).expect("reset event should succeed");

    output.fill(0.0);
    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("processing after reset should succeed");
    assert_matches(&output);
}

#[test]

fn stdlib_convolution_zero_latency_with_const_namespace_args_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(
        STDLIB_CONVOLUTION_ZERO_LATENCY_CONST_NAMESPACE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-4);

    assert_near(output[1], 0.5, 1e-4);

    assert_near(output[2], 0.25, 1e-4);

    assert_near(output[3], 0.0, 1e-4);

    assert_near(output[4], 0.125, 1e-4);

    for sample in output.iter().skip(5) {
        assert_near(*sample, 0.0, 1e-4);
    }
}

#[test]

fn stdlib_convolution_zero_latency_large_const_namespace_args_analyze() {
    let parsed = parse_program(STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_ANALYZE_EXAMPLE)
        .expect("parse should succeed");

    let _typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 44_100.0,

            block_size: 1024,
        },
    )
    .expect("semantic analysis should succeed");
}

#[test]

fn stdlib_convolution_zero_latency_large_const_wrapper_namespace_analyze() {
    let parsed = parse_program(STDLIB_CONVOLUTION_ZERO_LATENCY_LARGE_CONST_WRAPPER_ANALYZE_EXAMPLE)
        .expect("parse should succeed");

    let _typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 44_100.0,

            block_size: 1024,
        },
    )
    .expect("semantic analysis should succeed");
}

#[test]

fn stdlib_convolution_generic_f64_compile_and_run() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STDLIB_CONVOLUTION_F64_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    assert_near(output[0], 1.0, 1e-4);

    assert_near(output[1], 0.5, 1e-4);

    assert_near(output[2], 0.25, 1e-4);

    assert_near(output[3], 0.0, 1e-4);
}

#[test]

fn nested_struct_field_and_method_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(NESTED_STRUCT_FIELD_AND_METHOD_EXAMPLE, frames);

    let mut outputs = vec![0.0_f32; frames * 2];

    process_interleaved(&mut instance, &[], &mut outputs, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * 2;

        assert_near(outputs[base], 1.5, 1e-6);

        assert_near(outputs[base + 1], 4.0, 1e-6);
    }
}

#[test]

fn multiline_struct_method_call_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(MULTILINE_STRUCT_METHOD_CALL_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.75, 1e-6);
    }
}

#[test]

fn nested_generic_struct_array_field_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(NESTED_GENERIC_STRUCT_ARRAY_FIELD_EXAMPLE, frames);

    let mut outputs = vec![0.0_f32; frames * 2];

    process_interleaved(&mut instance, &[], &mut outputs, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * 2;

        assert_near(outputs[base], 1.0, 1e-6);

        assert_near(outputs[base + 1], 3.0, 1e-6);
    }
}

#[test]

fn stdlib_fft_rejects_zero_size() {
    let parsed = parse_program(STDLIB_FFT_ZERO_SIZE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject zero-sized std::fft instantiations"
    );
}

#[test]

fn analyze_mutable_typed_array_param() {
    let src = r#"

outs { out1 }

init {

  data: f32[4] = [1.0, 2.0, 3.0, 4.0]

}

def write_first(arr: f32[]) {

  arr[0] = 9.0

  return arr[0]

}

sample {

  out1 = write_first(data)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "analysis should allow mutable array params in def: {:?}",
        result.err()
    );
}

#[test]

fn analyze_mutable_buffer_param() {
    let src = r#"

ins { in1 }

outs { out1 }

buffers { buf }

def write_first(b: buffer<f32>, x: f32) {

  b[0] = x

  return b[0]

}

sample {

  out1 = write_first(buf, in1)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "analysis should allow mutable buffer params in def: {:?}",
        result.err()
    );
}

// Test generic struct def param (monomorphization)

#[test]

fn analyze_generic_struct_def_param() {
    let src = r#"

outs { out1 }

struct Box<T> {

  value: T = 0.0

}

def unbox(b: Box) {

  return b.value

}

init {

  mybox = Box<f32>(value = 42.0)

}

sample {

  out1 = unbox(mybox)

}

"#;

    let parsed = parse_program(src).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "analysis should succeed for generic struct def param: {:?}",
        result.err()
    );
}

// Test generic struct def param compiles and runs

const DEF_GENERIC_STRUCT_PARAM: &str = r#"

outs { out1 }

struct Box<T> {

  value: T = 0.0

}

def unbox(b: Box) {

  return b.value

}

init {

  mybox = Box<f32>(value = 42.0)

}

sample {

  out1 = unbox(mybox)

}

"#;

#[test]

fn def_generic_struct_param_compiles_and_runs() {
    let frames = 2;

    let (mut instance, _, _) = compile_instance(DEF_GENERIC_STRUCT_PARAM, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 42.0, 1e-6);
    }
}

// ── Array param ABI fix tests (ptr+len) ──────────────────────────────

const ARRAY_PARAM_SUM_EXAMPLE: &str = r#"

outs { out1 }

init {

    data: f32[4]

    data[0] = 1.0

    data[1] = 2.0

    data[2] = 3.0

    data[3] = 4.0

}

def sum_array(arr: f32[]) {

    total = 0.0

    n = arr.len()

    for i in 0..n {

        total = total + arr[i]

    }

    return total

}

sample {

    out1 = sum_array(data)

}

"#;

#[test]

fn array_param_sum_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_SUM_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

const ARRAY_PARAM_SINGLE_DEF_EXAMPLE: &str = r#"

outs { out1 }

init {

    data: f32[2]

    data[0] = 10.0

    data[1] = 20.0

}

def first(arr: f32[]) { return arr[0] }

sample {

    out1 = first(data)

}

"#;

#[test]

fn array_param_single_def_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_SINGLE_DEF_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

const ARRAY_PARAM_DEF_TO_DEF_EXAMPLE: &str = r#"

outs { out1 }

init {

    data: f32[2]

    data[0] = 10.0

    data[1] = 20.0

}

def first(arr: f32[]) { return arr[0] }

def wrap_first(arr: f32[]) { return first(arr) }

sample {

    out1 = wrap_first(data)

}

"#;

#[test]

fn array_param_def_to_def_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_DEF_TO_DEF_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 10.0, 1e-6);
    }
}

const ARRAY_PARAM_LEN_FORWARDED_EXAMPLE: &str = r#"

outs { out1 }

init {

    data: f32[3]

    data[0] = 5.0

    data[1] = 10.0

    data[2] = 15.0

}

def get_len(arr: f32[]) { return arr.len() }

sample {

    out1 = get_len(data)

}

"#;

#[test]

fn array_param_len_forwarded_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_LEN_FORWARDED_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const ARRAY_PARAM_MUTATION_EXAMPLE: &str = r#"

outs { out1 }

init {

    data: f32[4]

    data[0] = 1.0

    data[1] = 2.0

    data[2] = 3.0

    data[3] = 4.0

}

def write_and_sum(arr: f32[]) {

    arr[0] = arr[1] + arr[2]

    return arr[0] + arr[3]

}

sample {

    out1 = write_and_sum(data)

}

"#;

#[test]

fn array_param_mutation_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(ARRAY_PARAM_MUTATION_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 9.0, 1e-6);
    }
}

const BUFFER_PARAM_MUTATION_EXAMPLE: &str = r#"

ins { in1 }

outs { out1 }

buffers { buf: buffer<f32> }

def write_first(b: buffer<f32>, x: f32) {

    b[0] = x

    return b[0]

}

sample {

    out1 = write_first(buf, in1)

}

"#;

#[test]

fn buffer_param_mutation_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(BUFFER_PARAM_MUTATION_EXAMPLE, frames);

    let input: Vec<f32> = (0..frames).map(|n| (n + 1) as f32 * 0.25).collect();

    let mut output = vec![0.0_f32; frames];

    let buf_idx = instance.buffer_index("buf").expect("buf");

    let mut buf_data = vec![0.0_f32; frames];

    bind_buffer(
        &mut instance,
        buf_idx,
        buf_data.as_mut_ptr() as *mut u8,
        frames,
        1,
        48_000.0,
        PrimitiveType::F32,
    )
    .expect("bind");

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        assert_near(*sample, input[idx], 1e-6);
    }
}

const UNTYPED_ARRAY_PARAM_FROM_INIT_ARRAY_EXAMPLE: &str = r#"

outs { out1 }

init {

    data = [0.5, 1.0, 2.5]

}

def sum_first_last(arr: []) {

    return arr[0] + arr[2]

}

sample {

    out1 = sum_first_last(data)

}

"#;

#[test]

fn untyped_array_param_from_init_array_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(UNTYPED_ARRAY_PARAM_FROM_INIT_ARRAY_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_F32_TYPE_ARG_EXAMPLE: &str = r#"

outs { out1 }

struct Stereo<T> {

  v: T[2]

}

init {

  data: Stereo<f32>[1000]

}

sample {

  s = data[10]

  s.v[0] = 0.5

  s.v[1] = 1.5

  out1 = s.v[0] + s.v[1]

}

"#;

#[test]

fn generic_struct_array_explicit_f32_type_arg_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_F32_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_IMPLICIT_DEFAULT_F32_EXAMPLE: &str = r#"

outs { out1 }

struct Stereo<T> {

  v: T[2]

}

init {

  data: Stereo[1000]

}

sample {

  s = data[10]

  s.v[0] = 1.0

  s.v[1] = 2.0

  out1 = s.v[0] + s.v[1]

}

"#;

const GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE: &str = r#"

outs { out1 }

struct Complex<T> {

  re: T

  im: T

  def set(self, re, im) {

    self.re = re

    self.im = im

  }

  def mul_parts(self, re, im) {

    old_re = self.re

    old_im = self.im

    self.re = old_re * re - old_im * im

    self.im = old_re * im + old_im * re

  }

  def sum(self) {

    return self.re + self.im

  }

}

init {

  bins: Complex<f32>[4]

}

sample {

  bins[1].set(1.0, 2.0)

  bins[1].mul_parts(3.0, -4.0)

  out1 = bins[1].sum()

}

"#;

const GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_F64_EXAMPLE: &str = r#"

outs { out1 }

struct Complex<T> {

  re: T

  im: T

  def set_polar(self, magnitude, phase) {

    self.re = magnitude * cos(phase)

    self.im = magnitude * sin(phase)

  }

  def conjugate(self) {

    self.im = -self.im

  }

  def scale_assign(self, gain) {

    self.re = self.re * gain

    self.im = self.im * gain

  }

  def sum(self) {

    return self.re + self.im

  }

}

init {

  bins: Complex<f64>[4]

}

sample {

  bins[0].set_polar(f64(2.0), f64(0.5))

  bins[0].conjugate()

  bins[0].scale_assign(f64(0.5))

  out1 = f32(bins[0].sum())

}

"#;

const STDLIB_COMPLEX_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE: &str = r#"

import std/complex

outs { out1 }

init {

  bins: std::complex::Complex<f32>[4]

}

sample {

  bins[1].set(1.0, 2.0)

  bins[1].mul_parts(3.0, -4.0)

  out1 = bins[1].real() + bins[1].imag()

}

"#;

#[test]

fn generic_struct_array_implicit_default_f32_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_IMPLICIT_DEFAULT_F32_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn generic_struct_array_indexed_method_calls_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

#[test]

fn generic_struct_array_indexed_method_calls_f64_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(
        GENERIC_STRUCT_ARRAY_INDEXED_METHOD_CALLS_F64_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = 0.5_f32.cos() - 0.5_f32.sin();

    for sample in &output {
        assert_near(*sample, expected, 1e-5);
    }
}

#[test]

fn stdlib_complex_array_indexed_method_calls_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(STDLIB_COMPLEX_ARRAY_INDEXED_METHOD_CALLS_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 13.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_F64_TYPE_ARG_EXAMPLE: &str = r#"

outs { out1 }

struct Stereo<T> {

  v: T[2]

}

init {

  data: Stereo<f64>[1000]

}

sample {

  s = data[10]

  s.v[0] = f64(0.5)

  s.v[1] = f64(1.5)

  out1 = f32(s.v[0] + s.v[1])

}

"#;

#[test]

fn generic_struct_array_explicit_f64_type_arg_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_F64_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_I32_TYPE_ARG_EXAMPLE: &str = r#"

outs { out1 }

struct Stereo<T> {

  v: T[2]

}

init {

  data: Stereo<i32>[1000]

}

sample {

  s = data[10]

  s.v[0] = 1

  s.v[1] = 2

  out1 = f32(s.v[0] + s.v[1])

}

"#;

#[test]

fn generic_struct_array_explicit_i32_type_arg_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_I32_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_I64_TYPE_ARG_EXAMPLE: &str = r#"

outs { out1 }

struct Stereo<T> {

  v: T[2]

}

init {

  data: Stereo<i64>[1000]

}

sample {

  s = data[10]

  s.v[0] = i64(1)

  s.v[1] = i64(3)

  out1 = f32(s.v[0] + s.v[1])

}

"#;

#[test]

fn generic_struct_array_explicit_i64_type_arg_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) =
        compile_instance(GENERIC_STRUCT_ARRAY_EXPLICIT_I64_TYPE_ARG_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_EXPLICIT_BOOL_TYPE_ARG_ERROR_EXAMPLE: &str = r#"

outs { out1 }

struct Stereo<T> {

  v: T[2]

}

init {

  data: Stereo<bool>[1000]

}

sample {

  out1 = 0.0

}

"#;

#[test]

fn generic_struct_array_explicit_bool_type_arg_is_rejected() {
    let parsed =
        parse_program(GENERIC_STRUCT_ARRAY_EXPLICIT_BOOL_TYPE_ARG_ERROR_EXAMPLE).expect("parse");

    let errs = analyze(parsed).expect_err("bool generic arg should be rejected");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("bool") && d.message.contains("generic type")),
        "expected bool generic type arg rejection, got {:?}",
        errs
    );
}

const GENERIC_STRUCT_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE: &str = r#"

outs { out1 }

struct Pair<T, U> {

  a: T

  b: U

}

init {

  p = Pair<f32, i64>(1.5, i64(2))

}

sample {

  out1 = p.a + f32(p.b)

}

"#;

#[test]

fn generic_struct_two_type_params_explicit_type_args_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(
        GENERIC_STRUCT_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

const GENERIC_STRUCT_ARRAY_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE: &str = r#"

outs { out1 }

struct Pair<T, U> {

  a: T

  b: U

}

init {

  data: Pair<f32, i64>[1000]

}

sample {

  s = data[10]

  s.a = 0.5

  s.b = i64(3)

  out1 = s.a + f32(s.b)

}

"#;

#[test]

fn generic_struct_array_two_type_params_explicit_type_args_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(
        GENERIC_STRUCT_ARRAY_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

const GENERIC_PROC_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE: &str = r#"

proc Duo<T, U> {

  ins {

    in1: T

    in2: U

  }

  outs {

    out1: T

    out2: U

  }

  sample {

    out1 = in1

    out2 = in2

  }

}

outs { out1 }

init {

  p = Duo<f32, i64>()

}

sample {

  p(1.25, i64(2))

  out1 = p.out1 + f32(p.out2)

}

"#;

#[test]

fn generic_proc_two_type_params_explicit_type_args_compile_and_run() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(
        GENERIC_PROC_TWO_TYPE_PARAMS_EXPLICIT_TYPE_ARGS_EXAMPLE,
        frames,
    );

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.25, 1e-6);
    }
}

const TOP_LEVEL_CONST_EVENT_SIZE_EXAMPLE: &str = r#"

const N = 3



proc Voice {

  params { sum = 0.0 }

  outs { out1 }

  events {

    load(values: f32[N]) {

      sum = values[0] + values[1] + values[2]

    }

  }

  sample {

    out1 = sum

  }

}



outs { out1 }

events {

  load(values: f32[N]) {

    voice.load(values)

  }

}

init {

  voice = Voice()

}

sample {

  out1 = voice()

}

"#;

#[test]

fn top_level_consts_can_drive_event_sizes_and_proc_apis() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_CONST_EVENT_SIZE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let event_idx = instance.event_index("load").expect("load event must exist");

    assert_eq!(instance.event_payload_bytes(event_idx), Some(12));

    let mut payload = Vec::new();

    payload.extend_from_slice(&1.0_f32.to_ne_bytes());

    payload.extend_from_slice(&2.0_f32.to_ne_bytes());

    payload.extend_from_slice(&3.0_f32.to_ne_bytes());

    trigger_event_by_index(&mut instance, event_idx, &payload).expect("event trigger should work");

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 6.0, 1e-6);
    }
}

const CONST_SCOPE_COMPILE_AND_RUN_EXAMPLE: &str = r#"

const N = 3



outs { out1 }



def bonus() {

  const X = 0.5

  return X

}



init {

  const BASE: i32 = 1

  vals: f32[N] = [0.0, 0.0, 0.0]

  vals[BASE + 1] = 1.25

  seed = vals[2]

}



sample {

  const SCALE: f32 = 2.0

  out1 = seed + bonus() + SCALE

}

"#;

#[test]

fn consts_work_in_def_init_and_sample_scopes() {
    let frames = 4;

    let (mut instance, _, _) = compile_instance(CONST_SCOPE_COMPILE_AND_RUN_EXAMPLE, frames);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.75, 1e-6);
    }
}

const CONST_ASSIGN_ERROR_EXAMPLE: &str = r#"

outs { out1 }

sample {

  const X = 1

  X = 2

  out1 = 0.0

}

"#;

const CONST_RESERVED_NAME_ERROR_EXAMPLE: &str = r#"

const SR = 1

outs { out1 }

sample {

  out1 = 0.0

}

"#;

const CONST_RUNTIME_INIT_ERROR_EXAMPLE: &str = r#"

outs { out1 }

sample {

  x = 1.0

  const BAD = x

  out1 = 0.0

}

"#;

const NAMESPACE_CONST_ACCESS_EXAMPLE: &str = r#"

import std/convolution



outs {

  out1

  out2

}



sample {

  out1 = f32(std::convolution<8, 8>::HopSize)

  out2 = f32(std::convolution::HopSize)

}

"#;

#[test]

fn consts_reject_assignment_reserved_names_and_runtime_initializers() {
    let program =
        parse_program(CONST_ASSIGN_ERROR_EXAMPLE).expect("local const assignment should parse");
    let errs = analyze(program).expect_err("assigning to a const should be rejected");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("cannot assign to constant 'X'")),
        "expected const assignment error, got {:?}",
        errs
    );

    let program =
        parse_program(CONST_RESERVED_NAME_ERROR_EXAMPLE).expect("reserved const name should parse");
    let errs = analyze(program).expect_err("builtin const names should be reserved");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("constant name 'SR' is reserved")),
        "expected reserved const name error, got {:?}",
        errs
    );

    let program = parse_program(CONST_RUNTIME_INIT_ERROR_EXAMPLE)
        .expect("runtime const initializer should parse");
    let errs = analyze(program).expect_err("runtime initializer should be rejected");

    assert!(
        errs.iter().any(|d| {
            d.message.contains("const 'BAD'") && d.message.contains("non-constant symbol 'x'")
        }),
        "expected compile-time const initializer error, got {:?}",
        errs
    );
}

#[test]

fn namespace_consts_are_accessible_via_qualified_paths() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(NAMESPACE_CONST_ACCESS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * 2];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        assert_near(output[frame * 2], 4.0, 1e-6);

        assert_near(output[frame * 2 + 1], 128.0, 1e-6);
    }
}

#[test]
fn stdlib_schroeder_is_a_configurable_namespace() {
    let source = r#"
import std/reverb

outs 2

init:
  reverb = std::reverb::Schroeder<2048, 1024>::Reverb()

sample:
  reverb(0.0, 0.0)
  out1 = f32(std::reverb::Schroeder<2048, 1024>::CombLines) + reverb.out1
  out2 = f32(std::reverb::Schroeder<2048, 1024>::AllpassLines) + reverb.out2
"#;
    let frames = 4;
    let (mut instance, in_channels, out_channels) = compile_instance(source, frames);

    assert_eq!(in_channels, 0);
    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * 2];
    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        assert_near(output[frame * 2], 8.0, 1e-6);
        assert_near(output[frame * 2 + 1], 4.0, 1e-6);
    }
}
