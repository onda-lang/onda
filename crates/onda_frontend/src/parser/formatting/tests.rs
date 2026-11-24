use super::*;

fn formatted(source: &str) -> String {
    let path = Path::new("test.onda");
    let result = format_source(source, path).unwrap();
    assert_eq!(format_source(&result, path).unwrap(), result);
    result
}

#[test]
fn formats_authored_code_without_resolving_imports_or_changing_literals() {
    let source = "\nimport   some/unavailable\ninclude   \"a file.on\"\n\nsample:\n\t# cue\n\tif(x<0):\n\t\tprint( \"mètre  #, \\\"L\\\"\" , - x ) # keep\n\telse:\n\t\tout1=f<f32>(x*2, n=bank[1:3])\n";
    assert_eq!(formatted(source), "import some/unavailable\ninclude \"a file.on\"\n\nsample:\n  # cue\n  if (x < 0):\n    print(\"mètre  #, \\\"L\\\"\", -x)  # keep\n  else:\n    out1 = f<f32>(x * 2, n = bank[1:3])\n");
}

#[test]
fn wraps_nested_arguments_and_reflows_existing_breaks() {
    let args = [
        "0.0",
        "std::smoothing::_duration_samples(release_s)",
        "release_shape",
        "release_curve",
        "linear_slope = true",
    ];
    let expected = format!(
        "sample:\n  transition.begin(\n    {}\n  )  # note\n",
        args.join(",\n    ")
    );
    for separator in [", ", ",\n    "] {
        let input = format!(
            "sample:\n  transition.begin({}) # note",
            args.join(separator)
        );
        assert_eq!(formatted(&input), expected);
    }
    assert_eq!(
        formatted("sample:\n  out1 = f(\n    a,\n    g(b,c)\n  )"),
        "sample:\n  out1 = f(a, g(b, c))\n"
    );
}

#[test]
fn packs_short_array_elements_to_the_line_width() {
    let notes =
        "0.0, 0.0, 12.0, 3.0, 0.0, 7.0, 10.0, 7.0, 0.0, 12.0, 15.0, 10.0, 5.0, 3.0, 7.0, 10.0";
    let declaration = "const Notes: f32[16] =";
    let expected = format!("{declaration} [\n  {notes}\n]\n");
    for separator in [", ", ",\n    "] {
        let source = format!("{declaration} [{}]", notes.replace(", ", separator));
        assert_eq!(formatted(&source), expected);
    }

    let values = ["9"; 50].join(", ");
    let first = ["9"; 33].join(", ");
    let rest = ["9"; 17].join(", ");
    assert_eq!(
        formatted(&format!("const Values: i32[50] = [{values}]")),
        format!("const Values: i32[50] = [\n  {first},\n  {rest}\n]\n")
    );
    assert_eq!(
        formatted("const Small: i32[3] = [\n    -1,\n    0,\n    1\n]"),
        "const Small: i32[3] = [-1, 0, 1]\n"
    );
}

#[test]
fn wraps_arrays_with_long_or_complex_elements_one_per_line() {
    for item in ["mix(a, b)", "LongConstant"] {
        let values = [item; 9].join(", ");
        let rows = [item; 9].join(",\n  ");
        assert_eq!(
            formatted(&format!("const Values: f32[9] = [{values}]")),
            format!("const Values: f32[9] = [\n  {rows}\n]\n")
        );
    }
}

#[test]
fn packs_short_array_elements_between_comment_and_blank_line_anchors() {
    let values = ["-1"; 35].join(", ");
    let full = ["-1"; 24].join(", ");
    let rest = ["-1"; 11].join(", ");
    let source = format!("const Values: i32[105] = [{values}, # first\n  {values},\n\n  {values}]");
    let expected = format!(
        "const Values: i32[105] = [\n  {full},\n  {rest},  # first\n  {full},\n  {rest},\n\n  {full},\n  {rest}\n]\n"
    );
    assert_eq!(formatted(&source), expected);
}

#[test]
fn wraps_long_if_elif_and_while_conditions_at_boolean_operators() {
    let terms = "!self.active(KickPattern) && !self.active(SnarePattern) && !self.active(HatPattern) && !self.active(OpenHatPattern)";
    let rows = terms.replace(" && ", "\n    && ");
    for (prefix, keyword) in [
        ("", "if"),
        ("", "while"),
        ("  if false:\n    out1 = 0\n", "elif"),
    ] {
        let expected =
            format!("sample:\n{prefix}  {keyword} (\n    {rows}\n  ):  # enabled\n    out1 = 1\n");
        for condition in [terms.to_owned(), format!("({terms})")] {
            let source = format!("sample:\n{prefix}  {keyword} {condition}: # enabled\n    out1=1");
            assert_eq!(formatted(&source), expected);
        }
    }
}

