    #[test]
    fn positional_and_named_binding_counts_use_zero_based_domains() {
        let source = r#"
init:
  clamped = 0 {1000}
  wrapped = 0 {count = 1000, mode = wrap}

sample:
  clamped += 1
  wrapped += 1
  out1 = f32(clamped + wrapped)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("single-bound ranges should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("single-bound ranges should lower to optimized MIR");
        for (name, expected_mode) in [
            ("clamped", onda_mir::IntegerRangeMode::Clamp),
            ("wrapped", onda_mir::IntegerRangeMode::Wrap),
        ] {
            let range = mir
                .state
                .iter()
                .find(|state| state.name == name)
                .and_then(|state| state.integer_range)
                .unwrap_or_else(|| panic!("missing integer range for {name}"));
            assert_eq!(range.min, onda_mir::ScalarValue::I32(0));
            assert_eq!(range.max, onda_mir::ScalarValue::I32(999));
            assert_eq!(range.mode, expected_mode);
        }
    }

    #[test]
    fn exclusive_and_inclusive_binding_ranges_preserve_their_endpoints() {
        let source = r#"
init:
  exclusive = 10 {10..20}
  inclusive = 10 {range = 10..=20, mode = wrap}

sample:
  out1 = f32(exclusive + inclusive)
"#;
        let typed = analyze(parse_program(source).expect("binding ranges should parse"))
            .expect("binding ranges should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("binding ranges should lower to optimized MIR");
        for (name, expected_max, expected_mode) in [
            ("exclusive", 19, onda_mir::IntegerRangeMode::Clamp),
            ("inclusive", 20, onda_mir::IntegerRangeMode::Wrap),
        ] {
            let range = mir
                .state
                .iter()
                .find(|state| state.name == name)
                .and_then(|state| state.integer_range)
                .unwrap_or_else(|| panic!("missing integer range for '{name}'"));
            assert_eq!(range.min, onda_mir::ScalarValue::I32(10));
            assert_eq!(range.max, onda_mir::ScalarValue::I32(expected_max));
            assert_eq!(range.mode, expected_mode);
        }
    }

    #[test]
    fn binding_ranges_reject_empty_domains_and_allow_one_past_i32_max_as_the_end() {
        assert_analyze_error_contains(
            "init:\n  value: i32 = 0 {0}\nsample:\n  out1 = f32(value)\n",
            "integer binding count must be positive",
        );
        for domain in ["{range = 5..5}", "{range = 6..5}"] {
            assert_analyze_error_contains(
                &format!("init:\n  value: i32 = 0 {domain}\nsample:\n  out1 = f32(value)\n"),
                "begin bound must be less than its exclusive end bound",
            );
        }
        assert_analyze_error_contains(
            r#"
init:
  value: i64 = 0 {
    range = (-9223372036854775807 - 1)..(-9223372036854775807 - 1)
  }

sample:
  out1 = f32(value)
"#,
            "begin bound must be less than its exclusive end bound",
        );

        let source = r#"
init:
  value: i32 = 2147483647 {range = 2147483647..2147483648}

sample:
  out1 = f32(value)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("an exclusive i32 end may be one past the largest stored value");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("the exclusive end should lower to a representable inclusive invariant");
        let range = mir
            .state
            .iter()
            .find(|state| state.name == "value")
            .and_then(|state| state.integer_range)
            .expect("value should retain its integer range");
        assert_eq!(range.min, onda_mir::ScalarValue::I32(i32::MAX));
        assert_eq!(range.max, onda_mir::ScalarValue::I32(i32::MAX));

        let source = r#"
init:
  value: i64 = -9223372036854775807 - 1 {
    range = (-9223372036854775807 - 1)..(-9223372036854775807)
  }

sample:
  out1 = f32(value)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("an i64 binding range may begin at i64::MIN");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("the minimum i64 range should lower without endpoint underflow");
        let range = mir
            .state
            .iter()
            .find(|state| state.name == "value")
            .and_then(|state| state.integer_range)
            .expect("value should retain its integer range");
        assert_eq!(range.min, onda_mir::ScalarValue::I64(i64::MIN));
        assert_eq!(range.max, onda_mir::ScalarValue::I64(i64::MIN));
    }

    #[test]
    fn ranged_state_does_not_capture_a_shadowing_function_parameter() {
        let source = r#"
init:
  index: i32 = 0 {0..4, wrap}

def overwrite(index: i32):
  index = 100
  return index

sample:
  out1 = f32(overwrite(5))
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("a shadowing function parameter should analyze");
        let overwrite = typed
            .defs
            .iter()
            .find(|function| function.name == "overwrite")
            .expect("overwrite function");
        let Stmt::Assign { expr, .. } = &overwrite.body[0] else {
            panic!("the first function statement should be an assignment");
        };
        assert!(
            matches!(expr, Expr::Int { value: 100, .. }),
            "the top-level range must not normalize a shadowing parameter: {expr:?}"
        );
    }

    #[test]
    fn processor_ranged_state_normalizes_generated_method_writes() {
        let source = r#"
proc Counter:
  init:
    position: i32 = 0 {0..4, wrap}

  sample:
    position += 1
    out1 = f32(position)

init:
  counter = Counter()

sample:
  out1 = counter()
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("processor range should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("processor range should lower to optimized MIR");
        let dump = onda_mir::format_program(mir.as_program());
        assert!(dump.contains("intrinsic range_wrap"));
        assert!(
            mir.state
                .iter()
                .find(|state| state.name == "counter.position")
                .and_then(|state| state.integer_range)
                .is_some(),
            "{dump}"
        );
    }

    #[test]
    fn nested_generic_processor_ranged_state_survives_flattening() {
        let source = r#"
proc Counter<T>:
  init:
    position: i32 = 0 {0..4, wrap}
    marker: T = T(0)

  sample:
    position -= 1
    out1 = f32(position)

proc Wrapper<T>:
  init:
    counter = Counter<T>()

  sample:
    out1 = counter()

init:
  wrapper = Wrapper<f32>()

sample:
  out1 = wrapper()
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("nested generic processor range should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("nested generic processor range should lower to optimized MIR");
        let dump = onda_mir::format_program(mir.as_program());
        assert!(dump.contains("intrinsic range_wrap"), "{dump}");
        assert!(
            mir.state
                .iter()
                .find(|state| state.name == "wrapper.counter__position")
                .and_then(|state| state.integer_range)
                .is_some(),
            "{dump}"
        );
    }

    #[test]
    fn ranged_dynamic_for_bound_eliminates_safe_array_clamps() {
        let source = r#"
proc Sum<T>:
  init:
    values: T[16]
    count: i32 = 8 {0..9}
    base: i32 = 0 {0..8, wrap}

  sample:
    total: T = T(0)
    for i in 0..count:
      total += values[base + i]
    out1 = total

init:
  count: i32 = 0 {0..2}
  sum = Sum<f32>()

sample:
  out1 = sum()
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("ranged dynamic loop should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("ranged dynamic loop should lower to optimized MIR");
        let dump = onda_mir::format_program(mir.as_program());
        let count_range = mir
            .state
            .iter()
            .find(|state| state.name == "sum.count")
            .and_then(|state| state.integer_range)
            .expect("nested count state should retain its declared range");
        assert_eq!(count_range.min, onda_mir::ScalarValue::I32(0), "{dump}");
        assert_eq!(count_range.max, onda_mir::ScalarValue::I32(8), "{dump}");
        assert!(
            mir.functions
                .iter()
                .find(|function| function.name.ends_with(".__onda_proc_step"))
                .and_then(|function| {
                    function
                        .params
                        .iter()
                        .find(|parameter| parameter.name == "self.count")
                })
                .and_then(|parameter| parameter.integer_range)
                .is_some(),
            "{dump}"
        );
        assert!(dump.contains("] unchecked"), "{dump}");
        assert!(!dump.contains("] clamp"), "{dump}");
    }

    #[test]
    fn integer_ranges_cross_value_parameters_and_scalar_returns() {
        let source = r#"
const N = 8

def bounded(index: i32):
  result = index {N, wrap}
  return result

def forward(index: i32):
  return bounded(index)

struct Table:
  values: f32[N]

  def read(self, index: i32):
    return self.values[index]

init:
  table: Table
  cursor = 0 {N, wrap}

sample:
  out1 = table.read(forward(cursor))
  cursor += 1
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("range-carrying calls should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("range-carrying calls should lower to optimized MIR");
        let dump = onda_mir::format_program(mir.as_program());

        let (read_function, read) = mir
            .functions
            .iter()
            .enumerate()
            .find(|(_, function)| function.name.ends_with(".read"))
            .expect("read helper should be present");
        let read_ranges = onda_mir::analyze_program_integer_ranges(mir.as_program());
        let read_index = read
            .params
            .iter()
            .position(|parameter| parameter.name == "index")
            .expect("read helper should retain its index parameter");
        let range = read_ranges
            .function(onda_mir::FunctionId::new(read_function as u32))
            .and_then(|ranges| ranges.parameter(onda_mir::ParameterId::new(read_index as u32)))
            .expect("the call site should constrain read's index parameter");
        assert_eq!(range.min(), 0, "{dump}");
        assert_eq!(range.max(), 7, "{dump}");
        assert!(dump.contains("] unchecked"), "{dump}");
        assert!(!dump.contains("] clamp"), "{dump}");
    }

    #[test]
    fn constant_for_indices_remove_bounds_normalization_across_surfaces() {
        let source = r#"
const N = 4

struct Cell:
  value: f32

proc Voice:
  ins 1
  outs 1
  params:
    gain = 0.5

  sample:
    out1 = in1 * gain

ins N
outs N
params N

init:
  cells: Cell[N]
  voices: Voice[N] = Voice()

sample:
  for i in 0..N:
    cells[i].value = ins[i] + params[i]
    outs[i] = voices[i](cells[i].value)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("constant indexed surfaces should analyze");
        let mir = lower_program_to_optimized_mir(&typed)
            .expect("constant indexed surfaces should lower to optimized MIR");
        let dump = onda_mir::format_program(mir.as_program());

        assert!(dump.contains(".$forward_body"), "{dump}");
        assert!(
            dump.contains("integer_range=clamp(i32(0)..=i32(3))"),
            "{dump}"
        );
        assert!(dump.contains("] unchecked"), "{dump}");
        assert!(!dump.contains("] clamp"), "{dump}");
        assert!(!dump.contains("intrinsic range_clamp"), "{dump}");
    }

    #[test]
    fn loop_variables_are_immutable_in_all_scopes() {
        for source in [
            r#"
sample:
  for i in 0..4:
    i = 2
  out1 = 0.0
"#,
            r#"
init:
  for i in 0..4:
    i = 2

sample:
  out1 = 0.0
"#,
            r#"
def bad() -> i32:
  for i in 0..4:
    i = 2
  return 0

sample:
  out1 = f32(bad())
"#,
            r#"
const def bad() -> i32:
  for i in 0..4:
    i = 2
  return 0

const Result = bad()

sample:
  out1 = f32(Result)
"#,
            r#"
task bad():
  for i in 0..4:
    i = 2
    yield

block:
  await bad()
  sample:
    out1 = 0.0
"#,
            r#"
proc P:
  task bad():
    for i in 0..4:
      i = 2
      yield
  block:
    await bad()
    sample:
      out1 = 0.0

init:
  p = P()

sample:
  out1 = p()
"#,
        ] {
            assert_analyze_error_contains(source, "cannot assign to loop variable 'i'");
        }
    }

    #[test]
    fn explicit_unsafe_array_access_lowers_to_unchecked_bounds() {
        let source = r#"
init:
  values: f32[4]

sample:
  write_unsafe(values, 2, 0.5)
  out1 = read_unsafe(values, 2)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("explicit unsafe operations should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("explicit unsafe operations should lower through trusted source MIR");
    }

    #[test]
    fn unsafe_access_rejects_non_numeric_indices_during_analysis() {
        let source = r#"
init:
  values: f32[4]

buffers:
  bank: f32 { count = 2 }

sample:
  write_unsafe(values, true, 0.5)
  out1 = read_unsafe(values, false)
  selected = read_unsafe(bank[true], 0)
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("unsafe indices must be numeric");
        let index_errors = errors
            .iter()
            .filter(|diagnostic| {
                diagnostic
                    .message
                    .contains("index argument requires numeric type, got Bool")
            })
            .count();
        assert_eq!(index_errors, 3, "{errors:?}");
    }

    #[test]
    fn write_unsafe_rejects_incompatible_values_during_analysis() {
        let source = r#"
init:
  values: f32[4]

sample:
  write_unsafe(values, 0, true)
  out1 = values[0]
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("unsafe writes must preserve the element type");
        assert!(
            errors.iter().any(|diagnostic| diagnostic
                .message
                .contains("'write_unsafe' value type mismatch: cannot assign Bool to F32")),
            "{errors:?}"
        );
    }

    #[test]
    fn write_unsafe_rejects_read_only_input_arrays_during_analysis() {
        let source = r#"
ins:
  source: f32[2]

sample:
  write_unsafe(source, 0, 1.0)
  out1 = source[0]
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("write_unsafe must reject a read-only input array");
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains("storage 'source' is read-only")),
            "{errors:?}"
        );
    }

    #[test]
    fn write_unsafe_rejects_aggregate_arrays_during_analysis() {
        let source = r#"
struct Cell:
  value: f32

init:
  cells: Cell[2]

sample:
  write_unsafe(cells, 0, 1.0)
  out1 = 0.0
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("write_unsafe must reject aggregate assignment");
        assert!(
            errors.iter().any(|diagnostic| diagnostic
                .message
                .contains("write_unsafe does not support aggregate array 'cells'")),
            "{errors:?}"
        );
    }

    #[test]
    fn aggregate_read_unsafe_rejects_scalar_value_contexts() {
        let source = r#"
struct Cell:
  value: f32

init:
  cells: Cell[2]

sample:
  out1 = read_unsafe(cells, 0)
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("an aggregate reference must not become a scalar value");
        assert!(
            errors.iter().any(|diagnostic| {
                diagnostic.message.contains(
                "aggregate read_unsafe from 'cells' is only valid in an alias or reference argument"
            )
            }),
            "{errors:?}"
        );
    }

    #[test]
    fn unsafe_dynamic_interface_access_preserves_direction_permissions() {
        let source = r#"
ins 2
outs 2
params:
  controls: f32[2] = [0.0, 1.0]

sample:
  write_unsafe(ins, 0, 1.0)
  controls.write_unsafe(0, 1.0)
  outs.write_unsafe(0, read_unsafe(outs, 0))
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("unsafe interface access must preserve read/write direction");
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains("storage 'ins' is read-only")),
            "{errors:?}"
        );
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains("storage 'outs' is write-only")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|diagnostic| diagnostic
                .message
                .contains("storage 'controls' is read-only")),
            "{errors:?}"
        );
    }

    #[test]
    fn unsafe_buffer_access_arity_matches_the_declared_shape() {
        let source = r#"
buffers:
  stereo: f32[2]

sample:
  out1 = read_unsafe(stereo, 0)
"#;
        let errors = analyze(parse_program(source).expect("source should parse"))
            .expect_err("a stereo buffer requires channel and frame indices");
        assert!(
            errors
                .iter()
                .any(|diagnostic| diagnostic.message.contains("expects 2 index arguments")),
            "{errors:?}"
        );
    }

    #[test]
    fn explicit_unsafe_buffer_access_lowers_for_resources_and_parameters() {
        let source = r#"
def copy_at(src: buffer<f32>, dst: buffer<f32>, index: i32):
  value = read_unsafe(src, index)
  write_unsafe(dst, index, value)
  return value

buffers:
  source: f32
  destination: f32
  stereo: f32[2]

sample:
  write_unsafe(stereo, 1, 0, 0.25)
  out1 = copy_at(source, destination, 0) + read_unsafe(stereo, 1, 0)
"#;
        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("explicit unsafe buffer operations should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("unsafe buffer operations should lower through trusted source MIR");
    }

    #[test]
    fn compile_inputs_replace_config_initializers_and_recompute_derived_constants() {
        let source = r#"
config const Base: i32 = 4
const def doubled(value: i32) -> i32:
  return value * 2
const Size: i32 = doubled(Base)

outs Size
sample:
  for i in 0..Size:
    outs[i] = 0.0
"#;
        let program = parse_program(source).expect("source should parse");
        let mut inputs = CompileInputs::default();
        inputs.constants.insert(
            "Base".to_owned(),
            ConstValue::Scalar(TypedConstValue::I32(8)),
        );
        let typed = analyze_with_options_and_inputs(program, AnalysisOptions::default(), &inputs)
            .expect("selected configuration should analyze");
        assert_eq!(typed.outs.len(), 16);
    }

    #[test]
    fn compile_inputs_are_selected_before_namespace_specialization() {
        let source = r#"
config const Size: i32 = 2
namespace LUT<N = 2>:
  const Table: i32[N] = [10, 20, 30, 40]
sample:
  out1 = f32(LUT<Size>::Table[3])
"#;
        let program = parse_program(source).expect("source should parse");
        let mut inputs = CompileInputs::default();
        inputs.constants.insert(
            "Size".to_owned(),
            ConstValue::Scalar(TypedConstValue::I32(4)),
        );
        let typed = analyze_with_options_and_inputs(program, AnalysisOptions::default(), &inputs)
            .expect("selected constants should drive namespace instantiation");
        let table = typed
            .const_arrays
            .iter()
            .find(|array| array.name.ends_with("::Table"))
            .expect("specialized namespace table");
        assert_eq!(table.len, 4);
        assert_eq!(table.values[3], TypedConstValue::I32(40));
    }

    #[test]
    fn configuration_defaults_see_previously_selected_configuration_values() {
        let source = r#"
config const Base: i32 = 4
config const Size: i32 = Base * 2
sample:
  out1 = 0.0
"#;
        let program = parse_program(source).expect("source should parse");
        let defaults = inspect_compile_constants(
            program.clone(),
            AnalysisOptions::default(),
            &CompileInputs::default(),
        )
        .expect("source defaults should resolve");
        assert_eq!(
            defaults
                .iter()
                .map(|descriptor| descriptor.value.clone())
                .collect::<Vec<_>>(),
            vec![
                ConstValue::Scalar(TypedConstValue::I32(4)),
                ConstValue::Scalar(TypedConstValue::I32(8)),
            ]
        );

        let mut inputs = CompileInputs::default();
        inputs.constants.insert(
            "Base".to_owned(),
            ConstValue::Scalar(TypedConstValue::I32(8)),
        );
        let selected = inspect_compile_constants(program, AnalysisOptions::default(), &inputs)
            .expect("later defaults should see earlier host selections");
        assert_eq!(
            selected
                .iter()
                .map(|descriptor| descriptor.value.clone())
                .collect::<Vec<_>>(),
            vec![
                ConstValue::Scalar(TypedConstValue::I32(8)),
                ConstValue::Scalar(TypedConstValue::I32(16)),
            ]
        );
    }

    #[test]
    fn compile_inputs_support_every_current_value_const_type() {
        let source = r#"
config const Enabled: bool = false
config const I32Value: i32 = 1
config const I64Value: i64 = i64(2)
config const F32Value: f32 = 3.0
config const F64Value: f64 = 4.0
config const Fixed: i32[2] = [5, 6]
config const Dynamic: f64[] = [7.0]

sample:
  out1 = 0.0
"#;
        let program = parse_program(source).expect("source should parse");
        let mut inputs = CompileInputs::default();
        inputs.constants.extend([
            (
                "Enabled".to_owned(),
                ConstValue::Scalar(TypedConstValue::Bool(true)),
            ),
            (
                "I32Value".to_owned(),
                ConstValue::Scalar(TypedConstValue::I32(10)),
            ),
            (
                "I64Value".to_owned(),
                ConstValue::Scalar(TypedConstValue::I64(9_007_199_254_740_993)),
            ),
            (
                "F32Value".to_owned(),
                ConstValue::Scalar(TypedConstValue::F32(0.25)),
            ),
            (
                "F64Value".to_owned(),
                ConstValue::Scalar(TypedConstValue::F64(0.125)),
            ),
            (
                "Fixed".to_owned(),
                ConstValue::Array {
                    elem_ty: PrimitiveType::I32,
                    len: 2,
                    values: vec![TypedConstValue::I32(11), TypedConstValue::I32(12)],
                },
            ),
            (
                "Dynamic".to_owned(),
                ConstValue::Array {
                    elem_ty: PrimitiveType::F64,
                    len: 3,
                    values: vec![
                        TypedConstValue::F64(1.0),
                        TypedConstValue::F64(2.0),
                        TypedConstValue::F64(3.0),
                    ],
                },
            ),
        ]);
        let descriptors = inspect_compile_constants(program, AnalysisOptions::default(), &inputs)
            .expect("every current const value type should resolve");
        assert_eq!(descriptors.len(), 7);
        assert!(matches!(
            descriptors.last(),
            Some(CompileConstDescriptor {
                kind: CompileConstKind::Array,
                value: ConstValue::Array { len: 3, .. },
                ..
            })
        ));
    }

    #[test]
    fn fixed_config_array_shape_tracks_selected_upstream_constants() {
        let source = r#"
config const Size: i32 = 4
config const Values: f32[Size] = [0.0, 0.25, 0.5, 1.0]
sample:
  out1 = 0.0
"#;
        let program = parse_program(source).expect("source should parse");
        let mut inputs = CompileInputs::default();
        inputs.constants.insert(
            "Size".to_owned(),
            ConstValue::Scalar(TypedConstValue::I32(8)),
        );
        let default_errors =
            inspect_compile_constants(program.clone(), AnalysisOptions::default(), &inputs)
                .expect_err("the source default must match the selected fixed shape");
        assert!(default_errors.iter().any(|diagnostic| {
            diagnostic.message.contains("expected 8") || diagnostic.message.contains("length 8")
        }));

        inputs.constants.insert(
            "Values".to_owned(),
            ConstValue::Array {
                elem_ty: PrimitiveType::F32,
                len: 4,
                values: vec![TypedConstValue::F32(0.0); 4],
            },
        );
        let override_errors =
            inspect_compile_constants(program.clone(), AnalysisOptions::default(), &inputs)
                .expect_err("a host array must match the selected fixed shape");
        assert!(override_errors.iter().any(|diagnostic| {
            diagnostic.message.contains("expected 8") || diagnostic.message.contains("length 8")
        }));

        inputs.constants.insert(
            "Values".to_owned(),
            ConstValue::Array {
                elem_ty: PrimitiveType::F32,
                len: 8,
                values: vec![TypedConstValue::F32(0.0); 8],
            },
        );
        let descriptors = inspect_compile_constants(program, AnalysisOptions::default(), &inputs)
            .expect("a matching selected array should resolve");
        assert!(matches!(
            descriptors.get(1),
            Some(CompileConstDescriptor {
                kind: CompileConstKind::FixedArray,
                value: ConstValue::Array { len: 8, .. },
                ..
            })
        ));
    }

    #[test]
    fn compile_inputs_reject_unknown_ordinary_and_mistyped_targets() {
        let source = r#"
config const Configured: i32 = 1
const Ordinary: i32 = 2
sample:
  out1 = 0.0
"#;
        let program = parse_program(source).expect("source should parse");
        for (name, value, expected) in [
            (
                "Missing",
                ConstValue::Scalar(TypedConstValue::I32(1)),
                "unknown configuration constant",
            ),
            (
                "Ordinary",
                ConstValue::Scalar(TypedConstValue::I32(1)),
                "is not host-configurable",
            ),
            (
                "Configured",
                ConstValue::Scalar(TypedConstValue::F32(1.0)),
                "expects i32",
            ),
        ] {
            let mut inputs = CompileInputs::default();
            inputs.constants.insert(name.to_owned(), value);
            let errors =
                inspect_compile_constants(program.clone(), AnalysisOptions::default(), &inputs)
                    .expect_err("invalid compile input should fail");
            assert!(
                errors
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(expected)),
                "{errors:?}"
            );
        }
    }

    #[test]
    fn configuration_constants_require_explicit_types() {
        let program = parse_program("config const Untyped = 1\nsample:\n  out1 = 0.0\n")
            .expect("the parser should leave type validation to semantics");
        let errors = inspect_compile_constants(
            program,
            AnalysisOptions::default(),
            &CompileInputs::default(),
        )
        .expect_err("configuration constants need a stable host-facing type");
        assert!(errors
            .iter()
            .any(|diagnostic| diagnostic.message.contains("requires an explicit type")));
    }

    #[test]
    fn supplied_compile_input_does_not_evaluate_its_source_default() {
        let source = r#"
config const Selected: i32 = missing_default
sample:
  out1 = f32(Selected)
"#;
        let program = parse_program(source).expect("source should parse");
        let mut inputs = CompileInputs::default();
        inputs.constants.insert(
            "Selected".to_owned(),
            ConstValue::Scalar(TypedConstValue::I32(7)),
        );
        analyze_with_options_and_inputs(program, AnalysisOptions::default(), &inputs)
            .expect("the supplied initializer should replace the source default");
    }

    #[test]
    fn delegates_dispatch_owner_when_handlers() {
        let source = r#"
delegate finished(reason: i32)

init:
  result: i32 = 0

event trigger():
  finished(7)

when finished(reason):
  result = reason

sample:
  out1 = f32(result)
"#;
        let program = parse_program(source).expect("delegate source should parse");
        let typed = analyze(program).expect("delegate source should analyze");
        assert_eq!(typed.delegates.len(), 1);
        assert_eq!(typed.delegates[0].name, "finished");
    }

    #[test]
    fn delegate_when_handlers_receive_owner_buffers() {
        let source = r#"
delegate top_fired()

proc Child:
  delegate fired()
  buffers:
    own: f32
  when fired():
    own[0] = own[0] + 2.0
  block:
    held = 0.0
    sample:
      fired()
      out1 = held

proc Parent:
  delegate own_fired()
  buffers:
    own: f32
    routed: f32
    child_own: f32
  init:
    child = Child(own = child_own)
  when own_fired():
    own[0] = own[0] + 1.0
  when child.fired():
    routed[0] = routed[0] + 10.0
  sample:
    own_fired()
    out1 = child()

buffers:
  top: f32
  own: f32
  routed: f32
  child_own: f32

when top_fired():
  top[0] = top[0] + 100.0

when parent.own_fired():
  top[0] = top[0] + 1000.0

init:
  parent = Parent(own = own, routed = routed, child_own = child_own)

sample:
  top_fired()
  out1 = parent()
"#;
        let program = parse_program(source).expect("delegate buffer source should parse");
        let typed = analyze(program).expect("delegate handlers should receive owner buffers");
        lower_program_to_optimized_mir(&typed)
            .expect("delegate handler buffer access should lower to MIR");
    }

    #[test]
    fn indexed_child_delegate_handlers_receive_owner_buffers() {
        let source = r#"
proc Child:
  delegate fired()
  sample:
    fired()
    out1 = 0.0

proc Parent:
  buffers:
    source: f32
  init:
    children: Child[2] = Child()
  when children[1].fired():
    source[0] = source[0] + 1.0
  sample:
    out1 = children[0]() + children[1]()

buffers:
  source: f32
init:
  parent = Parent(source = source)
sample:
  out1 = parent()
"#;
        let program = parse_program(source).expect("indexed delegate source should parse");
        let typed = analyze(program).expect("indexed handler should receive its owner buffer");
        lower_program_to_optimized_mir(&typed)
            .expect("indexed delegate handler buffer access should lower to MIR");
    }

    #[test]
    fn delegates_reject_init_reachability_through_runtime_defs() {
        assert_analyze_error_contains(
            r#"
delegate finished()

def publish():
  finished()

init:
  publish()

sample:
  out1 = 0.0
"#,
            "init -> def publish -> delegate finished",
        );
    }

    #[test]
    fn delegate_reachability_uses_the_selected_overload() {
        for source in [
            r#"
delegate finished()

def helper(value: f32):
  return

def helper(value: i32):
  finished()

init:
  helper(1.0)

sample:
  out1 = 0.0
"#,
            r#"
delegate finished()

def helper(value: i32):
  finished()

def helper(value: f32):
  return

init:
  helper(1.0)

sample:
  out1 = 0.0
"#,
            r#"
delegate finished(value: f32)

def helper(value: f32):
  return

def helper(value: i32):
  finished(f32(value))

when finished(value):
  helper(value)

sample:
  out1 = 0.0
"#,
        ] {
            let program = parse_program(source).expect("overloaded delegate source should parse");
            analyze(program)
                .expect("a call to the pure overload must not inherit another overload's effects");
        }
    }

    #[test]
    fn delegate_when_overloads_use_owner_visible_types() {
        for source in [
            r#"
params:
  selected: i32 = 1

delegate finished()

def helper(value: f32):
  return

def helper(value: i32):
  return

when finished():
  helper(selected)

sample:
  out1 = 0.0
"#,
            r#"
proc Child:
  delegate finished()

  event trigger():
    finished()

  sample:
    out1 = 0.0

def helper(value: i32):
  return

def helper(value: f32):
  return

init:
  selected: i32 = 1
  child = Child()

when child.finished():
  helper(selected)

sample:
  out1 = child()
"#,
            r#"
params:
  selected: i64 = 1

delegate finished()

def identity(value):
  return value

init:
  observed: i64 = 0

when finished():
  observed = identity(selected)

sample:
  out1 = f32(observed)
"#,
        ] {
            let program = parse_program(source).expect("delegate overload source should parse");
            let typed = analyze(program)
                .expect("when handlers should use the same owner-visible types as other blocks");
            lower_program_to_optimized_mir(&typed)
                .expect("owner-typed when overload should lower to valid MIR");
        }
    }

    #[test]
    fn delegate_reachability_rejects_the_selected_effectful_overload() {
        for source in [
            r#"
delegate finished()

def helper(value: f32):
  finished()

def helper(value: i32):
  return

init:
  helper(1.0)

sample:
  out1 = 0.0
"#,
            r#"
delegate finished()

def helper(value: i32):
  return

def helper(value: f32):
  finished()

init:
  helper(1.0)

sample:
  out1 = 0.0
"#,
            r#"
delegate finished()

struct Value:
  x: f32

def helper(value: f32):
  return

def helper(value: Value):
  finished()

init:
  value = Value(1.0)
  helper(value)

sample:
  out1 = 0.0
"#,
            r#"
delegate finished()

buffers:
  source: f32

def helper(value: f32):
  return

def helper(value: buffer<f32>):
  finished()

init:
  helper(source)

sample:
  out1 = 0.0
"#,
            r#"
delegate finished()

const values: f32[1] = [0.0]

def helper(value: i32[]):
  return

def helper(value: f32[]):
  finished()

init:
  helper(values)

sample:
  out1 = 0.0
"#,
            r#"
delegate finished()

params:
  selected: i32 = 1

def helper(value: f32):
  return

def helper(value: i32):
  finished()

init:
  helper(selected)

sample:
  out1 = 0.0
"#,
        ] {
            assert_analyze_error_contains(
                source,
                "init code in the top-level owner cannot call or reach a delegate",
            );
        }
    }

    #[test]
    fn delegates_reject_value_use() {
        assert_analyze_error_contains(
            r#"
delegate finished()

sample:
  result = finished()
  out1 = result
"#,
            "has no result and must be used as a statement",
        );
    }

    #[test]
    fn owner_local_callable_names_cannot_be_shadowed_by_value_bindings() {
        for (source, expected) in [
            (
                r#"
delegate finished()

def invoke(finished: i32):
  return

sample:
  out1 = 0.0
"#,
                "function parameter 'finished' conflicts with owner-local delegate 'finished'",
            ),
            (
                r#"
delegate finished(value: i32)

when finished(finished):
  print(finished)

sample:
  out1 = 0.0
"#,
                "when binding 'finished' conflicts with owner-local delegate 'finished'",
            ),
            (
                r#"
event update():
  return

sample:
  update = 1
  out1 = 0.0
"#,
                "binding 'update' conflicts with owner-local event 'update'",
            ),
            (
                r#"
task worker():
  yield

sample:
  for worker in 0..1:
    out1 = f32(worker)
"#,
                "loop variable 'worker' conflicts with owner-local task 'worker'",
            ),
            (
                r#"
def helper():
  return

def invoke(helper: i32):
  return

sample:
  out1 = 0.0
"#,
                "function parameter 'helper' conflicts with owner-local function 'helper'",
            ),
            (
                r#"
def helper():
  return

sample:
  const helper = 1
  out1 = 0.0
"#,
                "local constant 'helper' conflicts with owner-local function 'helper'",
            ),
            (
                r#"
proc Voice:
  sample:
    out1 = 0.0

def inspect(Voice: i32):
  return

sample:
  out1 = 0.0
"#,
                "function parameter 'Voice' conflicts with owner-local processor 'Voice'",
            ),
            (
                r#"
struct Pair:
  value: f32

def inspect(Pair: i32):
  return

sample:
  out1 = 0.0
"#,
                "function parameter 'Pair' conflicts with owner-local struct 'Pair'",
            ),
            (
                r#"
proc Voice:
  delegate finished()

  def invoke(finished: i32):
    return

  sample:
    out1 = 0.0

init:
  voice = Voice()

sample:
  out1 = voice()
"#,
                "function parameter 'finished' conflicts with owner-local delegate 'finished' in processor 'Voice'",
            ),
        ] {
            assert_analyze_error_contains(source, expected);
        }
    }

    #[test]
    fn delegates_reject_unshadowed_bare_value_use() {
        assert_analyze_error_contains(
            r#"
delegate finished()

def invalid() -> i32:
  return finished

sample:
  out1 = 0.0
"#,
            "delegate 'finished' is callable only and cannot be used as a value",
        );
    }

    #[test]
    fn delegates_reject_owner_member_collisions() {
        assert_analyze_error_contains(
            r#"
delegate finished()

event finished():
  return

sample:
  out1 = 0.0
"#,
            "delegate 'finished' conflicts with event 'finished'",
        );
    }

    #[test]
    fn delegates_reject_recursive_event_dispatch_with_source_names() {
        assert_analyze_error_contains(
            r#"
delegate finished()

event restart():
  finished()

when finished():
  restart()

sample:
  out1 = 0.0
"#,
            "event restart -> delegate finished -> when finished #1 -> event restart",
        );
    }

    #[test]
    fn delegates_reject_child_dispatch_cycles_with_source_names() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

init:
  child = Child()

when child.fired():
  child.trigger()

sample:
  out1 = child()
"#,
            "when child.fired #1 -> delegate Child.fired through child -> when child.fired #1",
        );
    }

    #[test]
    fn delegates_reject_resetting_the_active_dispatching_task() {
        assert_analyze_error_contains(
            r#"
delegate finished()

task worker():
  finished()
  yield

when finished():
  worker.reset()

block:
  await worker()
  sample:
    out1 = 0.0
"#,
            "cannot dispatch a delegate whose synchronous handler may reset that active task",
        );
    }

    #[test]
    fn delegates_reject_child_dispatch_resetting_the_active_parent_task() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

