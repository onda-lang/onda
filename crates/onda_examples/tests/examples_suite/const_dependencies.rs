use super::*;

fn assert_context_output(source: &str, expected: f32) {
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level,
            },
        );
        let mut output = [0.0; 16];
        for _ in 0..32 {
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        }
        assert!(
            output.iter().all(|&value| (value - expected).abs() < 0.001),
            "{source}\n{output:?}"
        );
    }
}

fn assert_const_and_native_result(functions: impl Fn(&str) -> String, expected: &str) {
    let mut source = String::new();
    for (namespace, prefix) in [("Folded", "const "), ("Native", "")] {
        source.push_str(&format!("namespace {namespace}:\n"));
        for line in functions(prefix).lines() {
            source.push_str(&format!("  {line}\n"));
        }
    }
    source.push_str(&format!(
        "sample:\n  out1 = f32(Folded::run() == Native::run() && Folded::run() == {expected})\n"
    ));
    assert_context_output(&source, 1.0);
}

#[test]
fn const_named_arguments_preserve_source_evaluation_order() {
    for (call, expected) in [
        ("take(b = write(xs, 2), a = write(xs, 1))", 13),
        ("take(write(xs, 2), b = write(xs, 1))", 13),
        ("take(b = xs[0], a = write(xs, 1))", 11),
        ("take(a = write(xs, 2))", 22),
    ] {
        let functions = |prefix: &str| {
            format!(
            "{prefix}def write(xs: i32[1], value: i32) -> i32:\n  xs[0] = value\n  return value\n\
             {prefix}def take(a: i32, b: i32 = 0) -> i32:\n  return a + b\n\
             {prefix}def run() -> i32:\n  xs: i32[1] = [0]\n  result = {call}\n  return xs[0] * 10 + result\n"
        )
        };
        assert_const_and_native_result(functions, &expected.to_string());
    }
}

#[test]
fn cached_array_defaults_produce_independent_values_in_every_context() {
    for callable in ["", "const "] {
        let parameters = if callable.is_empty() {
            &["i32[1]"][..]
        } else {
            &["i32[1]", "i32[]", "[]"][..]
        };
        for parameter in parameters {
            for default in ["[element()]", "build()"] {
                let source = format!(
                "const def element() -> i32:\n  return i32(SR / 48000)\nconst def build() -> i32[1]:\n  return [element()]\n{callable}def change(values: {parameter} = {default}) -> i32:\n  original = values[0]\n  values[0] = 99\n  return original\ninit:\n  initial = change() + change()\nsample 2:\n  out1 = f32(initial + change() + change())\n"
            );
                assert_context_output(&source, 6.0);
            }
        }
    }
}

#[test]
fn const_and_native_literal_contexts_match_at_numeric_boundaries() {
    let literal = "(16777216.0 + 1.0) - 16777216.0";
    for (body, expected) in [
        (format!("  x: f32 = {literal}\n  return x\n"), 0),
        (format!("  x = {literal}\n  return x\n"), 0),
        (format!("  x: f32 = 0.0\n  x = {literal}\n  return x\n"), 0),
        (format!("  return {literal}\n"), 0),
        (format!("  return take({literal})\n"), 0),
        ("  return take()\n".to_owned(), 0),
        (format!("  x: f32[1] = [{literal}]\n  return x[0]\n"), 0),
        (format!("  x = [{literal}]\n  return x[0]\n"), 0),
        ("  x = make()\n  return x[0]\n".to_owned(), 0),
        (format!("  return first([{literal}])\n"), 0),
        (
            format!("  x: f32[1] = [0.0]\n  x[0] = {literal}\n  return x[0]\n"),
            0,
        ),
        (format!("  x: f32 = sin({literal})\n  return x\n"), 0),
        (format!("  x: f64 = {literal}\n  return f32(x)\n"), 1),
        (format!("  x: f32 = f32({literal})\n  return x\n"), 1),
        (
            "  x: f32 = (16777216 + 1) - 16777216\n  return x\n".to_owned(),
            1,
        ),
    ] {
        let functions = |prefix: &str| {
            format!(
                "{prefix}def make() -> f32[1]:\n  return [{literal}]\n\
                 {prefix}def first(values: f32[1]) -> f32:\n  return values[0]\n\
                 {prefix}def take(value: f32 = {literal}) -> f32:\n  return value\n\
             {prefix}def run() -> f32:\n{body}"
            )
        };
        assert_const_and_native_result(functions, &format!("{expected}.0"));
    }
}

#[test]
fn const_and_native_integer_contexts_select_width_before_arithmetic() {
    let literal = "(1073741824 * 4) / 2";
    for body in [
        format!("  x: i32 = {literal}\n  return x\n"),
        format!("  x = {literal}\n  return x\n"),
        format!("  x: i32 = 1\n  x = {literal}\n  return x\n"),
        format!("  return {literal}\n"),
        format!("  return take({literal})\n"),
        "  return take()\n".to_owned(),
        format!("  x: i32[1] = [{literal}]\n  return x[0]\n"),
        format!("  x = [{literal}]\n  return x[0]\n"),
        "  x = make()\n  return x[0]\n".to_owned(),
        format!("  return first([{literal}])\n"),
        format!("  x: i32[1] = [1]\n  x[0] = {literal}\n  return x[0]\n"),
    ] {
        assert_const_and_native_result(
            |prefix| {
                format!(
                    "{prefix}def make() -> i32[1]:\n  return [{literal}]\n\
                 {prefix}def first(values: i32[1]) -> i32:\n  return values[0]\n\
                 {prefix}def take(value: i32 = {literal}) -> i32:\n  return value\n\
                 {prefix}def run() -> i32:\n{body}"
                )
            },
            "0",
        );
    }
}

#[test]
fn const_and_native_literal_operands_follow_concrete_operation_widths() {
    for (ty, body, expected) in [
        ("i32", "  return (2147483647 + 1) / 2\n", "-1073741824"),
        ("i32", "  return (2147483647 + 1) % 3\n", "-2"),
        ("i32", "  return (1 << 31) >> 1\n", "-1073741824"),
        ("i32", "  return 1 << 32\n", "1"),
        ("i32", "  return ~(1 << 31)\n", "2147483647"),
        ("i32", "  return abs(2147483647 + 1) / 2\n", "-1073741824"),
        ("i32", "  return max(2147483647 + 1, 0)\n", "0"),
        (
            "i32",
            "  x: i32 = 0\n  return x + (1073741824 * 4) / 2\n",
            "0",
        ),
        (
            "bool",
            "  x: i32 = 0\n  return x == (1073741824 * 4) / 2\n",
            "true",
        ),
        (
            "i32",
            "  x: i32 = 0\n  return min(x, (2147483647 + 1) / 2)\n",
            "-1073741824",
        ),
        (
            "i64",
            "  x: i64 = (2147483647 + 1) / 2\n  return x\n",
            "1073741824",
        ),
        ("i32", "  return i32((2147483647 + 1) / 2)\n", "1073741824"),
        (
            "i64",
            "  return (9223372036854775807 + 1) / 2\n",
            "-4611686018427387904",
        ),
        ("i64", "  return (1 << 63) / -1\n", "-9223372036854775808"),
        ("i64", "  return 1 << 64\n", "1"),
        (
            "f32",
            "  x: f32 = 0.0\n  return x + ((16777216.0 + 1.0) - 16777216.0)\n",
            "0.0",
        ),
        (
            "bool",
            "  x: f32 = 0.0\n  return x == ((16777216.0 + 1.0) - 16777216.0)\n",
            "true",
        ),
        (
            "f32",
            "  x: f32 = 0.0\n  return max(x, (16777216.0 + 1.0) - 16777216.0)\n",
            "0.0",
        ),
        (
            "f32",
            "  x: f32 = 0.0\n  return fma(x, 1.0, (16777216.0 + 1.0) - 16777216.0)\n",
            "0.0",
        ),
        (
            "f64",
            "  x: f64 = 0.0\n  return x + ((16777216.0 + 1.0) - 16777216.0)\n",
            "1.0",
        ),
        (
            "f32",
            "  x: f32 = 0.0\n  return x + ((16777216 + 1) - 16777216)\n",
            "1.0",
        ),
        (
            "f32",
            "  x: f32 = 0.0\n  return x + f32((16777216.0 + 1.0) - 16777216.0)\n",
            "1.0",
        ),
        (
            "f32",
            "  x: i32 = 0\n  return x + ((16777216.0 + 1.0) - 16777216.0)\n",
            "0.0",
        ),
    ] {
        assert_const_and_native_result(
            |prefix| format!("{prefix}def run() -> {ty}:\n{body}"),
            expected,
        );
    }
}

#[test]
fn generic_arithmetic_uses_each_specializations_width() {
    assert_context_output(
        "def calculate<T>(x: T) -> T:\n  return x + (2147483647 + 1) / 2\n\
         sample:\n  out1 = f32(calculate(i32(0)) == -1073741824 && calculate(i64(0)) == 1073741824 && calculate(f32(0.0)) == 1073741824.0 && calculate(f64(0.0)) == 1073741824.0)\n",
        1.0,
    );
}

#[test]
fn mixed_i64_float_operations_agree_with_generic_helpers() {
    assert_context_output(
        r#"
const Half = 0.5
const FixedCount: i64 = 16777217
const FixedHalf: f32 = 0.5
const Counts: i64[1] = [FixedCount]
const FoldedSum = FixedCount + FixedHalf
const FoldedMaximum = max(FixedHalf, Counts[0])
const FoldedComparison = FixedCount > 16777216.0
def add<T>(a: T, b: T) -> T:
  return a + b
def greater<T>(a: T, b: T) -> bool:
  return a > b
init:
  count: i64 = 16777217
  half: f32 = 0.5
  limit: f32 = 16777216.0
  direct = count + Half
  helper = add(count, Half)
sample:
  correct = direct == helper && direct == 16777217.5
  correct = correct && FoldedSum == helper && FoldedMaximum == 16777217.0 && FoldedComparison
  correct = correct && count + half == helper && half + count == helper
  correct = correct && max(count, Half) == 16777217.0 && max(Half, count) == 16777217.0
  correct = correct && min(count, 16777218.0) == 16777217.0
  correct = correct && clamp(count, 16777216.0, 16777218.0) == 16777217.0
  correct = correct && count > limit && greater(count, limit)
  correct = correct && limit < count && 16777216.0 < count
  correct = correct && count > 16777216.0 && count != limit
  correct = correct && f32(count) + Half == limit && f32(count) == limit
  out1 = f32(correct)
"#,
        1.0,
    );
}

#[test]
fn const_and_native_mixed_i64_float_operations_preserve_precision() {
    for (result, expression, expected) in [
        ("f64", "count + 0.5", "16777217.5"),
        ("f64", "0.5 + count", "16777217.5"),
        ("f64", "count + half", "16777217.5"),
        ("f64", "half + count", "16777217.5"),
        ("f64", "max(count, half)", "16777217.0"),
        ("f64", "max(half, count)", "16777217.0"),
        ("f64", "min(count, 16777218.0)", "16777217.0"),
        (
            "f64",
            "min(max(count, 16777216.0), 16777218.0)",
            "16777217.0",
        ),
        ("f64", "pow(count, 1.0)", "16777217.0"),
        ("bool", "count > 16777216.0", "true"),
        ("bool", "16777216.0 < count", "true"),
        ("bool", "count != f32(16777216.0)", "true"),
        ("f32", "f32(count) + half", "16777216.0"),
    ] {
        assert_const_and_native_result(
            |prefix| {
                format!(
                    "{prefix}def run() -> {result}:\n  count: i64 = 16777217\n  half: f32 = 0.5\n  return {expression}\n"
                )
            },
            expected,
        );
    }
}