#[test]
fn wraps_braced_conditions_and_preserves_inner_grouping() {
    let terms = "!self.active(KickPattern) && (!self.active(SnarePattern) || !self.active(HatPattern)) || !self.active(OpenHatPattern)";
    let rows = "!self.active(KickPattern)\n    && (!self.active(SnarePattern) || !self.active(HatPattern))\n    || !self.active(OpenHatPattern)";
    let source = format!("sample {{\n  if {terms} {{\n    out1=1\n  }}\n}}");
    assert_eq!(
        formatted(&source),
        format!("sample {{\n  if (\n    {rows}\n  ) {{\n    out1 = 1\n  }}\n}}\n")
    );

    let path = Path::new("test.onda");
    assert_ne!(
        syntax_tree("sample:\n  if a && (b || c):\n    out1=1", path),
        syntax_tree("sample:\n  if (a && b || c):\n    out1=1", path)
    );
    assert_eq!(
        formatted("sample:\n  if a && b:\n    out1=1"),
        "sample:\n  if a && b:\n    out1 = 1\n"
    );
}

#[test]
fn respects_explicit_braces_comments_and_crlf() {
    let source = "sample {\r\n    out1 = f(1, # left\r\n      2)\r\n}\r\n";
    assert_eq!(
        formatted(source),
        "sample {\r\n  out1 = f(1,  # left\r\n    2)\r\n}\r\n"
    );
    assert_eq!(formatted("sample{out1=1}"), "sample { out1 = 1 }\n");
}

#[test]
fn separates_loop_steps_ranges_and_graph_rates() {
    assert_eq!(
        formatted("sample:\n    for i@-1 in 5..=0:\n        out1+=i"),
        "sample:\n  for i @ -1 in 5..=0:\n    out1 += i\n"
    );
    assert_eq!(
        formatted("sample:\n  for i in 0. .. 5:\n    out1=i"),
        "sample:\n  for i in 0. ..5:\n    out1 = i\n"
    );
    assert_eq!(
        formatted("graph:\n  @sample in1>>out1"),
        "graph:\n  @sample in1 >> out1\n"
    );
}

#[test]
fn keeps_useful_continuations_in_long_arithmetic_expressions() {
    let expression = format!(
        "sample:\n  out1 = (\n    {}\n    + {}\n  )\n",
        "a".repeat(55),
        "b".repeat(55)
    );
    assert_eq!(formatted(&expression), expression);
}

#[test]
fn distinguishes_the_sample_section_from_a_namespace_segment() {
    assert_eq!(
        formatted("namespace std::sample <Channels=2>:\n  const Size=Channels\nsample(2*2):\n  out1=0"),
        "namespace std::sample<Channels = 2>:\n  const Size = Channels\nsample (2 * 2):\n  out1 = 0\n"
    );
}

#[test]
fn keeps_nested_generic_closers_separate_from_shift_operators() {
    let source = "sample:\n    f<A::B<1>   >(2)\n    out1=f<A::B<1> >(8>>1)\n";
    let output = formatted(source);
    assert_eq!(
        output,
        "sample:\n  f<A::B<1> >(2)\n  out1 = f<A::B<1> >(8 >> 1)\n"
    );
    let path = Path::new("test.onda");
    assert_eq!(syntax_tree(source, path), syntax_tree(&output, path));
}

#[test]
fn preserves_line_breaks_that_distinguish_comparisons_from_calls() {
    let source = "sample:\n    out1=(a<b>\n      (2)\n    )\n";
    let output = formatted(source);
    assert_eq!(output, "sample:\n  out1 = (a < b >\n    (2)\n  )\n");
    let path = Path::new("test.onda");
    assert_eq!(syntax_tree(source, path), syntax_tree(&output, path));
}

#[test]
fn wraps_angle_delimited_argument_lists() {
    let first = "FirstArgument".repeat(5);
    let second = "SecondArgument".repeat(4);
    for (source, expected) in [
        (
            format!("namespace Demo<{first} = 1, {second} = 2>:\n  const Value = 1\n"),
            format!("namespace Demo<\n  {first} = 1,\n  {second} = 2\n>:\n  const Value = 1\n"),
        ),
        (
            format!("sample:\n  out1 = f<{first}, {second}>(in1)\n"),
            format!("sample:\n  out1 = f<\n    {first},\n    {second}\n  >(in1)\n"),
        ),
    ] {
        assert_eq!(formatted(&source), expected);
    }
}

fn syntax_tree(source: &str, path: &Path) -> Vec<syntax::ItemSyntax> {
    syntax::parse_syntax(source, path, None).unwrap()
}

#[test]
fn builtin_sources_retain_the_same_syntax() {
    for module in crate::stdlib_module_names() {
        let source = crate::stdlib_module_source(module).unwrap();
        let path = Path::new(module);
        let output = format_source(source, path).unwrap();
        assert_eq!(format_source(&output, path).unwrap(), output, "{module}");
        assert!(
            syntax_tree(source, path) == syntax_tree(&output, path),
            "{module}"
        );
    }
}

