use super::*;

fn position_at(source: &str, needle: &str, token_offset: usize) -> NavigationPosition {
    let byte = source
        .find(needle)
        .map(|idx| idx + token_offset)
        .expect("test needle should exist");
    let line = source[..byte].bytes().filter(|byte| *byte == b'\n').count() as u32;
    let line_start = source[..byte].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    NavigationPosition {
        line,
        character: (byte - line_start) as u32,
    }
}

fn definition_at(source: &str, needle: &str, token_offset: usize) -> Option<Value> {
    let parsed = parse_program(source).expect("test source should parse");
    definition_for_document_with_parsed(
        source,
        None,
        &HashMap::new(),
        Some(&parsed),
        position_at(source, needle, token_offset),
    )
}

fn hover_at(source: &str, needle: &str, token_offset: usize) -> Option<Value> {
    let parsed = parse_program(source).expect("test source should parse");
    hover_for_document_with_parsed(
        source,
        None,
        &HashMap::new(),
        Some(&parsed),
        position_at(source, needle, token_offset),
    )
}

fn signature_at(source: &str, needle: &str, token_offset: usize) -> Option<Value> {
    let parsed = parse_program(source).expect("test source should parse");
    signature_help_for_document_with_parsed(
        source,
        None,
        &HashMap::new(),
        Some(&parsed),
        position_at(source, needle, token_offset),
    )
}

#[test]
fn unsafe_index_intrinsics_have_hover_and_signature_help() {
    let source = r#"outs:
  out1
init:
  values: f32[8]
sample:
  out1 = values.read_unsafe(0)
  write_unsafe(values, 0, out1)
"#;
    let hover = hover_at(source, "values.read_unsafe", "values.".len() + "read".len())
        .expect("unsafe read hover");
    let markdown = hover["contents"]["value"].as_str().expect("hover markdown");
    assert!(markdown.contains("memory-unsafe"), "{markdown}");

    let member = signature_at(source, "read_unsafe(0)", "read_unsafe(0".len())
        .expect("member unsafe signature");
    assert_eq!(member["signatures"][0]["label"], "read_unsafe(index, ...)");
    assert_eq!(member["activeParameter"], 0);

    let free = signature_at(
        source,
        "write_unsafe(values, 0, out1)",
        "write_unsafe(values, 0, ".len(),
    )
    .expect("free unsafe signature");
    assert_eq!(
        free["signatures"][0]["label"],
        "write_unsafe(storage, index, ..., value)"
    );
    assert_eq!(free["activeParameter"], 2);
}

#[test]
fn hides_proc_local_defs_from_external_member_navigation() {
    let source = r#"proc Voice:
  outs:
    out1
  def helper(x):
    return x
  sample:
    out1 = helper(0.0)

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice.helper(0.0)
"#;

    assert!(
        definition_at(source, "voice.helper", "voice.".len() + 1).is_none(),
        "external member access should not resolve private proc-local defs"
    );
    assert!(
        definition_at(source, "helper(0.0)", 1).is_some(),
        "proc-local def should still resolve inside its owning proc"
    );
}

#[test]
fn hides_private_params_from_external_member_navigation() {
    let source = r#"proc Voice:
  params:
    private cutoff = 1000.0
  outs:
    out1
  sample:
    out1 = cutoff

outs:
  out1
init:
  voice = Voice()
sample:
  out1 = voice.cutoff
"#;

    assert!(
        definition_at(source, "voice.cutoff", "voice.".len() + 1).is_none(),
        "external member access should not resolve private params"
    );
    assert!(
        definition_at(source, "out1 = cutoff", "out1 = ".len() + 1).is_some(),
        "private params should still resolve inside their owning proc"
    );
}

#[test]
fn resolves_proc_init_state_from_local_defs_and_events() {
    let source = r#"proc Voice:
  outs:
    out1

  event before_event():
    event_before = state

  def before_def():
    def_before = state

  init:
    state = 0.0

  def after_def():
    def_after = state

  event after_event():
    event_after = state

  sample:
    out1 = state
"#;

    for needle in [
        "event_before = state",
        "def_before = state",
        "def_after = state",
        "event_after = state",
        "out1 = state",
    ] {
        let definition = definition_at(source, needle, needle.find("state").unwrap() + 1)
            .unwrap_or_else(|| panic!("{needle} should resolve to init state"));
        assert_eq!(
            definition["range"]["start"]["line"],
            json!(11),
            "{needle} should goto init state declaration: {definition:?}"
        );
    }
}