#[test]
fn contextual_power_preserves_precision_before_explicit_casts() {
    for (result, expression, expected) in [
        ("f64", "f64(pow(16777217, 1))", "16777217.0"),
        ("f32", "f32(pow(16777217, 1) - 16777216)", "1.0"),
        ("f32", "pow(16777217, 1)", "16777216.0"),
        ("f64", "f64(pow(i64(16777217), i64(1)))", "16777216.0"),
    ] {
        assert_const_and_native_result(
            |prefix| format!("{prefix}def run() -> {result}:\n  return {expression}\n"),
            expected,
        );
    }
}

#[test]
fn primitive_casts_and_boolean_comparisons_match_const_and_native_execution() {
    for (source_type, value, result_type, expected) in [
        ("i64", "4294967297", "i32", "1"),
        ("i32", "-7", "i64", "-7"),
        ("f64", "-7.75", "i32", "-7"),
        ("f32", "7.75", "i64", "7"),
        ("f64", "1.0 / 0.0", "i32", "2147483647"),
        ("f64", "-1.0 / 0.0", "i64", "-9223372036854775808"),
        ("f64", "0.0 / 0.0", "i32", "0"),
        ("f64", "0.0 / 0.0", "bool", "true"),
        ("f64", "-0.0", "bool", "false"),
        ("i64", "-1", "bool", "true"),
        ("bool", "true", "f64", "1.0"),
        ("bool", "false", "i32", "0"),
        ("f64", "16777217.0", "f32", "16777216.0"),
        ("f32", "16777217.0", "f64", "16777216.0"),
    ] {
        let expression = if result_type == "bool" {
            "value != 0".to_owned()
        } else {
            format!("{result_type}(value)")
        };
        assert_const_and_native_result(
            |prefix| {
                format!(
                    "{prefix}def run() -> {result_type}:\n  value: {source_type} = {value}\n  return {expression}\n"
                )
            },
            expected,
        );
    }
}

#[test]
fn integer_literal_subtrees_preserve_the_selected_float_context() {
    for (expression, expected) in [
        ("(16777216 + 1) + 0.5 - 16777216.0", "0.0"),
        ("0.5 + (16777216 + 1) - 16777216.0", "0.0"),
        ("max(16777216 + 1, 16777216.0) - 16777216.0", "0.0"),
        ("pow(16777216 + 1, 1.0) - 16777216.0", "0.0"),
        ("((16777216 + 1) - 16777216) + 0.0", "1.0"),
        ("((2147483647 + 1) / 2) + 0.0", "1073741824.0"),
    ] {
        assert_const_and_native_result(
            |prefix| format!("{prefix}def run() -> f32:\n  return {expression}\n"),
            expected,
        );
    }
}

#[test]
fn deferred_literal_operands_preserve_runtime_argument_effects() {
    assert_const_and_native_result(
        |prefix| {
            format!(
            "{prefix}def write(values: i32[1], value: i32) -> i32:\n  values[0] = value\n  return value\n\
             {prefix}def run() -> i32:\n  values: i32[1] = [0]\n  result = write(values, 1) + max(write(values, 2), (1073741824 * 4) / 2)\n  return values[0] * 10 + result\n"
        )
        },
        "23",
    );
}

#[test]
fn runtime_storage_and_tuple_boundaries_select_literal_width() {
    let literal = "(16777216.0 + 1.0) - 16777216.0";
    let source = format!(
        r#"
struct Data:
  narrow: f32
  wide: f64
  pair: (f32, f64)
def take_pair(value: (f32, f64)) -> (f32, f64):
  return value
def return_pair() -> (f32, f64):
  return ({literal}, {literal})
init:
  narrow: f32 = {literal}
  wide: f64 = {literal}
  values: f32[1] = [{literal}]
  data = Data(narrow = {literal}, wide = {literal}, pair = ({literal}, {literal}))
sample:
  fresh = {literal}
  narrow = {literal}
  values[0] = {literal}
  values[:] = {literal}
  local: f32[1] = [{literal}]
  pair: (f32, f64) = ({literal}, {literal})
  returned = return_pair()
  passed = take_pair(({literal}, {literal}))
  out1 = f32((fresh == 0.0 && narrow == 0.0 && wide == 1.0 && values[0] == 0.0 && local[0] == 0.0 && data.narrow == 0.0 && data.wide == 1.0 && data.pair[0] == 0.0 && data.pair[1] == 1.0 && pair[0] == 0.0 && pair[1] == 1.0 && returned[0] == 0.0 && returned[1] == 1.0 && passed[0] == 0.0 && passed[1] == 1.0))
"#
    );
    assert_context_output(&source, 1.0);
}

#[test]
fn tuple_binding_defaults_agree_with_scalar_arithmetic_and_overloads() {
    for (assignment, value) in [
        ("pair = (Limit, Float)", "pair[0]"),
        ("value, float = (Limit, Float)", "value"),
        ("value, _ = (Limit, Float)", "value"),
        ("_, value = (Float, Limit)", "value"),
    ] {
        let source = format!(
            "const Limit = 2147483647\nconst Float = 16777217.0\ndef next(value: i32) -> i32:\n  return value + 1\ndef next(value: i64) -> i64:\n  return value + 1\ndef run() -> bool:\n  {assignment}\n  return {value} + 1 == next({value}) && {value} + 1 == -2147483648\nsample:\n  out1 = f32(run())\n"
        );
        assert_context_output(&source, 1.0);
    }
}

#[test]
fn tuple_binding_defaults_and_concrete_types_survive_materialization() {
    for (assignment, integer, large, float, fixed) in [
        (
            "pair = (Limit, Large, Float, Fixed)",
            "pair[0]",
            "pair[1]",
            "pair[2]",
            "pair[3]",
        ),
        (
            "integer, large, float, fixed = (Limit, Large, Float, Fixed)",
            "integer",
            "large",
            "float",
            "fixed",
        ),
    ] {
        let body = format!(
            "{assignment}\n  result = {integer} + 1 == -2147483648 && {large} == i64(4294967297) && {float} - 16777216.0 == 0.0 && {fixed} - 16777216.0 == 1.0"
        );
        for scope in [
            format!(
                "def run() -> bool:\n  {body}\n  return result\nsample:\n  out1 = f32(run())\n"
            ),
            format!("sample:\n  {body}\n  out1 = f32(result)\n"),
            format!("init:\n  {body}\nsample:\n  out1 = f32(result)\n"),
            format!(
                "sample:\n  out1 = 0.0\n  if true:\n    {}\n    out1 = f32(result)\n",
                body.replace('\n', "\n  ")
            ),
        ] {
            let source = format!(
                "const Limit = 2147483647\nconst Large = 4294967297\nconst Float = 16777217.0\nconst Fixed = f64(Float)\n{scope}"
            );
            assert_context_output(&source, 1.0);
        }
    }
}

#[test]
fn tuple_destructuring_selects_context_independently_for_each_destination() {
    for (targets, values) in [
        ("narrow, _", "Precise - 16777216.0, 0"),
        ("narrow, fresh", "Precise - 16777216.0, Fixed - 16777216.0"),
        ("_, narrow", "0, Precise - 16777216.0"),
        ("fresh, narrow", "Fixed - 16777216.0, Precise - 16777216.0"),
    ] {
        for scope in [
            format!("def run() -> f32:\n  narrow: f32 = -1\n  {targets} = ({values})\n  return narrow\nsample:\n  out1 = run()\n"),
            format!("sample:\n  narrow: f32 = -1\n  {targets} = ({values})\n  out1 = narrow\n"),
            format!("init:\n  narrow: f32 = -1\n  {targets} = ({values})\nsample:\n  out1 = narrow\n"),
        ] {
            let source = format!("const Precise = 16777217.0\nconst Fixed: f64 = Precise\n{scope}");
            assert_context_output(&source, 0.0);
        }
    }
}

#[test]
fn tuple_destructuring_preserves_order_and_discarded_component_effects() {
    assert_context_output(
        "def advance(values: i32[1]) -> i64:\n  values[0] = values[0] + 1\n  return i64(values[0])\ndef run() -> i32:\n  values: i32[1] = [0]\n  first: i64 = 0\n  second: i64 = 0\n  first, _, second = (advance(values), advance(values), advance(values))\n  return i32(first * 100 + values[0] * 10 + second)\nsample:\n  out1 = f32(run())\n",
        133.0,
    );
}

#[test]
fn owner_proc_calls_in_assignment_selectors_retain_block_hooks() {
    let source = r#"
proc Counter:
  init:
    count: i32 = 0
  block:
    count += 1000
    sample:
      count += 1
      out1 = f32((count >= 1000))
proc Bank:
  init:
    counters: Counter[1] = Counter()
  sample:
    out1 = 0.0
def select(owner: Bank) -> i32:
  return i32(owner.counters[0]())
def mark(owner: Bank, values: f32[64]):
  values[select(owner)] = 1.0
init:
  counter = Bank()
  values: f32[64]
sample:
  mark(counter, values)
  out1 = values[1]
"#;
    assert_context_output(source, 1.0);
}

#[test]
fn const_and_runtime_array_aliases_observe_the_same_mutations() {
    let cases = [
        ("ys = xs\n  xs[0] = 10\n  return ys[0]", 10),
        ("ys = xs[:]\n  xs[0] = 10\n  return ys[0]", 10),
        ("ys = xs\n  ys[0] = 10\n  return xs[0]", 10),
        (
            "ys = xs[1:]\n  zs = ys[:1]\n  zs[0] = 10\n  return xs[1]",
            10,
        ),
        ("ys: i32[2] = xs\n  xs[0] = 10\n  return ys[0]", 7),
        (
            "ys = xs[:]\n  zs: i32[2] = ys\n  ys[0] = 10\n  return zs[0]",
            7,
        ),
        ("ys = xs\n  xs = [10, 11]\n  return ys[0]", 10),
        ("ys = xs\n  xs = ys\n  ys[0] = 10\n  return xs[0]", 10),
        ("ys = xs[:]\n  if true:\n    ys[0] = 10\n  return xs[0]", 10),
        (
            "ys = xs[:]\n  for i in 0..2:\n    zs = ys\n    zs[i] += 1\n  return xs[0] + xs[1]",
            18,
        ),
    ];
    for (body, expected) in cases {
        let source = format!(
            "const def folded() -> i32:\n  xs: i32[2] = [7, 9]\n  {body}\ndef ordinary() -> i32:\n  xs: i32[2] = [7, 9]\n  {body}\nsample:\n  out1 = f32(folded())\n  out2 = f32(ordinary())\n"
        );
        for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            let (mut instance, _, _) = compile_instance_with_options(
                &source,
                16,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 16,
                    fast_math: false,
                    opt_level,
                },
            );
            let mut output = [0.0; 32];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            assert!(
                output.iter().all(|&value| value == expected as f32),
                "{source}\n{output:?}"
            );
        }
    }
}