init:
  child = Child()

task worker():
  child.trigger()
  yield

when child.fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = child()
"#,
            "cannot dispatch a delegate whose synchronous handler may reset that active task",
        );
    }

    #[test]
    fn delegates_reject_child_step_resetting_the_active_parent_task() {
        for source in [
            r#"
proc Child:
  kouts:
    value
  delegate fired()
  block:
    fired()
    value = 0.0

init:
  child = Child()

task worker():
  value = child()
  yield

when child.fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = 0.0
"#,
            r#"
proc Child:
  kouts:
    value
  delegate fired()
  block:
    fired()
    value = 0.0

init:
  children: Child[2] = Child()
  index: i32 = 0

task worker():
  value = children[index]()
  yield

when children[0].fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = 0.0
"#,
        ] {
            assert_analyze_error_contains(
                source,
                "cannot dispatch a delegate whose synchronous handler may reset that active task",
            );
        }
    }

    #[test]
    fn delegates_reject_child_task_step_resetting_the_active_parent_task() {
        assert_analyze_error_contains(
            r#"
proc Child:
  kouts:
    value
  delegate fired()
  task publish():
    fired()
    yield
  block:
    await publish()
    value = 0.0

init:
  child = Child()

task worker():
  value = child()
  yield

when child.fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = 0.0
"#,
            "cannot dispatch a delegate whose synchronous handler may reset that active task",
        );
    }

    #[test]
    fn delegates_do_not_attribute_unawaited_child_tasks_to_proc_steps() {
        let program = parse_program(
            r#"
proc Child:
  kouts:
    value
  delegate fired()
  task publish():
    fired()
    yield
  block:
    value = 0.0

init:
  child = Child()

task worker():
  value = child()
  yield

when child.fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = 0.0
"#,
        )
        .expect("unawaited child task source should parse");
        analyze(program).expect("an unawaited child task cannot publish during a proc step");
    }

    #[test]
    fn delegates_track_child_event_dispatch_through_local_aliases() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