#[test]
fn hover_resolves_count_shorthand_proc_ports() {
    let source = r#"proc Delay<T>:
  ins<T> 1
  outs<T> 1

  sample:
    out1 = in1
"#;

    let input_hover = hover_at(source, "in1", 1).expect("count-shorthand input should hover");
    let output_hover = hover_at(source, "out1", 1).expect("count-shorthand output should hover");
    let input = input_hover["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    let output = output_hover["contents"]["value"]
        .as_str()
        .unwrap_or_default();

    assert!(
        input.contains("proc input in1"),
        "input hover should describe generated input port: {input:?}"
    );
    assert!(
        output.contains("proc output out1"),
        "output hover should describe generated output port: {output:?}"
    );
}

#[test]
fn hover_shows_control_domains_for_top_level_and_proc_param_references() {
    let source = r#"proc Voice:
  params:
    gain = 1.0 {0.0, 2.0}
  outs:
    out1
  sample:
    out1 = gain

params:
  mix = 0.5 {0.0, 1.0, curve = -4, unit = "%", step = 0.25}
  cutoff = 440.0 {20.0, 20000.0, log, "Hz"}
outs:
  out1
init:
  voice = Voice()
sample:
  out1 = mix + cutoff + voice.gain
"#;

    let top_level = hover_at(source, "mix +", 1).expect("top-level param should hover");
    let logarithmic = hover_at(source, "cutoff +", 1).expect("logarithmic param should hover");
    let proc_local =
        hover_at(source, "out1 = gain", "out1 = ".len() + 1).expect("proc param should hover");
    let proc_external = hover_at(source, "voice.gain", "voice.".len() + 1)
        .expect("external proc param should hover");

    let top_level = top_level["contents"]["value"].as_str().unwrap_or_default();
    let logarithmic = logarithmic["contents"]["value"]
        .as_str()
        .unwrap_or_default();
    let proc_local = proc_local["contents"]["value"].as_str().unwrap_or_default();
    let proc_external = proc_external["contents"]["value"]
        .as_str()
        .unwrap_or_default();

    assert!(
        top_level.contains(r#"param mix = 0.5 {0.0, 1.0, curve = -4, unit = "%", step = 0.25}"#),
        "top-level param hover should include its control domain: {top_level:?}"
    );
    assert!(
        logarithmic.contains(r#"param cutoff = 440.0 {20.0, 20000.0, scale = log, unit = "Hz"}"#),
        "logarithmic param hover should include its control domain: {logarithmic:?}"
    );
    for hover in [proc_local, proc_external] {
        assert!(
            hover.contains("proc param gain = 1.0 {0.0, 2.0}"),
            "proc param hover should include its range: {hover:?}"
        );
    }
}

#[test]
fn resolves_struct_field_declaration_navigation() {
    let source = r#"struct Box:
  value: f32 = 0.0

  def get(self):
    return self.value
"#;

    let definition =
        definition_at(source, "value: f32", 1).expect("field declaration should resolve to itself");
    assert_eq!(
        definition["range"]["start"]["line"],
        json!(1),
        "field declaration should goto its own definition: {definition:?}"
    );

    let hover = hover_at(source, "value: f32", 1).expect("field hover should resolve");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("field value")),
        "field hover should describe the field: {hover:?}"
    );
}

#[test]
fn resolves_self_struct_field_navigation() {
    let source = r#"struct Box:
  value: f32 = 0.0

  def set(self, x):
    self.value = x

  def get(self):
    return self.value
"#;

    let write_definition = definition_at(source, "self.value", "self.".len() + 1)
        .expect("self.field write should resolve");
    assert_eq!(
        write_definition["range"]["start"]["line"],
        json!(1),
        "self.field write should goto the field declaration: {write_definition:?}"
    );

    let read_definition = definition_at(source, "return self.value", "return self.".len() + 1)
        .expect("self.field read should resolve");
    assert_eq!(
        read_definition["range"]["start"]["line"],
        json!(1),
        "self.field read should goto the field declaration: {read_definition:?}"
    );

    let hover =
        hover_at(source, "self.value", "self.".len() + 1).expect("self.field hover should resolve");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("field value")),
        "self.field hover should describe the field: {hover:?}"
    );
}

#[test]
fn resolves_self_struct_method_and_named_argument_navigation() {
    let source = r#"struct Box:
  value: f32 = 0.0

  def set(self, x):
    self.value = x

  def bump(self, amount):
    self.set(x = amount)
"#;

    let method_definition =
        definition_at(source, "self.set", "self.".len() + 1).expect("self.method should resolve");
    assert_eq!(
        method_definition["range"]["start"]["line"],
        json!(3),
        "self.method should goto the method declaration: {method_definition:?}"
    );

    let arg_definition =
        definition_at(source, "x = amount", 1).expect("method named arg should resolve");
    assert_eq!(
        arg_definition["range"]["start"]["line"],
        json!(3),
        "self.method named arg should goto the method parameter: {arg_definition:?}"
    );
}