#[test]
fn const_and_runtime_array_parameters_share_storage_and_returns_capture_values() {
    for parameter in ["i32[2]", "i32[]", "[]"] {
        let body = "  xs: i32[2] = [7, 9]\n  copy = identity(xs)\n  changed = change(xs)\n  return xs[0] + changed + copy[0]\n";
        let source = format!(
            "const def identity(xs: i32[2]) -> i32[2]:\n  return xs\nconst def change(xs: {parameter}) -> i32:\n  ys = xs\n  ys[0] = 10\n  return xs[0]\nconst def folded() -> i32:\n{body}def runtime_identity(xs: i32[2]) -> i32[2]:\n  return xs\ndef runtime_change(xs: {parameter}) -> i32:\n  ys = xs\n  ys[0] = 10\n  return xs[0]\ndef ordinary() -> i32:\n{}sample:\n  out1 = f32(folded() + ordinary())\n",
            body.replace("identity(xs)", "runtime_identity(xs)").replace("change(xs)", "runtime_change(xs)")
        );
        assert_context_output(&source, 54.0);
    }
}

#[test]
fn const_fixed_parameters_replace_sliced_storage_without_redirecting_it() {
    let source = "const def replace(xs: i32[2]) -> i32:\n  alias = xs\n  xs = [10, 11]\n  return alias[0]\nconst def read() -> i32:\n  xs: i32[3] = [7, 8, 9]\n  result = replace(xs[1:])\n  return result + xs[0] + xs[1] + xs[2]\nsample:\n  out1 = f32(read())\n";
    assert_context_output(source, 38.0);
}

#[test]
fn const_fixed_local_replacement_preserves_aliases_with_a_dependent_dimension() {
    let source = "const def read(n: i32) -> i32:\n  xs: i32[n]\n  alias = xs\n  xs = [7, 9]\n  alias[0] = 10\n  return xs[0] + xs[1]\nsample:\n  out1 = f32(read(2))\n";
    assert_context_output(source, 19.0);
}

#[test]
fn const_def_shadowed_assignments_execute_with_lexical_bindings() {
    let source = r#"
namespace Library<N = 2>:
  const Table: i32[2] = [1 / 0, 0]
  const def scalar(Table: i32) -> i32:
    Table += 1
    return Table
  const def array(Table: i32[2]) -> i32:
    Table[0] = 7
    return Table[0]
  const def local() -> i32:
    Table: i32[2] = [0, 0]
    Table[0] = 7
    return Table[0]
sample:
  out1 = f32(Library<3>::scalar(6) + Library<3>::array([0, 0]) + Library<3>::local())
"#;
    assert_context_output(source, 21.0);
}

#[test]
fn inferred_return_types_retain_checked_tuple_selectors() {
    let source = r#"
def read():
  values = (i32(7), f32(9.5))
  return values[i32(SR / 48000) - 1]
def choose(value: i32) -> f32:
  return 1.0
def choose(value: f32) -> f32:
  return 2.0
init:
  initial = choose(read())
sample 2:
  out1 = initial + choose(read())
"#;
    assert_context_output(source, 2.0);
}

#[test]
fn runtime_defaults_retain_checked_tuple_selectors() {
    let source = r#"
def read() -> f32:
  values = (i32(7), f32(9.5))
  selected = values[(i32(SR / 48000) - 1)]
  return f32(selected)
def wrapper(value: f32 = read()) -> f32:
  return value
init:
  initial = wrapper()
sample 2:
  out1 = initial + wrapper()
"#;
    assert_context_output(source, 14.0);
}

#[test]
fn generic_methods_retain_checked_tuple_types_and_slice_extents() {
    let source = r#"
struct Box<T>:
  value: T
  def read(self) -> f32:
    values = (i32(7), self.value)
    selected = values[(i32(SR / 48000) - 1)]
    array: i32[2] = [7, 9]
    view = array[:(i32(SR / 48000))]
    copy: i32[(i32(SR / 48000))] = view
    return f32(selected) + f32(copy[copy.len() - 1] + view.len() * 10)
def wrapper<T>(value: T) -> f32:
  box = Box<T>(value = value)
  return box.read()
init:
  initial = wrapper<f32>(9.5)
sample 2:
  out1 = initial + wrapper<f32>(9.5)
"#;
    assert_context_output(source, 48.0);
}

#[test]
fn checked_tuple_selectors_preserve_bindings_and_overloads() {
    for generic in [false, true] {
        for binding in [false, true] {
            let declaration = if generic {
                "read<T>(value: T)"
            } else {
                "read(value: f32)"
            };
            let call = if generic {
                "read<f32>(9.5)"
            } else {
                "read(9.5)"
            };
            let selection = if binding {
                "  selected = values[(i32(SR / 48000) - 1)]\n  return choose(selected)"
            } else {
                "  return choose(values[(i32(SR / 48000) - 1)])"
            };
            let overload = if generic {
                ""
            } else {
                "def read(value: i32) -> f32:\n  return -1000.0\n"
            };
            let source = format!(
                r#"
{overload}
def choose(value: i32) -> f32:
  return f32(value) + 100.0
def choose(value: f32) -> f32:
  return value
def {declaration} -> f32:
  values = (i32(7), value)
{selection}
def relay() -> f32:
  return {call}
init:
  initial = relay()
sample 2:
  out1 = initial + relay()
"#
            );
            assert_context_output(&source, 214.0);
        }
    }
}

#[test]
fn checked_slice_extents_preserve_fixed_array_copies() {
    for generic in [false, true] {
        let declaration = if generic { "read<T>()" } else { "read()" };
        let call = if generic { "read<i32>()" } else { "read()" };
        let source = format!(
            r#"
def {declaration} -> i32:
  values: i32[2] = [7, 9]
  view = values[:(i32(SR / 48000))]
  copy: i32[(i32(SR / 48000))] = view
  return i32(copy[copy.len() - 1]) + view.len() * 10
def relay() -> i32:
  return {call}
init:
  initial = relay()
sample 2:
  out1 = f32(initial + relay())
"#
        );
        assert_context_output(&source, 34.0);
    }
}

#[test]
fn forwarded_processor_boundaries_keep_declaration_layouts_and_effective_rates() {
    let source = r#"
def dimension() -> i32:
  values: i32[(i32(SR / 48000))]
  return values.len() + (i32(SR / 48000)) * 10
buffers:
  memory: i32[1]
proc Voice:
  block:
    sample:
      out1 = 0.0
proc Bank:
  buffers:
    shared: i32[1]
  init:
    voices: Voice[2] = Voice()
  block:
    shared[0] = dimension()
    sample:
      out1 = 0.0
def inner(bank: Bank):
  return bank.voices[0]()
init:
  bank = Bank(shared = memory)
sample 2:
  out1 = inner(bank)
"#;
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level,
            },
        );
        let mut memory = [0_i32];
        bind_buffer(
            &mut instance,
            0,
            memory.as_mut_ptr().cast::<u8>(),
            1,
            1,
            48_000.0,
            PrimitiveType::I32,
        )
        .unwrap();
        let mut output = [0.0; 16];
        for _ in 0..32 {
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        }
        assert_eq!(memory[0], 21);
    }
}

#[test]
fn surviving_processor_hooks_preserve_instance_selection_and_boundary_order() {
    let source = r#"
const Values: i32[1] = [7]
buffers:
  memory: i32[1]
proc Voice:
  buffers:
    shared: i32[1]
  init:
    value: i32 = 0
  block:
    value += Values[0]
    sample:
      out1 = f32(value)
    value += 10
    shared[0] = value
proc Bank:
  buffers:
    shared: i32[1]
  init:
    voice = Voice(shared = shared)
    value: i32 = 0
  block:
    value += 100
    sample:
      out1 = voice() + f32(value)
    value = shared[0]
init:
  live = Bank(shared = memory)
  dead = Bank(shared = memory)
sample:
  if false:
    out1 = dead()
  out1 = live()
"#;
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level,
            },
        );
        let mut memory = [0_i32];
        bind_buffer(
            &mut instance,
            0,
            memory.as_mut_ptr().cast::<u8>(),
            1,
            1,
            48_000.0,
            PrimitiveType::I32,
        )
        .unwrap();
        let mut output = [0.0; 16];
        for expected in [107.0, 141.0] {
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            assert!(
                output.iter().all(|&sample| sample == expected),
                "{output:?}"
            );
        }
    }
}