init:
  children: Child[1] = Child()
  child = children[0]
  child.trigger()

sample:
  out1 = children[0]()
"#,
            "init code in the top-level owner cannot call or reach a delegate",
        );
    }

    #[test]
    fn delegates_track_child_event_dispatch_through_conditional_aliases() {
        for source in [
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

def relay(target):
  target.trigger()

params:
  choose = false

init:
  children: Child[2] = Child()
  target = children[0]
  if choose:
    target = children[1]
  target.trigger()

sample:
  out1 = children[0]() + children[1]()
"#,
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

def relay(target):
  target.trigger()

params:
  choose = false

init:
  left = Child()
  right = Child()
  target = left
  if choose:
    target = right
  relay(target)

sample:
  out1 = left() + right()
"#,
        ] {
            assert_analyze_error_contains(
                source,
                "init code in the top-level owner cannot call or reach a delegate",
            );
        }
    }

    #[test]
    fn conditional_child_aliases_preserve_indexed_delegate_routes() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

params:
  choose = false

init:
  children: Child[2] = Child()

task worker():
  target = children[0]
  if choose:
    target = children[1]
  target.trigger()
  yield

when children[1].fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = children[0]() + children[1]()
"#,
            "cannot dispatch a delegate whose synchronous handler may reset that active task",
        );
    }

    #[test]
    fn delegates_reject_forwarded_child_dispatch_resetting_the_active_task() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