#[test]
fn resolves_struct_constructor_field_named_arguments() {
    let source = r#"struct Pair:
  left: f32 = 0.0
  right: f32 = 0.0

init:
  p = Pair(left = 1.0, right = 2.0)
"#;

    let left_definition =
        definition_at(source, "left = 1.0", 1).expect("constructor field arg should resolve");
    assert_eq!(
        left_definition["range"]["start"]["line"],
        json!(1),
        "struct constructor arg should goto the field declaration: {left_definition:?}"
    );
}

#[test]
fn resolves_explicit_struct_typed_def_param_members() {
    let source = r#"struct Box:
  value: f32 = 0.0

  def get(self):
    return self.value

def read(box: Box):
  return box.value + box.get()
"#;

    let field_definition = definition_at(source, "box.value", "box.".len() + 1)
        .expect("typed struct param field should resolve");
    assert_eq!(
        field_definition["range"]["start"]["line"],
        json!(1),
        "typed struct param field should goto the field declaration: {field_definition:?}"
    );

    let method_definition = definition_at(source, "box.get", "box.".len() + 1)
        .expect("typed struct param method should resolve");
    assert_eq!(
        method_definition["range"]["start"]["line"],
        json!(3),
        "typed struct param method should goto the method declaration: {method_definition:?}"
    );
}

#[test]
fn resolves_namespace_local_const_use_navigation() {
    let source = r#"namespace DSP:
  const Bias = 0.5

  def shape(x):
    return x + Bias
"#;

    let definition = definition_at(source, "return x + Bias", "return x + ".len() + 1)
        .expect("namespace-local const use should resolve");
    assert_eq!(
        definition["range"]["start"]["line"],
        json!(1),
        "namespace-local const use should goto the const declaration: {definition:?}"
    );

    let hover = hover_at(source, "return x + Bias", "return x + ".len() + 1)
        .expect("namespace-local const hover should resolve");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("const Bias")),
        "namespace-local const hover should describe the const: {hover:?}"
    );
}

#[test]
fn resolves_namespace_local_const_in_generic_def_expression() {
    let source = r#"namespace sc:
  const TEST = 10
  
  def sampleDuration<T>():
    return T(1.0) / T(SR)

  def blockDuration<T>():
    return T(BS) / T(SR) * TEST
"#;

    let definition = definition_at(source, "* TEST", "* ".len() + 1)
        .expect("namespace-local const in generic def should resolve");
    assert_eq!(
        definition["range"]["start"]["line"],
        json!(1),
        "namespace-local generic-def const use should goto the const declaration: {definition:?}"
    );

    let hover = hover_at(source, "* TEST", "* ".len() + 1)
        .expect("namespace-local const hover should resolve in generic def");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("const TEST")),
        "namespace-local generic-def const hover should describe the const: {hover:?}"
    );
}

#[test]
fn resolves_top_level_task_await_and_reset_to_the_task_declaration() {
    let source = r#"task prepare():
  work = 1
  yield

block:
  prepare.reset()
  await prepare()
  sample:
    out1 = 0.0
"#;
    for (needle, offset) in [
        ("prepare.reset", 1),
        ("prepare.reset", "prepare.".len() + 1),
        ("await prepare", "await ".len() + 1),
    ] {
        let definition =
            definition_at(source, needle, offset).expect("task control reference should resolve");
        assert_eq!(definition["range"]["start"]["line"], json!(0));
    }

    let hover = hover_at(source, "await prepare", "await ".len() + 1)
        .expect("task await hover should resolve");
    assert!(
        hover["contents"]["value"]
            .as_str()
            .is_some_and(|value| value.contains("task prepare()")),
        "task hover should describe the declaration: {hover:?}"
    );
}

#[test]
fn proc_task_navigation_and_document_symbols_are_owner_local() {
    let source = r#"proc Loader:
  task prepare():
    work = 1
    yield
  event reload():
    prepare.reset()
  block:
    await prepare()
    sample:
      out1 = 0.0
"#;
    let definition = definition_at(source, "await prepare", "await ".len() + 1)
        .expect("proc task await should resolve");
    assert_eq!(definition["range"]["start"]["line"], json!(1));

    let parsed = parse_program(source).expect("test source should parse");
    let symbols =
        document_symbols_for_document_with_parsed(source, None, &HashMap::new(), Some(&parsed));
    let children = symbols[0]["children"]
        .as_array()
        .expect("proc document symbol children");
    assert!(children.iter().any(|symbol| symbol["name"] == "prepare"));
}