#[test]
fn nested_namespace_const_specializations_share_runtime_data_and_values() {
    let declarations = "namespace Source<N = 2>:\n  const def build() -> i32[N]:\n    values: i32[N]\n    for i in 0..N:\n      values[i] = N + i\n    return values\n  const Data = build()\n  const Value: i32 = N\n  const def read() -> i32:\n    return Value\nnamespace Wrapper<P = 2>:\n  const Alias: i32[] = Source<P>::Data\n  const Value: i32 = Source<P>::read()\nnamespace Chain<R = 2>:\n  const Alias: i32[] = Wrapper<R>::Alias\nparams:\n  index: i32 = 1\n";
    for (expression, expected) in [
        ("Wrapper<7>::Alias[index] + Source<7>::Data[index]", 16.0),
        ("Source<7>::Data[index] + Wrapper<7>::Alias[index]", 16.0),
        ("Chain<7>::Alias[index] + Source<7>::Data[index]", 16.0),
        ("Source<7>::Data[index] + Chain<7>::Alias[index]", 16.0),
        ("Wrapper<7>::Alias[index] + Wrapper<8>::Alias[index]", 17.0),
        ("Source<7>::read() + Wrapper<7>::Value", 14.0),
        ("Wrapper<7>::Value + Source<7>::read()", 14.0),
    ] {
        let source = format!("{declarations}sample:\n  out1 = f32({expression})\n");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == expected),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn const_array_arguments_preserve_snapshots_and_copy_on_write() {
    let source = "const Table: i32[3] = [7, 9, 11]\nconst def first(xs: i32[]) -> i32:\n  return xs[0]\nconst def change(source: i32[2]) -> i32:\n  xs: i32[2] = source\n  original: i32 = xs[0]\n  copy: i32[2] = xs\n  xs[0] = 99\n  copy[1] = 42\n  return original + first(copy) + xs[0] + copy[1]\nconst def forward(xs: i32[]) -> i32:\n  return change(xs[1:])\nsample:\n  out1 = f32(forward(Table) + Table[0] + Table[1] + Table[2])\n";
    let (mut instance, _, _) = compile_instance_with_options(
        source,
        16,
        CompileOptions {
            sample_rate: 48_000.0,
            block_size: 16,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );
    let mut output = [0.0; 16];
    process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
    assert!(output.iter().all(|&sample| sample == 186.0), "{output:?}");
}

#[test]
fn const_and_runtime_slice_bounds_use_the_same_i32_conversion() {
    for (slice, len, first) in [
        ("4294967296:", 3, 7),
        (":4294967297", 1, 7),
        ("-4294967295:", 2, 9),
        (":4294967295", 2, 7),
        (":4294967297.0", 3, 7),
        ("-4294967295.0:", 3, 7),
        ("i64(4294967297):", 2, 9),
        (":f64(4294967297.0)", 3, 7),
    ] {
        let source = format!(
            "const Table: i32[3] = [7, 9, 11]\nconst Whole = Table[{slice}]\nconst def size(xs: i32[]) -> i32:\n  return xs.len()\nsample:\n  view = Table[{slice}]\n  copy: i32[{len}] = Table[{slice}]\n  out1 = f32(Whole[0] + Whole.len() + view[0] + view.len() + copy[0] + copy.len() + size(Table[{slice}]))\n"
        );
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output
                .iter()
                .all(|&sample| sample == (3 * first + 4 * len) as f32),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn runtime_const_slice_defaults_keep_namespace_specialization_and_scope() {
    let source = "namespace Library<Size = 2>:\n  const def build() -> i32[Size]:\n    values: i32[Size]\n    for i in 0..Size:\n      values[i] = i + Size\n    return values\n  const Table = build()\n  def read(xs: i32[Size] = Table[:]) -> i32:\n    return xs[0] + xs.len()\nconst Table: i32[1] = [99]\nsample:\n  out1 = f32(Library<2>::read() + Library<3>::read())\n";
    let (mut instance, _, _) = compile_instance_with_options(
        source,
        16,
        CompileOptions {
            sample_rate: 48_000.0,
            block_size: 16,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );
    let mut output = [0.0; 16];
    process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
    assert!(output.iter().all(|&sample| sample == 10.0), "{output:?}");
}

#[test]
fn runtime_fixed_array_defaults_defer_generic_element_types() {
    let source = "def read<T>(xs: T[2] = [T(7), T(9)]) -> T:\n  return xs[0]\nsample:\n  out1 = read<f32>() + f32(read<i32>())\n";
    let (mut instance, _, _) = compile_instance_with_options(
        source,
        16,
        CompileOptions {
            sample_rate: 48_000.0,
            block_size: 16,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );
    let mut output = [0.0; 16];
    process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
    assert!(output.iter().all(|&sample| sample == 14.0), "{output:?}");
}

#[test]
fn generic_array_default_initializers_use_concrete_types() {
    let prefix = "const def build(value: i32) -> i32[2]:\n  return [value, value + 1]\n";
    for (default, expected) in [("build(T(7))", 15.0), ("build(i32(T(7)))", 15.0)] {
        let source = format!(
            "{prefix}def read<T>(values: T[2] = {default}) -> T:\n  return values[0] + values[1]\nsample:\n  out1 = f32(read<i32>())\n"
        );
        assert_context_output(&source, expected);
        assert_context_output(
            &source.replace("read<i32>()", "read<i32>([i32(9), i32(10)])"),
            19.0,
        );
        let source = format!(
            "{prefix}proc Voice<T>:\n  params:\n    values: T[2] = {default}\n  sample:\n    out1 = f32(values[0] + values[1])\ninit:\n  voice = Voice<i32>()\nsample:\n  out1 = voice()\n"
        );
        assert_context_output(&source, expected);
        assert_context_output(
            &source.replace("Voice<i32>()", "Voice<i32>(values = [i32(9), i32(10)])"),
            19.0,
        );
    }
}

#[test]
fn const_and_runtime_array_replacements_preserve_checked_shapes() {
    for prefix in ["", "const "] {
        for size in [2, 3] {
            let source = format!(
                "{prefix}def make() -> i32[{size}]:\n  values: i32[{size}]\n  for i in 0..{size}:\n    values[i] = {size}\n  return values\n{prefix}def read() -> i32:\n  values = make()\n  values = make()\n  values[0] = 7\n  scalar: i32 = 1\n  scalar = values[0] + values.len()\n  return scalar\nsample:\n  out1 = f32(read())\n"
            );
            let (mut instance, _, _) = compile_instance_with_options(
                &source,
                16,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 16,
                    fast_math: false,
                    opt_level: TargetOptLevel::O3,
                },
            );
            let mut output = [0.0; 16];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            assert!(
                output.iter().all(|&sample| sample == (7 + size) as f32),
                "{source}\n{output:?}"
            );
        }
    }
}

#[test]
fn inferred_const_array_bindings_match_native_types_and_values() {
    let declarations =
        "const Table: i32[3] = [7, 9, 11]\nconst def build() -> i32[2]:\n  return [7, 9]\n";
    for (initializer, result, expected) in [
        ("[7, 9]", "values[0] + values.len()", 9.0),
        ("Table", "values[0] + values.len()", 10.0),
        ("Table[1:]", "values[0] + values.len()", 11.0),
        ("build()", "values[0] + values.len()", 9.0),
        ("[4294967297, 9]", "i32(values[0] - 4294967296)", 1.0),
        ("[16777217.0]", "i32(values[0] - 16777216.0)", 0.0),
        ("[f64(16777217.0)]", "i32(values[0] - f64(16777216.0))", 1.0),
        ("[true, false]", "i32(values[0]) + values.len()", 3.0),
    ] {
        let source = format!(
            "{declarations}const def folded() -> i32:\n  values = {initializer}\n  return {result}\ndef native() -> i32:\n  values = {initializer}\n  return {result}\nsample:\n  out1 = f32(folded())\n  out2 = f32(native())\n"
        );
        for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            let (mut instance, _, _) = compile_instance_with_options(
                &source,
                16,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 16,
                    fast_math: false,
                    opt_level,
                },
            );
            let mut output = [0.0; 32];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            assert!(
                output.iter().all(|&value| value == expected),
                "{source}\n{output:?}"
            );
        }
    }
}

#[test]
fn explicit_const_array_copies_preserve_independent_values() {
    let source = "const Table: i32[2] = [7, 9]\nconst def change(xs: i32[2]) -> i32:\n  copy: i32[2] = xs\n  copy[0] = 11\n  from_const: i32[2] = Table\n  from_const[0] = 13\n  return xs[0] + copy[0] + from_const[0] + Table[0]\nsample:\n  out1 = f32(change(Table))\n";
    assert_context_output(source, 38.0);
}

#[test]
fn const_dependent_array_shapes_preserve_values_and_dead_dimensions() {
    for initializer in ["[7, 9]", "Table", "Table[:]", "build()"] {
        let source = format!(
            "const Table: i32[2] = [7, 9]\nconst def build() -> i32[2]:\n  return Table\nconst def copy(n: i32) -> i32:\n  values: i32[n] = {initializer}\n  return values[0] + values.len()\nconst def joined(n: i32, enabled: bool) -> i32:\n  if enabled:\n    values: i32[n]\n  else:\n    values: i32[2] = [7, 9]\n  values[0] += values.len()\n  return values[0]\nsample:\n  out1 = f32(joined(2, true) + joined(0, false) + copy(2))\n"
        );
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 20.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn const_struct_ranges_normalize_defaults_overrides_and_field_writes() {
    for (domain, expected) in [
        ("{count()}", 630.0),
        ("{count(), wrap}", 331.0),
        ("{Bounds[0]..count()}", 628.0),
        ("{Bounds[0]..count(), wrap}", 89.0),
        ("{Bounds[0]..=count()}", 838.0),
        ("{Bounds[0]..=count(), wrap}", 540.0),
    ] {
        let source = format!(
            "const Bounds: i32[2] = [-2, 4]\nconst Broken: i32[1] = [1 / 0]\nconst def count() -> i32:\n  return Bounds[1]\nstruct Defaulted:\n  value: i32 = 10 {domain}\nstruct Overridden:\n  value: i32 = Broken[0] {domain}\nsample:\n  defaulted = Defaulted()\n  overridden = Overridden(value = 9)\n  before = defaulted.value + overridden.value\n  defaulted.value = 11\n  overridden.value = -7\n  out1 = f32(before * 100 + defaulted.value * 10 + overridden.value)\n"
        );
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == expected),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn constant_branches_preserve_checked_widening_at_the_join() {
    let cases = [
        "sample:\n  if true:\n    x = f32(16777216.0)\n  else:\n    x = f64(Broken[0])\n  out1 = f32((x + 1.0) - 16777216.0)\n",
        "init:\n  result: f32 = 0.0\n  if true:\n    pair = (f32(16777216.0), i32(1))\n  else:\n    pair = (f64(Broken[0]), i64(0))\n  result = f32((pair[0] + 1.0) - 16777216.0)\nsample:\n  out1 = result\n",
        "block:\n  if true:\n    pair = (f32(16777216.0), i32(1))\n  else:\n    pair = (f64(Broken[0]), i64(0))\n  sample:\n    out1 = f32((pair[0] + 1.0) - 16777216.0)\n",
        "sample:\n  if false:\n    pair = (f64(Broken[0]), i64(0))\n  else:\n    pair = (f32(16777216.0), i32(1))\n  out1 = f32((pair[0] + 1.0) - 16777216.0)\n",
        "sample:\n  if true:\n    (x, y) = (f32(16777216.0), i32(1))\n  else:\n    (x, y) = (f64(Broken[0]), i64(0))\n  out1 = f32((x + 1.0) - 16777216.0)\n",
        "def f() -> f32:\n  if true:\n    pair = (f32(16777216.0), i32(1))\n  else:\n    pair = (f64(Broken[0]), i64(0))\n  return f32((pair[0] + 1.0) - 16777216.0)\nsample:\n  out1 = f()\n",
        "def f() -> f32:\n  if false:\n    x = f64(Broken[0])\n  else:\n    x = f32(16777216.0)\n    x += f32(1.0)\n  return f32((x + 1.0) - 16777216.0)\nsample:\n  out1 = f()\n",
        "def f() -> f32:\n  if false:\n    x = f64(Broken[0])\n  x = f32(16777216.0)\n  x += f32(1.0)\n  return f32((f64(x) + 1.0) - 16777216.0)\nsample:\n  out1 = f()\n",
        "def f() -> f32:\n  for i in 0..0:\n    x = f64(Broken[0])\n  x = f32(16777216.0)\n  x += f32(1.0)\n  return f32((f64(x) + 1.0) - 16777216.0)\nsample:\n  out1 = f()\n",
        "sample:\n  if true:\n    if false:\n      x = f64(Broken[0])\n    else:\n      x = f32(16777216.0)\n  else:\n    x = f64(Broken[0])\n  out1 = f32((x + 1.0) - 16777216.0)\n",
        "sample:\n  if true:\n    pair = (f32(16777216.0), i32(1))\n    pair = (pair[0] + f32(1.0), pair[1])\n  else:\n    pair = (f64(Broken[0]), i64(0))\n  out1 = f32((pair[0] + 1.0) - 16777216.0)\n",
        "namespace N<V = 16777216>:\n  def f() -> f32:\n    if true:\n      pair = (f32(V), i32(1))\n    else:\n      pair = (f64(Broken[0]), i64(0))\n    return f32((pair[0] + 1.0) - f64(V))\nsample:\n  out1 = N<16777216>::f()\n",
    ];
    for body in cases {
        let source = format!("const Broken: i32[1] = [0]\n{body}");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 1.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn constant_event_branches_preserve_checked_tuple_types() {
    let source = "const Broken: i32[1] = [0]\ninit:\n  result: f32 = 0.0\nevent trigger():\n  if true:\n    pair = (f32(16777216.0), i32(1))\n  else:\n    pair = (f64(Broken[0]), i64(0))\n  result = f32((pair[0] + 1.0) - 16777216.0)\nsample:\n  out1 = result\n";
    let (mut instance, _, _) = compile_instance_with_options(
        source,
        16,
        CompileOptions {
            sample_rate: 48_000.0,
            block_size: 16,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );
    let index = instance.event_index("trigger").unwrap();
    trigger_event_by_index_checked(
        &mut instance,
        index,
        &[],
        onda_runtime::ExecutionOutput::none(),
    )
    .unwrap();
    let mut output = [0.0; 16];
    process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
    assert!(output.iter().all(|&sample| sample == 1.0), "{output:?}");
}

#[test]
fn const_def_branch_scopes_match_runtime_scopes() {
    for body in [
        "if enabled:\n  values: i32[1] = [7]\nvalues: i32[2] = [1, 2]\nreturn values[1]\n",
        "if enabled:\n  value: i32[1] = [7]\nvalue: i32 = 9\nreturn value\n",
        "if enabled:\n  value: i32 = 7\nvalue: i32[2] = [1, 2]\nreturn value[1]\n",
        "total: i32 = 0\nvalues: i32[2] = [1, 2]\nif enabled:\n  total += values[1]\n  values[0] = 3\n  hidden: i32[1] = [7]\nelse:\n  total += values[0]\nreturn total + values[0]\n",
        "if enabled:\n  values: i32[2] = [1, 2]\nelse:\n  values: i32[2] = [3, 4]\nreturn values[1]\n",
        "if enabled:\n  return 7\nelse:\n  values: i32[2] = [1, 2]\nreturn values[1]\n",
        "if enabled:\n  if enabled:\n    values: i32[1] = [7]\n  values: i32 = 9\n  return values\nreturn 3\n",
        "total: i32 = 0\nfor i in 0..3:\n  if enabled:\n    values: i32[1] = [7]\n  values: i32[2] = [1, 2]\n  total += values[1]\nreturn total\n",
        "total: i32 = 0\nfor i in 0..3:\n  if enabled:\n    values: i32 = 7\n  values: i32[2] = [1, 2]\n  total += values[1]\nreturn total\n",
    ] {
        let body = body.lines().map(|line| format!("  {line}\n")).collect::<String>();
        let source = format!("const Table: i32[1] = [7]\nconst def folded(enabled: bool) -> i32:\n{body}def native(enabled: bool) -> i32:\n{body}sample:\n  out1 = f32(folded(true) - native(true) + folded(false) - native(false))\n");
        let (mut instance, _, _) = compile_instance_with_options(&source, 16, CompileOptions {
            sample_rate: 48_000.0, block_size: 16, fast_math: false, opt_level: TargetOptLevel::O3,
        });
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(output.iter().all(|&sample| sample == 0.0), "{source}\n{output:?}");
    }
}

#[test]
fn constant_branches_discard_checked_local_bindings() {
    for (body, expression, expected) in [
        (
            "    values: i32[1] = [7]\n  values: i32[2] = [1, 2]\n",
            "f32(values[1])",
            2.0,
        ),
        (
            "    value: i32[1] = [7]\n  value: i32 = 9\n",
            "f32(value)",
            9.0,
        ),
        (
            "    value: i32 = 7\n  value: i32[2] = [1, 2]\n",
            "f32(value[1])",
            2.0,
        ),
        (
            "    pair = (i32(7), i32(9))\n  pair = (i32(1), i32(2), i32(3))\n",
            "f32(pair[2])",
            3.0,
        ),
        (
            "    value: f64 = 16777216.0\n  value: f32 = 16777216.0\n  value += f32(1.0)\n",
            "f32((f64(value) + 1.0) - 16777216.0)",
            1.0,
        ),
    ] {
        for (condition, take_else) in [
            ("true", false),
            ("false", true),
            ("enabled || true", false),
            ("enabled && false", true),
        ] {
            for in_def in [false, true] {
                let (branch, following) = body.split_once('\n').unwrap();
                let dead = "    unused: i32 = Broken[0]";
                let (then_branch, else_branch) = if take_else {
                    (dead, branch)
                } else {
                    (branch, dead)
                };
                let statements = format!(
                    "  if {condition}:\n{then_branch}\n  else:\n{else_branch}\n{following}"
                );
                let executable = if in_def {
                    format!("def read(enabled: bool) -> f32:\n{statements}  return {expression}\nsample:\n  out1 = read(enabled)\n")
                } else {
                    format!("sample:\n{statements}  out1 = {expression}\n")
                };
                let source = format!(
                    "const Broken: i32[1] = [0]\nparams:\n  enabled: bool = false\n{executable}"
                );
                let (mut instance, _, _) = compile_instance_with_options(
                    &source,
                    16,
                    CompileOptions {
                        sample_rate: 48_000.0,
                        block_size: 16,
                        fast_math: false,
                        opt_level: TargetOptLevel::O3,
                    },
                );
                let mut output = [0.0; 16];
                process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
                assert!(
                    output.iter().all(|&sample| sample == expected),
                    "{source}\n{output:?}"
                );
            }
        }
    }
}

#[test]
fn constant_branches_discard_aggregate_alias_leaves() {
    for condition in ["true", "enabled || true"] {
        let source = format!("struct Wide:\n  value: f64\n  values: i32[1]\nstruct Narrow:\n  value: f32\n  values: i32[2]\nparams:\n  enabled: bool = false\ninit:\n  wide: Wide[1] = [Wide(value = 16777216.0, values = [7])]\n  narrow: Narrow[1] = [Narrow(value = 16777216.0, values = [1, 2])]\nsample:\n  if {condition}:\n    item = wide[0]\n  item = narrow[0]\n  item.value += f32(1.0)\n  out1 = f32((f64(item.value) + 1.0) - 16777216.0) + f32(item.values[1])\n");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 3.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn const_float_extrema_match_native_nan_and_zero_semantics() {
    for ty in ["f32", "f64"] {
        for (op, lhs, rhs, predicate, expected) in [
            ("min", "sqrt(-1.0)", "1.0", "value == 1.0", false),
            ("min", "1.0", "sqrt(-1.0)", "value == 1.0", false),
            ("max", "sqrt(-1.0)", "1.0", "value == 1.0", false),
            ("max", "1.0", "sqrt(-1.0)", "value == 1.0", false),
            ("min", "0.0", "-0.0", "1.0 / value > 0.0", false),
            ("min", "-0.0", "0.0", "1.0 / value > 0.0", false),
            ("max", "0.0", "-0.0", "1.0 / value > 0.0", true),
            ("max", "-0.0", "0.0", "1.0 / value > 0.0", true),
        ] {
            let expr = format!("{op}({ty}({lhs}), {ty}({rhs}))");
            let condition = predicate.replace("value", &expr);
            let source = format!(
                "const Broken: i32[1] = [0]\nconst Scalar: {ty} = {expr}\nconst Array: {ty}[1] = [{expr}]\nconst def folded() -> {ty}:\n  return {expr}\ndef native(lhs: {ty}, rhs: {ty}) -> {ty}:\n  return {op}(lhs, rhs)\nsample:\n  result = f32(({})) + f32(({})) + f32(({})) + f32(({}))\n  if {condition}:\n    {}\n  else:\n    {}\n  out1 = result\n",
                predicate.replace("value", "Scalar"),
                predicate.replace("value", "Array[0]"),
                predicate.replace("value", "folded()"),
                predicate.replace("value", &format!("native({ty}({lhs}), {ty}({rhs}))")),
                if expected { "result += 4.0" } else { "result += f32(Broken[0])" },
                if expected { "result += f32(Broken[0])" } else { "result += 4.0" },
            );
            let (mut instance, _, _) = compile_instance_with_options(
                &source,
                16,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 16,
                    fast_math: false,
                    opt_level: TargetOptLevel::O3,
                },
            );
            let mut output = [0.0; 16];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            let expected = if expected { 8.0 } else { 4.0 };
            assert!(
                output.iter().all(|&sample| sample == expected),
                "{source}\n{output:?}"
            );
        }
    }
}

#[test]
fn const_float_operations_match_runtime_operand_precision() {
    for expression in [
        "f32(16777216.0) + f32(1.0)",
        "f32(16777216.0) + 1.0",
        "1.0 + f32(16777216.0)",
        "f32(16777217.0 - 16777216.0)",
        "f32(f64(16777217.0) - f64(16777216.0))",
        "f64(f32(16777216.0)) + 1.0",
        "16777216.0 + 1.0",
    ] {
        let source = format!("const Scalar: f64 = {expression}\nconst Array: f64[1] = [{expression}]\nconst def read() -> f64:\n  return {expression}\nconst def defaulted(value: f64 = {expression}) -> f64:\n  return value\nsample:\n  runtime: f64 = {expression}\n  out1 = f32((Scalar - runtime) + (Array[0] - runtime) + (read() - runtime) + (defaulted() - runtime))\n");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 0.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn implicit_const_float_conversions_match_native_integer_casts() {
    for value in [4_611_686_293_305_294_849_i64, -4_611_686_293_305_294_849] {
        let source = format!(
            r#"
const Integer: i64 = {value}
const Scalar: f32 = Integer
const Array: f32[1] = [Integer]
const def result(value: i64) -> f32:
  return value
const def argument(value: f32) -> f32:
  return value
const def const_default(value: f32 = Integer) -> f32:
  return value
def runtime_default(value: f32 = Integer) -> f32:
  return value
params:
  integer: i64 = {value}
  default: f32 = Integer
sample:
  runtime = f32(integer)
  out1 = f32((Scalar == runtime && Array[0] == runtime && result(Integer) == runtime && argument(Integer) == runtime && const_default() == runtime && runtime_default() == runtime && default == runtime))
"#
        );
        assert_context_output(&source, 1.0);
    }
}

#[test]
fn const_def_loop_ranges_match_native_induction_semantics() {
    for range in [
        "i: i32 in 4294967296..1",
        "i: i32 @ -4294967295 in 0..1",
        "i: i32 in 0..4294967296",
        "i: i64 in 1..2",
        "i: i32 in 2147483647..=2147483647",
        "i: i64 in 9223372036854775807..=9223372036854775807",
        "i: i64 @ -9223372036854775808 in 9223372036854775807..=-1",
    ] {
        let body = format!(
            "  result: i64 = 0\n  for {range}:\n    result = i64(i) + 7\n  return result\n"
        );
        let source = format!("const def folded() -> i64:\n{body}def runtime() -> i64:\n{body}sample:\n  out1 = f32(folded() == runtime())\n");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 1.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn const_array_value_copies_and_ordinary_control_flow_preserve_native_results() {
    let cases = [
        (
            "const Table: i32[1] = [0]\nconst Disabled: bool = false\ndef fail() -> i32:\n  return Table[0]\ndef select() -> i32:\n  if Disabled:\n    return fail()\n  else:\n    return 7\n  return fail()\nsample:\n  out1 = f32(select())\n",
            7.0,
        ),
        (
            "const Table: i32[1] = [0]\nsample:\n  value: i32 = 7\n  while false:\n    value = Table[0]\n  for i @ -1 in 0..2:\n    value = Table[0]\n  for i in 0..2:\n    if true:\n      continue\n    else:\n      value = 0\n    value = Table[0]\n  out1 = f32(value)\n",
            7.0,
        ),
        (
            "const Table: i32[2] = [7, 9]\nconst def change(source: i32[2]) -> i32[2]:\n  xs: i32[2] = source\n  xs[0] = 99\n  return xs\nconst Alias: i32[] = Table\nconst Changed = change(Table)\nsample:\n  out1 = f32(Table[0] + Alias[0] + Changed[0])\n",
            113.0,
        ),
        (
            "const def select(xs: i32[2]) -> i32:\n  Selected = xs[0]\n  Copy = Selected\n  return Copy\nsample:\n  out1 = f32(select([7, 9]))\n",
            7.0,
        ),
        (
            "const def select(xs: i32[2]) -> i32:\n  total = 0\n  for i in 0..2:\n    Selected = xs[0]\n    Copy = Selected\n    total += Copy\n  return total + xs[1] + i32(xs.len())\nsample:\n  out1 = f32(select([7, 9]))\n",
            25.0,
        ),
        (
            "namespace N<V = 2>:\n  const def selected() -> i32:\n    return V\n  def read() -> i32:\n    return selected()\n  proc Voice:\n    sample:\n      out1 = f32(selected())\ninit:\n  voice = N<7>::Voice()\nsample:\n  out1 = voice() + f32(N<9>::read() + N<11>::selected())\n",
            27.0,
        ),
    ];
    for (source, expected) in cases {
        for fast_math in [false, true] {
            let (mut instance, _, _) = compile_instance_with_options(
                source,
                16,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 16,
                    fast_math,
                    opt_level: TargetOptLevel::O3,
                },
            );
            let mut output = [0.0; 16];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            assert!(
                output.iter().all(|&sample| sample == expected),
                "{source}\n{output:?}"
            );
        }
    }
}

#[test]
fn boolean_folding_preserves_concrete_arithmetic_in_native_execution() {
    for (condition, expected) in [
        ("(f32(16777216.0) + f32(1.0)) > f32(16777216.0)", false),
        ("(i32(2147483647) + i32(1)) > i32(0)", false),
        ("(i32(2147483647) + 1) > i32(0)", false),
        ("i64(9007199254740993) > i64(9007199254740992)", true),
        ("i64(9007199254740993) == i64(9007199254740992)", false),
        ("i32(i64(9007199254740993)) == i32(1)", true),
        ("f32(16777216.0) == 16777217.0", true),
        ("(i64(9223372036854775807) + 1) < i64(0)", true),
        ("(i32(1) << 32) == i32(1)", true),
    ] {
        let dead = if expected { "else" } else { "then" };
        for expression in [
            condition.to_owned(),
            format!("({condition}) && true"),
            format!("({condition}) || false"),
            format!("!(!({condition}))"),
        ] {
            let source = format!(
                "const Broken: i32[1] = [0]\nconst Selected = {expression}\nconst def choose() -> bool:\n  return {expression}\nsample:\n  result = f32({expression}) + f32(Selected) + f32(choose())\n  if {expression}:\n    {}\n  else:\n    {}\n  out1 = result\n",
                if dead == "then" { "result += f32(Broken[0])" } else { "result += 4.0" },
                if dead == "else" { "result += f32(Broken[0])" } else { "result += 4.0" },
            );
            let (mut instance, _, _) = compile_instance_with_options(
                &source,
                16,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 16,
                    fast_math: false,
                    opt_level: TargetOptLevel::O3,
                },
            );
            let mut output = [0.0; 16];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            let expected = 4.0 + if expected { 3.0 } else { 0.0 };
            assert!(
                output.iter().all(|&sample| sample == expected),
                "{source}\n{output:?}"
            );
        }
    }
}

#[test]
fn const_array_length_queries_preserve_i32_arithmetic_in_native_execution() {
    for declaration in [
        "const Table: i32[1] = [1 / 0]\n",
        "init:\n  Table: i32[1]\n",
    ] {
        let source = format!(
            "{declaration}const def check(values: i32[]) -> bool:\n  return values.len() + 2147483647 > 0\nsample:\n  if Table.len() + 2147483647 > 0:\n    result = 7.0\n  else:\n    result = 9.0\n  out1 = result + f32(check([0]))\n"
        );
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 9.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn known_boolean_outcomes_preserve_runtime_prefix_effects_in_native_execution() {
    for body in [
        "result = f32(tick(state) && false && fail())\nif !(tick(state) && false):\n  result += 2.0\nelse:\n  result += f32(Broken[0])\nwhile tick(state) && false:\n  result += f32(Broken[0])\nwhile tick(state) || true:\n  if state.count < 5:\n    continue\n  break\nout1 = result + f32(state.count)\n",
        "if tick(state) || true:\n  selected = i32(7)\nelse:\n  selected = i64(4294967297) + Broken[0]\nout1 = f32(selected + 2147483647 == i64(2147483654)) + f32(state.count) + 5.0\n",
        "result = f32(((tick(state) && false) == true) && fail())\nif (tick(state) && false) == true:\n  result += f32(Broken[0])\nelse:\n  result += 2.0\nwhile (tick(state) || true) != true:\n  result += f32(Broken[0])\nwhile (tick(state) || true) == true:\n  if state.count < 5:\n    continue\n  break\nout1 = result + f32(state.count)\n",
        "result = 0.0\nif (tick(state) && false) == (tick(state) || true):\n  result += f32(Broken[0])\nelse:\n  result += 2.0\nwhile ((tick(state) && false) == true) == true:\n  result += f32(Broken[0])\nwhile !((tick(state) && false) == true):\n  if state.count < 5:\n    continue\n  break\nout1 = result + f32(state.count)\n",
        "result = f32(((tick(state) || true) == true) || fail())\nif false != (tick(state) && false):\n  result += f32(Broken[0])\nelse:\n  result += 1.0\nwhile true == (tick(state) && false):\n  result += f32(Broken[0])\nwhile true == (tick(state) || true):\n  if state.count < 5:\n    continue\n  break\nout1 = result + f32(state.count)\n",
    ] {
        let source = format!(
            "const Broken: i32[1] = [0]\nconst def fail() -> bool:\n  return 1 / 0 > 0\nstruct State:\n  count: i32\ndef tick(state: State) -> bool:\n  state.count += 1\n  return state.count < 3\ninit:\n  state = State()\nsample:\n  state.count = 0\n{}",
            body.lines().map(|line| format!("  {line}\n")).collect::<String>()
        );
        for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
            let (mut instance, _, _) = compile_instance_with_options(&source, 16, CompileOptions {
                sample_rate: 48_000.0, block_size: 16, fast_math: false, opt_level,
            });
            let mut output = [0.0; 16];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            assert!(output.iter().all(|&sample| sample == 7.0), "{source}\n{output:?}");
        }
    }
}

#[test]
fn named_array_defaults_remain_lazy_and_preserve_element_values() {
    for len in [1, 2] {
        for value in [
            "Table".to_owned(),
            "build()".to_owned(),
            "Table[0:]".to_owned(),
        ] {
            let source = format!(
                "const def build() -> i32[{len}]:\n  values: i32[{len}]\n  for i in 0..{len}:\n    values[i] = i + 7\n  return values\nconst Table = build()\nproc Voice:\n  params:\n    values: i32[{len}] = {value}\n  ins:\n    in1: i32[{len}] = {value}\n  init:\n    selected: i32 = 0\n  event read(xs: i32[{len}] = {value}):\n    selected = xs[{len} - 1]\n  sample:\n    out1 = f32(values[{len} - 1] + in1[{len} - 1] + selected)\ninit:\n  voice = Voice()\n  voice.read()\nsample:\n  out1 = voice()\n"
            );
            let (mut instance, _, _) = compile_instance_with_options(
                &source,
                16,
                CompileOptions {
                    sample_rate: 48_000.0,
                    block_size: 16,
                    fast_math: false,
                    opt_level: TargetOptLevel::O3,
                },
            );
            let mut output = [0.0; 16];
            process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
            let expected = 3.0 * (len + 6) as f32;
            assert!(
                output.iter().all(|&sample| sample == expected),
                "{source}\n{output:?}"
            );
        }
    }
}

#[test]
fn deferred_integer_constants_preserve_values_and_overloads_in_native_execution() {
    for (value, expected) in [(7_i64, 2.0), (4_294_967_297, 4_294_967_296.0)] {
        let definitions = if value == 7 {
            "def select(value: i32):\n  return 1.0\ndef select(value: i64):\n  return 2.0\n"
        } else {
            "def select(value: i32):\n  return f32(value)\ndef select(value: i64):\n  return f32(value)\n"
        };
        for forced in ["", "params:\n  unused: i32 = i32(Table[0])\n"] {
            for expression in ["Table[0]", "Selected", "Copy[0]", "Alias[0]"] {
                let source = format!("const Table: i64[1] = [{value}]\nconst Selected = Table[0]\nconst Copy = [Table[0]]\nconst Alias = Copy\n{forced}{definitions}proc Voice:\n  sample:\n    out1 = select({expression})\ninit:\n  voice = Voice()\nsample:\n  out1 = voice()\n");
                for fast_math in [false, true] {
                    let (mut instance, _, _) = compile_instance_with_options(
                        &source,
                        48,
                        CompileOptions {
                            sample_rate: 48_000.0,
                            block_size: 48,
                            fast_math,
                            opt_level: TargetOptLevel::O3,
                        },
                    );
                    let mut output = [0.0_f32; 48];
                    process_interleaved(&mut instance, &[], &mut output, 48).unwrap();
                    assert!(
                        output.iter().all(|&sample| sample == expected),
                        "{source}\n{output:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn const_materialization_preserves_concrete_native_types() {
    let cases = [
        ("const def read() -> i64:\n  return 4294967297\nsample:\n  x: i32 = 1\n  out1 = f32(read() - x)\n", 4294967296.0),
        ("const def read() -> f64:\n  return 16777217.0\nsample:\n  x: f32 = 16777216.0\n  out1 = f32(read() - x)\n", 1.0),
        ("const def difference(xs: f64[], x: f32) -> f64:\n  return xs[0] - x\nsample:\n  out1 = f32(difference([16777217.0], f32(16777216.0)))\n", 1.0),
        ("const def difference(x: f64, y: f32) -> f64:\n  return x - y\nsample:\n  out1 = f32(difference(16777217.0, f32(16777216.0)))\n", 1.0),
        ("const def read() -> f64[1]:\n  xs: f64[1] = [16777217.0]\n  return xs\ndef consume(xs: f64[]) -> f64:\n  return xs[0]\nsample:\n  xs = read()\n  out1 = f32(consume(xs) - 16777216.0)\n", 1.0),
        ("const def read() -> f64[1]:\n  xs: f64[1] = [16777217.0]\n  return xs\ndef select(xs: f64[]) -> f32:\n  return 2.0\ndef select(xs: f32[]) -> f32:\n  return 1.0\nsample:\n  xs = read()\n  out1 = select(xs)\n", 2.0),
        ("const def first(xs: []) -> i32:\n  return i32(xs[0])\nconst Table: i64[2] = [7, 9]\nsample:\n  out1 = f32(first([7]) + first(Table) + first(Table[1:]))\n", 23.0),
        ("const Broken: i32[1] = [0]\nsample:\n  if min(i32(0), i32(1)) > i32(0):\n    out1 = f32(Broken[0])\n  if sin(0.0) != 0.0:\n    out1 = f32(Broken[0])\n  out1 = 7.0\n", 7.0),
        ("const Table: f32[3] = [7.0, 9.0, 11.0]\nconst Alias = Table\nparams:\n  index: i32 = 1\nsample:\n  out1 = Table[index] + Alias[index]\n", 18.0),
    ];
    for (source, expected) in cases {
        let (mut instance, _, _) = compile_instance_with_options(
            source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == expected),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn const_builtin_evaluation_matches_concrete_runtime_widths() {
    for (expression, ty) in [
        ("sin(1.0)", "f32"),
        ("sin(f32(1.0))", "f32"),
        ("sin(f64(1.0))", "f64"),
        ("min(4294967297, i32(1))", "i32"),
        ("max(f64(16777217.0), f32(16777216.0))", "f64"),
        ("fma(f32(16777216.0), f32(1.0), f32(1.0))", "f32"),
        ("min(9007199254740993, 9007199254740994)", "i64"),
    ] {
        let source = format!("const Folded: {ty} = {expression}\nconst def read() -> {ty}:\n  return {expression}\nsample:\n  runtime: {ty} = {expression}\n  out1 = f32((Folded == runtime) && (read() == runtime))\n");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 1.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn lazy_const_defaults_and_checked_locals_match_native_execution() {
    for (source, expected) in [
        ("const def folded() -> i32:\n  x: f64 = 16777217.0\n  return i32(x - f64(16777216.0))\ndef native() -> i32:\n  x: f64 = 16777217.0\n  return i32(x - f64(16777216.0))\nsample:\n  out1 = f32(folded() == native())\n", 1.0),
        ("const def folded(enabled: bool) -> i64:\n  if enabled:\n    x = i32(7)\n  else:\n    x = i64(4294967297)\n  return x + 2147483647\ndef native(enabled: bool) -> i64:\n  if enabled:\n    x = i32(7)\n  else:\n    x = i64(4294967297)\n  return x + 2147483647\nsample:\n  out1 = f32(folded(true) == native(true) && folded(false) == native(false))\n", 1.0),
        ("const def make() -> i32[1]:\n  return [7]\nconst def read() -> i32:\n  xs: i32[1] = make()\n  ys: i32[1] = xs\n  ys[0] = 9\n  return xs[0] + ys[0]\nsample:\n  out1 = f32(read())\n", 16.0),
        ("const Broken: i32[1] = [1 / 0]\nstruct Value:\n  value: i32 = Broken[0]\ninit:\n  value = Value(value = 7)\nsample:\n  out1 = f32(value.value)\n", 7.0),
        ("const Broken: i32[1] = [1 / 0]\nproc Value:\n  params:\n    value: i32 = Broken[0]\n  sample:\n    out1 = f32(value)\ninit:\n  value = Value(value = 7)\nsample:\n  out1 = value()\n", 7.0),
        ("const Table: i32[1] = [7]\nstruct Inner:\n  value: i32 = Table[0]\nstruct Outer:\n  inner: Inner\n  values: Inner[2]\ninit:\n  outer = Outer()\nsample:\n  out1 = f32(outer.inner.value + outer.values[1].value)\n", 14.0),
        ("namespace N<Size = 2>:\n  const def make() -> i32[Size]:\n    values: i32[Size]\n    for i in 0..Size:\n      values[i] = Size\n    return values\n  proc Value:\n    params:\n      values: i32[Size] = make()\n    sample:\n      out1 = f32(values[0])\ninit:\n  a = N<2>::Value()\n  b = N<3>::Value()\nsample:\n  out1 = a() + b()\n", 5.0),
        ("const Table: i32[1] = [7]\nproc Value:\n  ins:\n    in1: i32[1] = Table\n  sample:\n    out1 = f32(in1[0])\ninit:\n  value = Value()\nsample:\n  out1 = value()\n", 7.0),
        ("const Table: i32[1] = [7]\nproc Value:\n  init:\n    selected: i32 = 0\n  event read(value: i32 = Table[0]):\n    selected = value\n  sample:\n    out1 = f32(selected)\ninit:\n  value = Value()\n  value.read()\nsample:\n  out1 = value()\n", 7.0),
    ] {
        let (mut instance, _, _) = compile_instance_with_options(source, 16, CompileOptions {
            sample_rate: 48_000.0, block_size: 16, fast_math: false, opt_level: TargetOptLevel::O3,
        });
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(output.iter().all(|&sample| sample == expected), "{source}\n{output:?}");
    }
}

#[test]
fn const_local_numeric_context_matches_native_execution() {
    for body in [
        "  x: i64 = 2147483647\n  y: i32 = 1\n  z = x + y\n  return z - 1\n",
        "  x: i64 = 2147483647\n  y: i32 = 1\n  z = x + y\n  z = z - 1\n  return z\n",
        "  x: f64 = 16777217.0\n  y: f32 = 0.0\n  z = x + y\n  z = z + 1.0\n  return i64(z)\n",
    ] {
        let source = format!("const def folded() -> i64:\n{body}def native() -> i64:\n{body}sample:\n  out1 = f32(folded() == native())\n");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(
            output.iter().all(|&sample| sample == 1.0),
            "{source}\n{output:?}"
        );
    }
}

#[test]
fn deferred_slice_lengths_preserve_native_fixed_copies() {
    for body in [
        "  view = values[Bounds[0]:]\n  copy: f32[3] = view\n",
        "  view: f32[] = values[Bounds[0]:]\n  nested = view[:2]\n  copy: f32[2] = nested\n",
        "  if enabled:\n    view = values[Bounds[0]:]\n  else:\n    view = values[Bounds[1]:]\n  copy: f32[3] = view\n",
    ] {
        let source = format!("const def build() -> i32[2]:\n  return [1, 1]\nconst Bounds = build()\nparams:\n  enabled: bool = false\ninit:\n  values: f32[4] = [1.0, 2.0, 3.0, 4.0]\nsample:\n{body}  out1 = copy[0]\n");
        let (mut instance, _, _) = compile_instance_with_options(
            &source,
            16,
            CompileOptions {
                sample_rate: 48_000.0,
                block_size: 16,
                fast_math: false,
                opt_level: TargetOptLevel::O3,
            },
        );
        let mut output = [0.0; 16];
        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
        assert!(output.iter().all(|&sample| sample == 2.0), "{source}\n{output:?}");
    }
}

#[test]
fn const_calls_execute_transitive_helpers_at_each_effective_rate() {
    let source = r#"
const DeclaredRate: i32 = i32(SR / 48000)
const def high_rate() -> bool:
  return SR > 48000.0
const def rate() -> i32:
  return i32(SR / 48000)
def choose() -> i32:
  if high_rate():
    return 7 + rate() + DeclaredRate
  return 9 + rate() + DeclaredRate
def relay() -> i32:
  return choose()
init:
  initial = relay()
sample 2:
  out1 = f32(initial + relay())
"#;
    for (sample_rate, expected) in [(44_100.0, 17.0), (48_000.0, 21.0)] {
        assert_const_output(source, sample_rate, expected);
    }
    let source = r#"
def choose() -> i32:
  if SR > 48000.0:
    return 7
  return 9
proc Voice:
  sample 2:
    out1 = f32(choose())
init:
  voice = Voice()
sample:
  out1 = voice() + f32(choose())
"#;
    assert_const_output(source, 48_000.0, 16.0);
}

#[test]
fn const_values_follow_each_runtime_context() {
    for initializer in ["i32(SR / 48000)", "rate()"] {
        let source = format!(
            r#"
const DeclaredRate: i32 = i32(SR / 48000)
const def rate() -> i32:
  return i32(SR / 48000)
def read() -> i32:
  Local: i32 = {initializer}
  Alias: i32 = Local
  return Alias + Local + DeclaredRate
init:
  initial = read()
sample 2:
  Local: i32 = {initializer}
  out1 = f32(initial + read() + Local)
"#
        );
        assert_const_output(&source, 48_000.0, 10.0);
    }
    let source = r#"
def read() -> i32:
  Local: i32 = i32(SR / 48000)
  return Local
proc Voice:
  sample 2:
    Local: i32 = i32(SR / 48000)
    out1 = f32(Local + read())
init:
  voice = Voice()
sample:
  out1 = voice() + f32(read())
"#;
    assert_const_output(source, 48_000.0, 5.0);
}

#[test]
fn const_array_defaults_match_literal_and_scalar_defaults() {
    for default in ["build()", "[SR / 48000.0]"] {
        let source = format!(
            r#"
const def build() -> f32[1]:
  return [SR / 48000.0]
def read(values: f32[1] = {default}) -> f32:
  return values[0]
def scalar(value: f32 = SR / 48000.0) -> f32:
  return value
init:
  initial = read() + scalar()
sample 2:
  out1 = initial + read() + scalar()
"#
        );
        assert_const_output(&source, 48_000.0, 6.0);
    }
}

#[test]
fn const_defs_keep_host_rate_aliases_fixed() {
    for alias in [
        "HOST_SR",
        "HOST_SAMPLE_RATE",
        "HOST_SAMPLERATE",
        "host_sample_rate",
        "host_samplerate",
    ] {
        let source = format!(
            r#"
namespace Rates<N = 1>:
  const Table: i32[1] = [1 / 0]
  const def build() -> f32[N]:
    return [({alias} + SR) / 48000.0]
  const def host() -> f32:
    return {alias} / 48000.0
  const def length() -> i32:
    copy: i32[i32({alias} / 48000)] = Table
    return copy.len()
def read(values: f32[1] = Rates<1>::build()) -> f32:
  Host: f32 = Rates<1>::host()
  return values[0] + Host + f32(Rates<1>::length())
sample 2:
  out1 = read()
"#
        );
        assert_const_output(&source, 48_000.0, 5.0);
    }
}

fn assert_const_output(source: &str, sample_rate: f32, expected: f32) {
    let (mut instance, _, _) = compile_instance_with_options(
        source,
        64,
        CompileOptions {
            sample_rate,
            block_size: 64,
            fast_math: false,
            opt_level: TargetOptLevel::O3,
        },
    );
    let mut output = [0.0; 64];
    for _ in 0..8 {
        process_interleaved(&mut instance, &[], &mut output, 64).unwrap();
    }
    assert!(
        output
            .iter()
            .all(|&sample| (sample - expected).abs() < 0.0001),
        "{source}\nrate={sample_rate}, expected={expected}, output={output:?}"
    );
}

#[test]
fn shared_runtime_default_graphs_execute_in_each_caller_context() {
    let source = r#"
const def rate() -> f32:
  return SR / 48000
def d0(x: f32 = rate()) -> f32:
  return x
def d1(x: f32 = d0() + d0()) -> f32:
  return x
def d2(x: f32 = d1() + d1()) -> f32:
  return x
init:
  initial = d2()
sample 2:
  out1 = initial + d2()
"#;
    assert_context_output(source, 12.0);
}

#[test]
fn specialization_preserves_scalar_binding_widths_across_scopes_and_contexts() {
    for declaration in [
        "scoped(flag: bool, ignored: f32)",
        "scoped<T>(flag: bool, ignored: T)",
        "scoped(flag: bool, ignored)",
    ] {
        let source = format!(
            r#"
def {declaration} -> i64:
  if flag:
    x = i32(7)
    return i64(x + 2147483647)
  if !flag:
    x = i64(7)
    return x + 2147483647
  return 0
init:
  initial = scoped(true, 0.0)
sample 2:
  valid = initial == i64(-2147483642) && scoped(true, 0.0) == i64(-2147483642)
  out1 = f32(valid && scoped(false, 0.0) == i64(2147483654))
"#
        );
        assert_context_output(&source, 1.0);
    }
}

#[test]
fn specialization_preserves_runtime_slice_lengths_across_scopes_and_contexts() {
    for declaration in [
        "scoped(flag: bool, ignored: f32)",
        "scoped<T>(flag: bool, ignored: T)",
        "scoped(flag: bool, ignored)",
    ] {
        let source = format!(
            r#"
def {declaration} -> i32:
  values: i32[4] = [1, 2, 3, 4]
  if flag:
    xs = values[:2]
    return xs.len()
  if !flag:
    xs = values[:3]
    return xs.len()
  return 0
init:
  initial = scoped(true, 0.0)
sample 2:
  valid = initial == 2 && scoped(true, 0.0) == 2 && scoped(false, 0.0) == 3
  out1 = f32(valid)
"#
        );
        assert_context_output(&source, 1.0);
    }
}

#[test]
fn contextual_constants_select_width_at_each_concrete_use() {
    let definitions = "const Precise = 16777217.0\nconst Alias = Precise + 0.0\ndef difference<T>(value: T, base: T) -> T:\n  return value - base\n";
    for (ty, expected) in [("f32", 0.0), ("f64", 1.0)] {
        for expression in [
            "difference(Alias, base)",
            "-difference(base, Alias)",
            "Alias - base",
            "max(base, Alias) - base",
        ] {
            let source = format!("{definitions}init:\n  base: {ty} = 16777216.0\n  result = {expression}\nsample:\n  out1 = f32(result)\n");
            assert_context_output(&source, expected);
        }
    }
}

#[test]
fn contextual_integer_constants_use_the_selected_runtime_width() {
    for (ty, expected) in [("i32", 0.0), ("i64", 1.0)] {
        for args in ["value, Limit", "Limit, value"] {
            let source = format!("const Limit = 2147483647\ndef add<T>(a: T, b: T) -> T:\n  return a + b\ninit:\n  value: {ty} = 1\n  result = add({args})\nsample:\n  out1 = f32(result > 0)\n");
            assert_context_output(&source, expected);
        }
    }
}

#[test]
fn contextual_constant_binding_defaults_match_const_and_native_execution() {
    for (value, operation, expected) in [
        ("2147483647", "+ 1", "-2147483648"),
        ("-2147483648", "- 1", "2147483647"),
        ("2147483648", "+ 1", "2147483649"),
        ("64", "+ 1", "65"),
    ] {
        let source = format!("const Limit = {value}\nconst Alias = Limit\nconst def folded() -> i64:\n  value = Alias\n  return value {operation}\ndef native() -> i64:\n  value = Alias\n  return value {operation}\nsample:\n  out1 = f32(folded() == native() && native() == i64({expected}))\n");
        assert_context_output(&source, 1.0);
    }
}

#[test]
fn contextual_constants_evaluate_once_before_use_site_narrowing() {
    assert_const_and_native_result(
        |prefix| {
            format!("const Half = (2147483647 + 1) / 2\nconst Alias = Half\n{prefix}def run() -> i32:\n  named: i32 = Alias\n  inline: i32 = (2147483647 + 1) / 2\n  return i32((named == 1073741824 && inline == -1073741824))\n")
        },
        "1",
    );
}

#[test]
fn contextual_builtin_constants_keep_full_evaluation_precision() {
    let source = "const Count = BS * BS\nconst Huge = Count * Count * Count * Count\nconst Alias = Huge\nconst Rate = SR\nconst Volume = Rate * Rate * Rate\nconst Direct = SR * SR * SR\nconst Angle = sin(SR)\nconst Fixed = f32(SR) * f32(SR) * f32(SR)\ninit:\n  wide: i64 = Alias\n  inferred = Alias\nsample:\n  rate: f64 = SR\n  expected = rate * rate * rate\n  narrow = f32(SR) * f32(SR) * f32(SR)\n  out1 = f32((wide == 4294967296 && inferred == wide && Volume == expected && Direct == expected && abs(Angle - sin(rate)) < 0.000000000001 && Fixed == narrow && f64(Fixed) != expected))\n";
    assert_context_output(source, 1.0);
}

#[test]
fn contextual_integer_subexpressions_keep_full_precision_in_mixed_constants() {
    // At the test's block size of 16, eight products exceed i32.
    let product = "BS * BS * BS * BS * BS * BS * BS * BS";
    let source = format!(
        "const Integer = {product}\nconst ViaAlias = Integer + 0.0\nconst Direct = ({product}) + 0.0\nconst Divided = ({product}) / 3 + 0.0\nconst Greater = ({product}) > BS\nconst Narrow = i32(BS) * i32(BS) * i32(BS) * i32(BS) * i32(BS) * i32(BS) * i32(BS) * i32(BS)\nsample:\n  out1 = f32((Direct == ViaAlias && Direct == 4294967296.0 && Divided == 1431655765.0 && Greater && Narrow == 0))\n"
    );
    assert_context_output(&source, 1.0);
}

#[test]
fn constant_graph_fanout_converts_separately_for_each_destination() {
    for expression in ["Precise", "Values[0]"] {
        let source = format!(
            "const Precise: f64 = 16777217.0\nconst Values: f64[1] = [Precise]\nproc Narrow:\n  ins:\n    in1: f32\n  sample:\n    out1 = in1\nproc Wide:\n  ins:\n    in1: f64\n  outs:\n    out1: f64\n  sample:\n    out1 = in1\ninit:\n  narrow = Narrow()\n  wide = Wide()\ngraph:\n  {expression} >> {{ narrow.in1, wide.in1 }}\n  f32(wide.out1 - f64(narrow.out1)) >> out1\n"
        );
        assert_context_output(&source, 1.0);
    }
}

#[test]
fn delayed_constant_graph_fanout_preserves_precision_and_timing_in_either_order() {
    for (input, expression, selection) in [("", "Precise", "in1"), ("[2]", "Values", "in1[0]")] {
        for destinations in ["narrow.in1, wide.in1", "wide.in1, narrow.in1"] {
            for delay in [1, 19] {
                let source = format!(
                    "const Precise: f64 = 16777217.0\nconst Values = [Precise, Precise]\nproc Narrow:\n  ins:\n    in1: f32{input}\n  sample:\n    out1 = {selection}\nproc Wide:\n  ins:\n    in1: f64{input}\n  outs:\n    out1: f64\n  sample:\n    out1 = {selection}\ninit:\n  narrow = Narrow()\n  wide = Wide()\ngraph:\n  {expression} >>[{delay}] {{ {destinations} }}\n  f32(wide.out1 - f64(narrow.out1)) >> out1\n"
                );
                for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
                    let (mut instance, _, _) = compile_instance_with_options(
                        &source,
                        16,
                        CompileOptions {
                            sample_rate: 48_000.0,
                            block_size: 16,
                            fast_math: false,
                            opt_level,
                        },
                    );
                    let mut output = [0.0; 16];
                    for block in 0..3 {
                        process_interleaved(&mut instance, &[], &mut output, 16).unwrap();
                        for (frame, value) in output.iter().enumerate() {
                            let expected = if block * 16 + frame < delay { 0.0 } else { 1.0 };
                            assert_eq!(*value, expected, "{source}\nblock {block}, frame {frame}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn delayed_runtime_graph_fanout_preserves_values_and_timing_in_either_order() {
    for (narrow_shape, wide_shape, expression, selection, cosine) in [
        ("", "", "sin(in1)", "in1", false),
        ("[1]", "[1]", "[sin(in1)]", "in1[0]", false),
        ("[2]", "[2]", "[sin(in1), cos(in1)]", "in1[1]", true),
        ("[2]", "[3]", "sin(in1)", "in1[1]", false),
    ] {
        for destinations in ["narrow.in1, wide.in1", "wide.in1, narrow.in1"] {
            for delay in [1, 19] {
                let source = format!(
                    "ins 1\nouts 2\nproc Narrow:\n  ins:\n    in1: f32{narrow_shape}\n  sample:\n    out1 = {selection}\nproc Wide:\n  ins:\n    in1: f64{wide_shape}\n  outs:\n    out1: f64\n  sample:\n    out1 = {selection}\ninit:\n  narrow = Narrow()\n  wide = Wide()\ngraph:\n  {expression} >>[{delay}] {{ {destinations} }}\n  narrow.out1 >> out1\n  f32(wide.out1) >> out2\n"
                );
                for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
                    let (mut instance, _, _) = compile_instance_with_options(
                        &source,
                        16,
                        CompileOptions {
                            sample_rate: 48_000.0,
                            block_size: 16,
                            fast_math: false,
                            opt_level,
                        },
                    );
                    let mut output = [0.0; 32];
                    for block in 0..3 {
                        let input = std::array::from_fn::<_, 16, _>(|frame| {
                            (block * 16 + frame) as f32 * 0.05
                        });
                        process_interleaved(&mut instance, &input, &mut output, 16).unwrap();
                        for (frame, channels) in output.as_chunks::<2>().0.iter().enumerate() {
                            let expected = if block * 16 + frame < delay {
                                0.0
                            } else {
                                let input = (block * 16 + frame - delay) as f32 * 0.05;
                                if cosine {
                                    input.cos()
                                } else {
                                    input.sin()
                                }
                            };
                            for value in channels {
                                assert!(
                                    (*value - expected).abs() < 0.000001,
                                    "{source}\nblock {block}, frame {frame}: {channels:?}, expected {expected}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn delayed_mixed_graph_fanout_preserves_contextual_constant_components() {
    for destinations in ["narrow.in1, wide.in1", "wide.in1, narrow.in1"] {
        for delay in [1, 19] {
            let source = format!(
                "const Precise = 16777217.0\nins 1\nproc Narrow:\n  ins:\n    in1: f32[2]\n  sample:\n    out1 = in1[1]\nproc Wide:\n  ins:\n    in1: f64[2]\n  outs:\n    out1: f64\n  sample:\n    out1 = in1[1]\ninit:\n  narrow = Narrow()\n  wide = Wide()\ngraph:\n  [sin(in1), Precise] >>[{delay}] {{ {destinations} }}\n  f32(wide.out1 - f64(narrow.out1)) >> out1\n"
            );
            for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
                let (mut instance, _, _) = compile_instance_with_options(
                    &source,
                    16,
                    CompileOptions {
                        sample_rate: 48_000.0,
                        block_size: 16,
                        fast_math: false,
                        opt_level,
                    },
                );
                let mut output = [0.0; 16];
                for block in 0..3 {
                    process_interleaved(&mut instance, &[0.5; 16], &mut output, 16).unwrap();
                    for (frame, value) in output.iter().enumerate() {
                        let expected = if block * 16 + frame < delay { 0.0 } else { 1.0 };
                        assert_eq!(*value, expected, "{source}\nblock {block}, frame {frame}");
                    }
                }
            }
        }
    }
}

#[test]
fn resource_constant_writes_follow_destination_precision() {
    for source in [
        "init:\n  values: f32[1] = [0]\nsample:\n  write_unsafe(values, 0, VALUE)\n  out1 = values[0]\n",
        "sample:\n  values: f32[1] = [0]\n  values.write_unsafe(0, VALUE)\n  out1 = values[0]\n",
        "init:\n  values: f32[2] = [0, 0]\nsample:\n  xs = values[:1]\n  write_unsafe(xs, 0, VALUE)\n  out1 = xs[0]\n",
        "def write(values: f32[1]):\n  write_unsafe(values, 0, VALUE)\ninit:\n  values: f32[1] = [0]\nsample:\n  write(values)\n  out1 = values[0]\n",
        "def write(values: f32[]):\n  write_unsafe(values, 0, VALUE)\ninit:\n  values: f32[1] = [0]\nsample:\n  write(values)\n  out1 = values[0]\n",
    ] {
        for (expression, expected) in [
            ("Precise - 16777216.0", 0.0),
            ("Fixed - 16777216.0", 1.0),
            ("i64(7)", 7.0),
        ] {
            let source = format!(
                "const Precise = 16777217.0\nconst Fixed: f64 = Precise\n{}",
                source.replace("VALUE", expression)
            );
            assert_context_output(&source, expected);
        }
    }
}

#[test]
fn concrete_constant_arithmetic_converts_after_its_own_width_is_evaluated() {
    assert_const_and_native_result(
        |prefix| {
            format!(
                "const Integer: i64 = 2147483647\nconst Float: f64 = 16777217.0\n{prefix}def take(value: f32 = Float - 16777216.0) -> f32:\n  return value\n{prefix}def run() -> i32:\n  integer: i32 = (Integer + 1) / 2\n  float: f32 = Float - 16777216.0\n  values: f32[1] = [Float - 16777216.0]\n  return i32((integer == 1073741824 && float == 1.0 && values[0] == 1.0 && take() == 1.0 && take(Float - 16777216.0) == 1.0))\n"
            )
        },
        "1",
    );
}