#[test]
fn preserves_colon_suite_indentation_across_explicit_braces() {
    let path = Path::new("test.onda");
    for (source, expected) in [
        ("sample:\n    if in1 > 0 {\n    out1=1\n    }",
         "sample:\n  if in1 > 0 {\n  out1 = 1\n  }\n"),
        ("sample {\n    if in1 > 0:\n        if in1 < 1 {\n        if in1 > 0:\n            out1=1\n        }\n    out1=2\n}",
         "sample {\n  if in1 > 0:\n    if in1 < 1 {\n    if in1 > 0:\n      out1 = 1\n    }\n  out1 = 2\n}\n"),
        ("buffers:\n    wave {\n    count=2\n    }",
         "buffers:\n  wave {\n  count = 2\n  }\n"),
        ("params:\n    wet=1 {\n        min=0,\n        max=1\n    }",
         "params:\n  wet = 1 {\n    min = 0,\n    max = 1\n  }\n"),
    ] {
        let output = formatted(source);
        assert_eq!(output, expected);
        assert_eq!(syntax_tree(source, path), syntax_tree(&output, path));
    }
}

#[test]
fn formats_indented_metadata_and_the_following_code() {
    let path = Path::new("test.onda");
    for (source, expected) in [
        (
            "buffers:\n    clips:\n        count=2\nsample:\n    out1=1\n",
            "buffers:\n  clips:\n    count = 2\nsample:\n  out1 = 1\n",
        ),
        (
            "buffers {\n    clips: # voices\n        count=2\n}\nimport std/math\nparams:\n    wet=1:\n        min=0,max=2\nsample:\n    out1=wet\n",
            "buffers {\n  clips:  # voices\n    count = 2\n}\nimport std/math\nparams:\n  wet = 1:\n    min = 0, max = 2\nsample:\n  out1 = wet\n",
        ),
        (
            "ins:\n    in1:\n        min=0,max=1\ninit:\n    cursor:i32=1:\n        0..=2,wrap\nsample:\n    out1=in1+cursor\n",
            "ins:\n  in1:\n    min = 0, max = 1\ninit:\n  cursor: i32 = 1:\n    0..=2, wrap\nsample:\n  out1 = in1 + cursor\n",
        ),
    ] {
        let output = formatted(source);
        assert_eq!(output, expected);
        assert_eq!(syntax_tree(source, path), syntax_tree(&output, path));
    }
}

#[test]
fn preserves_explicit_suites_when_reflowing_a_header() {
    let source = format!(
        "sample:\n  if (16 >> shift({}, 1)) > 0 {{\n    out1=1\n  }}\n",
        "argument".repeat(14)
    );
    let result = formatted(&source);
    let path = Path::new("test.onda");
    assert_eq!(syntax_tree(&source, path), syntax_tree(&result, path));
    let generics = "sample:\n  if f<A::B<C> >() {\n  if in1>0:\n    out1=2\n  }\n";
    assert_eq!(
        syntax_tree(generics, path),
        syntax_tree(&formatted(generics), path)
    );
}

#[test]
fn wraps_lists_on_both_sides_of_comment_and_blank_lines() {
    let args = vec!["f(1, 2)"; 16].join(", ");
    let lines = vec!["    f(1, 2)"; 16].join(",\n");
    for anchor in ["  # first batch\n", "\n    # next batch\n", "\n\n"] {
        let source = format!("sample:\n  print({args},{anchor}    {args})\n");
        let expected = format!("sample:\n  print(\n{lines},{anchor}{lines}\n  )\n");
        assert_eq!(formatted(&source), expected);
    }
    let arg = "a".repeat(60);
    let comment = "x".repeat(32);
    let source = format!("sample:\n  print({arg}, 1, # {comment}\n    2)");
    let expected = format!("sample:\n  print(\n    {arg},\n    1,  # {comment}\n    2)\n");
    assert_eq!(formatted(&source), expected);
}

#[test]
fn reports_compiler_syntax_diagnostics_at_the_authored_location() {
    let path = Path::new("test.onda");
    for (source, message, location) in [
        (
            "def first():\n  return 1\nimport std/math\nsample:\n  out1=9223372036854775808\n",
            "integer literal is out of range",
            (5, 8),
        ),
        ("sample:\n  out1=1\n}\n", "unmatched '}'", (3, 1)),
    ] {
        let errors = format_source(source, path).unwrap_err();
        let compiler_errors = crate::parse_program_with_path(source, path).unwrap_err();
        for error in [&errors[0], &compiler_errors[0]] {
            assert_eq!(error.message, message);
            assert_eq!((error.line, error.column), location);
            assert!(error.file.as_deref().unwrap().ends_with("test.onda"));
        }
    }
    let eof = "def first():\n  if 1:\n    return 1\nsample {\n  out1 = 1\n";
    let formatted_error = &format_source(eof, path).unwrap_err()[0];
    let compiler_error = &crate::parse_program_with_path(eof, path).unwrap_err()[0];
    assert_eq!((formatted_error.line, formatted_error.column), (6, 1));
    assert_eq!((compiler_error.line, compiler_error.column), (6, 1));
    assert_eq!(
        formatted("sample:\n  out1=-9223372036854775808"),
        "sample:\n  out1 = -9223372036854775808\n"
    );
}