#[test]
fn delegate_hover_and_navigation_preserve_directional_identity() {
    let source = r#"proc Child:
  delegate stopped(reason: i32)
  sample:
    out1 = 0.0

delegate finished(reason: i32)
init:
  child = Child()
when child.stopped(reason):
  seen = reason
  finished(reason)
sample:
  out1 = child()
"#;
    let owner_hover =
        hover_at(source, "finished(reason)", 1).expect("owner delegate call should hover");
    assert!(owner_hover["contents"]["value"]
        .as_str()
        .is_some_and(|value| value.contains("delegate finished(reason: i32)")));
    let child_hover = hover_at(source, "child.stopped", "child.".len() + 1)
        .expect("subscription target should hover");
    assert!(child_hover["contents"]["value"]
        .as_str()
        .is_some_and(|value| value.contains("delegate stopped(reason: i32)")));
    let definition = definition_at(source, "child.stopped", "child.".len() + 1)
        .expect("subscription target should navigate");
    assert_eq!(definition["range"]["start"]["line"], json!(1));
    let binding_hover = hover_at(source, "seen = reason", "seen = ".len() + 1)
        .expect("when payload binding should hover in its body");
    assert!(binding_hover["contents"]["value"]
        .as_str()
        .is_some_and(|value| value.contains("delegate payload binding reason: i32")));
    let binding_definition = definition_at(source, "seen = reason", "seen = ".len() + 1)
        .expect("when payload binding should navigate to its declaration");
    assert_eq!(binding_definition["range"]["start"]["line"], json!(8));
}

#[test]
fn print_hover_and_signature_help_describe_variadic_typed_values() {
    let source = "sample:\n  print(\"left, right\", 3, 4.0)\n";
    let hover = hover_at(source, "print", 1).expect("print should hover");
    let markdown = hover["contents"]["value"]
        .as_str()
        .expect("print hover markdown");
    assert!(markdown.contains(PRINT_SIGNATURE));
    assert!(markdown.contains(PRINT_LABEL_SIGNATURE));

    let signature = signature_at(
        source,
        "print(\"left, right\", 3, 4.0)",
        "print(\"left, right\", 3, ".len(),
    )
    .expect("print should have signature help");
    assert_eq!(signature["activeSignature"], 1);
    assert_eq!(signature["activeParameter"], 1);
    assert_eq!(signature["signatures"][1]["label"], PRINT_LABEL_SIGNATURE);
}

#[test]
fn event_and_delegate_signature_help_preserve_explicit_parameters() {
    let source = r#"proc Child:
  event start(value: f32, enabled: bool):
    return
  delegate ready(reason: i32)
  sample:
    out1 = 0.0

init:
  children: Child[2] = Child()
  children[0].start(1.0, true)

when children.ready(index, reason):
  seen = reason

sample:
  out1 = children[0]()
"#;
    let event = signature_at(
        source,
        "children[0].start(1.0, true)",
        "children[0].start(1.0, ".len(),
    )
    .expect("child event should have signature help");
    assert_eq!(event["activeParameter"], 1);
    assert_eq!(
        event["signatures"][0]["label"],
        "event start(value: f32, enabled: bool)"
    );

    let delegate = signature_at(
        source,
        "when children.ready(index, reason)",
        "when children.ready(index, ".len(),
    )
    .expect("whole-array delegate target should have signature help");
    assert_eq!(delegate["activeParameter"], 1);
    assert_eq!(
        delegate["signatures"][0]["label"],
        "delegate ready(index: i32, reason: i32)"
    );
}

#[test]
fn when_symbols_and_typed_bindings_are_visible() {
    let source = r#"proc Child:
  delegate ready(values: f32[])
  when ready(values):
    nested_count = values.len()
  sample:
    out1 = 0.0

init:
  child = Child()

when child.ready(values):
  count = values.len()

sample:
  out1 = child()
"#;
    let hover = hover_at(source, "count = values.len", "count = ".len() + 1)
        .expect("typed binding should hover");
    assert!(hover["contents"]["value"]
        .as_str()
        .is_some_and(|value| value.contains("delegate payload binding values: f32[]")));

    let parsed = parse_program(source).expect("test source should parse");
    let symbols =
        document_symbols_for_document_with_parsed(source, None, &HashMap::new(), Some(&parsed));
    assert!(symbols
        .iter()
        .any(|symbol| symbol["name"] == "when child.ready"));
    let proc_children = symbols[0]["children"]
        .as_array()
        .expect("proc document symbol children");
    assert!(proc_children
        .iter()
        .any(|symbol| symbol["name"] == "when ready"));
}