def relay(targets):
  targets[0].trigger()

init:
  children: Child[2] = Child()

task worker():
  relay(children)
  yield

when children[0].fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = children[0]() + children[1]()
"#,
            "cannot dispatch a delegate whose synchronous handler may reset that active task",
        );
    }

    #[test]
    fn indexed_child_dispatch_only_applies_the_selected_delegate_route() {
        let source = r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

init:
  children: Child[2] = Child()

task worker():
  children[0].trigger()
  yield

when children[1].fired():
  worker.reset()

block:
  await worker()
  sample:
    out1 = children[0]() + children[1]()
"#;
        let program = parse_program(source).expect("delegate route source should parse");
        analyze(program).expect("an unrelated indexed route must not reset the active task");
    }

    #[test]
    fn delegates_cannot_be_called_through_child_receivers() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate finished()
  sample:
    out1 = 0.0

init:
  child = Child()

sample:
  child.finished()
  out1 = child()
"#,
            "cannot call delegate 'finished' through child receiver 'child'",
        );
    }

    #[test]
    fn delegates_cannot_be_called_through_indexed_child_receivers() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate finished()
  sample:
    out1 = 0.0

init:
  children: Child[2] = Child()

sample:
  children[0].finished()
  out1 = children[0]() + children[1]()
"#,
            "cannot call delegate 'finished' through child receiver 'children'",
        );
    }

    #[test]
    fn delegates_cannot_be_called_through_child_receiver_aliases() {
        assert_analyze_error_contains(
            r#"
proc Child:
  delegate finished()
  sample:
    out1 = 0.0

init:
  children: Child[1] = Child()

sample:
  child = children[0]
  child.finished()
  out1 = children[0]()
"#,
            "cannot call delegate 'finished' through child receiver 'child'",
        );
    }

    #[test]
    fn delegate_dispatch_analysis_reuses_repeated_helper_expansions() {
        let mut source = String::from(
            r#"
proc Child:
  delegate fired()
  event trigger():
    fired()
  sample:
    out1 = 0.0

def leaf(target):
  target.trigger()
"#,
        );
        for depth in 0..20 {
            let callee = if depth == 0 {
                "leaf".to_owned()
            } else {
                format!("helper{}", depth - 1)
            };
            source.push_str(&format!(
                "\ndef helper{depth}(target):\n  {callee}(target)\n  {callee}(target)\n"
            ));
        }
        source.push_str(
            r#"
init:
  child = Child()
  helper19(child)

sample:
  out1 = child()
"#,
        );

        assert_analyze_error_contains(
            &source,
            "init code in the top-level owner cannot call or reach a delegate",
        );
    }

    #[test]
    fn delegate_effectful_top_level_defs_are_owner_local() {
        assert_analyze_error_contains(
            r#"
delegate finished()

def publish():
  finished()

proc Child:
  sample:
    publish()
    out1 = 0.0

init:
  child = Child()

sample:
  out1 = child()
"#,
            "may publish a delegate owned by another owner",
        );
    }

    #[test]
    fn processor_outputs_can_be_tuple_destructured() {
        let source = r#"
proc Stereo:
  outs:
    left
    right
  sample:
    left = 1.0
    right = 2.0

outs:
  out1
  out2
init:
  stereo = Stereo()
sample:
  (out1, out2) = stereo()
"#;

        let program = parse_program(source).expect("source should parse");
        analyze(program).expect("processor outputs should destructure");
    }

    #[test]
    fn processor_output_destructuring_supports_bare_targets_and_discards() {
        let source = r#"
proc Stereo:
  outs:
    left
    right
  sample:
    left = 1.0
    right = 2.0

init:
  stereo = Stereo()
sample:
  out1, _ = stereo()
"#;

        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("processor outputs should support discarded targets");
        lower_program_to_optimized_mir(&typed)
            .expect("discarded processor outputs should lower without a binding");
    }

    #[test]
    fn nested_processor_outputs_can_be_tuple_destructured() {
        let source = r#"
proc Stereo:
  outs:
    left
    right
  sample:
    left = 1.0
    right = 2.0

proc Parent:
  init:
    stereo = Stereo()
  outs:
    out1
    out2
  sample:
    (out1, out2) = stereo()

init:
  parent = Parent()
outs:
  out1
  out2
sample:
  (out1, out2) = parent()
"#;

        let program = parse_program(source).expect("source should parse");
        analyze(program).expect("nested processor outputs should destructure");
    }

    #[test]
    fn nested_processor_state_preserves_struct_array_field_paths() {
        let source = r#"
struct Data:
  storage: f32[8]

proc Line:
  init:
    data = Data()
    index = 0 {8, wrap}
  sample:
    data.storage[index] = in1
    out1 = data.storage[index]
    index += 1

proc Effect:
  init:
    line = Line()
  sample:
    out1 = line(in1)

init:
  effect = Effect()
sample:
  out1 = effect(1.0)
"#;

        let typed = analyze(parse_program(source).expect("source should parse"))
            .expect("nested struct array state should analyze");
        lower_program_to_optimized_mir(&typed)
            .expect("nested struct array state should lower to MIR");
    }

    #[test]
    fn dynamically_indexed_processor_outputs_can_be_tuple_destructured() {
        let source = r#"
proc Stereo:
  outs:
    left
    right
  sample:
    left = 1.0
    right = 2.0

outs:
  out1
  out2
init:
  voices: Stereo[2] = Stereo()
sample:
  i = 1
  (out1, out2) = voices[i]()
"#;

        let program = parse_program(source).expect("source should parse");
        analyze(program).expect("dynamically indexed processor outputs should destructure");
    }

    #[test]
    fn block_rate_processor_outputs_can_be_tuple_destructured_in_tasks() {
        let source = r#"
proc ControlPair:
  kouts:
    left
    right
  block:
    left = 1.0
    right = 2.0

init:
  pair = ControlPair()

task worker():
  (left, right) = pair()
  yield

block:
  await worker()
  sample:
    out1 = 0.0
"#;

        let program = parse_program(source).expect("source should parse");
        analyze(program).expect("block-rate processor outputs should destructure in a task");
    }

    #[test]
    fn processor_array_param_outputs_can_be_tuple_destructured_in_defs() {
        let source = r#"
proc Stereo:
  outs:
    left
    right
  sample:
    left = 1.0
    right = 2.0

def sum_voice(voices, i):
  (left, right) = voices[i]()
  return left + right

outs:
  out1
init:
  voices: Stereo[2] = Stereo()
sample:
  out1 = sum_voice(voices, 1)
"#;

        let program = parse_program(source).expect("source should parse");
        analyze(program).expect("processor-array def outputs should destructure");
    }

    #[test]
    fn processor_output_destructuring_reports_arity_mismatch() {
        let source = r#"
proc Stereo:
  outs:
    left
    right
  sample:
    left = 1.0
    right = 2.0

outs:
  out1
init:
  stereo = Stereo()
sample:
  (left, right, extra) = stereo()
  out1 = left
"#;
        let program = parse_program(source).expect("source should parse");
        let errors = analyze(program).expect_err("arity mismatch should fail");
        assert_eq!(errors.len(), 1, "unexpected diagnostics: {errors:#?}");
        assert!(errors.iter().any(|error| error.message.contains(
            "processor output destructuring has 3 targets, but the processor has 2 outputs"
        )));
    }

    #[test]
    fn top_level_main_uses_the_normal_entry_semantics() {
        let source = r#"
proc Main:
  params:
    gain = 0.5 {0.0, 1.0, unit = "x"}
  outs:
    output
  init:
    state = 1.0
  sample:
    output = state * gain
"#;

        let typed = analyze(parse_program(source).expect("Main entry should parse"))
            .expect("Main entry should analyze as a top-level program");
        assert_eq!(typed.outs, ["output"]);
        assert_eq!(typed.param_default("gain"), Some(0.5));
        assert!(typed.state_vars.iter().any(|name| name == "state"));
    }
