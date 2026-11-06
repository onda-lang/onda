use super::*;

#[test]
fn owned_element_copy_chains_keep_resource_environments_bounded() {
    fn count(shape: &ProcessorViewShape) -> usize {
        fn frame(environment: &ProcessorEnvironmentShape) -> usize {
            1 + environment.outer.as_deref().map_or(0, frame)
                + environment
                    .resource_sources
                    .iter()
                    .map(count)
                    .sum::<usize>()
        }
        shape.environments.iter().map(frame).sum()
    }
    let size = |copies| {
        let mut source = String::from("proc Voice:\n  buffers:\n    clip: f32\n  sample:\n    out1 = clip[0]\ndef step(voice) -> f32:\n  return voice()\ndef exercise(v: Voice, index: i32) -> f32:\n  a0: Voice[2] = [v, v]\n");
        for i in 1..copies {
            source.push_str(&format!(
                "  a{i}: Voice[2] = [a{}[index], a{}[1 - index]]\n",
                i - 1,
                i - 1
            ));
        }
        source.push_str(&format!("  return step(a{}[index])\nbuffers:\n  clip: f32\ninit:\n  v = Voice(clip = clip)\nsample:\n  out1 = exercise(v, i32(in1))\n", copies - 1));
        let typed = analyze(&source);
        let shape = typed
            .processor_views
            .function_params
            .iter()
            .find(|(name, _)| name.starts_with("step.__onda_mono__"))
            .expect("specialized helper")
            .1
            .get("voice")
            .unwrap();
        let size = count(shape);
        let mir = crate::lower_program_to_validated_mir(&typed).unwrap();
        let parameters = mir
            .functions
            .iter()
            .map(|function| function.params.len())
            .max()
            .unwrap();
        (size, parameters)
    };
    assert_eq!(size(2), size(64));
}

#[test]
fn owned_processor_copy_chains_keep_resource_environments_bounded() {
    for copies in [1, 128] {
        let mut source = String::from(
            "proc Voice:\n  buffers:\n    clip: f32\n  sample:\n    out1 = clip[0]\ndef step(voice) -> f32:\n  return voice()\nbuffers:\n  clip: f32\ninit:\n  original = Voice(clip = clip)\n",
        );
        let mut previous = "original".to_owned();
        for index in 0..copies {
            let name = format!("copy{index}");
            source.push_str(&format!("  {name}: Voice = {previous}\n"));
            previous = name;
        }
        source.push_str(&format!("sample:\n  out1 = step({previous})\n"));
        let typed = analyze(&source);
        let (_, parameters) = typed
            .processor_views
            .function_params
            .iter()
            .find(|(name, _)| name.starts_with("step.__onda_mono__"))
            .expect("specialized helper");
        let mut environment = parameters["voice"].environments.first();
        let mut depth = 0;
        while let Some(frame) = environment {
            depth += 1;
            environment = frame.outer.as_deref();
        }
        assert!(
            depth <= 2,
            "{copies} copies retained {depth} resource frames"
        );
        crate::lower_program_to_validated_mir(&typed).unwrap();
    }
}

const VOICE: &str = r#"
proc Voice:
  delegate fired(value: i32)
  sample:
    fired(1)
    out1 = 1.0
def step(voices, index: i32) -> f32:
  return voices[index]()
def forward(voices, index: i32) -> f32:
  return step(voices, index)
"#;

fn bank(name: &str) -> String {
    format!(
        r#"
proc {name}:
  init:
    voices: Voice[2]
    total: i32 = 0
  when voices[0].fired(value):
    total += value
  sample:
    out1 = f32(total)
"#
    )
}

fn analyze(source: &str) -> crate::TypedProgram {
    crate::analyze(onda_frontend::parse_program(source).expect("source parses"))
        .unwrap_or_else(|errors| panic!("source analyzes: {errors:?}"))
}

#[test]
fn dynamic_root_resource_recipes_share_implementations_across_bindings() {
    let typed = analyze(
        r#"
proc Voice:
  buffers:
    clip: f32
  sample:
    out1 = clip[0]
def step(voice) -> f32:
  return voice()
buffers:
  left: f32 {2}
  right: f32 {2}
init:
  selection: i32 = 0
  a: Voice[2] = Voice(clip = left[selection])
  b: Voice[2] = Voice(clip = right[selection])
sample:
  out1 = step(a[0]) + step(b[1])
"#,
    );
    assert_eq!(typed.processor_views.root_resource_recipes.len(), 1);
    assert_eq!(
        typed
            .processor_views
            .function_params
            .keys()
            .filter(|name| name.starts_with("step.__onda_mono__"))
            .count(),
        1
    );
    crate::lower_program_to_validated_mir(&typed)
        .expect("shared recipes lower against either binding");
}

#[test]
fn root_resource_recipe_overload_errors_preserve_source_spans() {
    let source = r#"proc Voice:
  buffers:
    clip: f32
  sample:
    out1 = clip[0]
def choose(index: i32) -> i32:
  return index
def choose(index: f32) -> i32:
  return 0
buffers:
  clips: f32 {2}
init:
  index: i32 = 0
  voices: Voice[2] = Voice(clip = clips[choose(true)])
sample:
  out1 = voices[0]()
"#;
    let errors = crate::analyze(onda_frontend::parse_program(source).unwrap()).unwrap_err();
    let overload_errors = errors
        .iter()
        .filter(|error| {
            error
                .message
                .contains("no matching overload for function 'choose'")
        })
        .collect::<Vec<_>>();
    assert!(!overload_errors.is_empty(), "{errors:?}");
    for error in overload_errors {
        assert_eq!((error.line, error.column), (14, 41), "{error:?}");
        assert_eq!((error.end_line, error.end_column), (14, 53), "{error:?}");
    }
}

#[test]
fn root_resource_helpers_follow_the_selected_compile_context() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-root-resource-context.onda"
    );
    for source in [
        source.to_owned(),
        source.replace("choose(index: i32)", "choose(index)"),
    ] {
        let typed = analyze(&source);
        crate::lower_program_to_validated_mir(&typed)
            .expect("deferred selectors use their specialized, materialized call targets");
    }
}

#[test]
fn resource_recipes_bind_generic_and_overloaded_selector_calls() {
    let composed = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-resource-selector-calls.onda"
    );
    crate::lower_program_to_validated_mir(&analyze(composed))
        .expect("composed selector calls bind against their declaration's state");
    let source = r#"
proc Voice:
  buffers:
    clip: f32
  sample:
    out1 = clip[0]
def step(voice) -> f32:
  return voice()
def choose(index):
  return index
buffers:
  clips: f32 {2}
init:
  voices: Voice[2] = Voice(clip = clips[choose(0)])
sample:
  out1 = voices[0]()
"#;
    let overloaded = source.replace(
        "def choose(index):\n  return index",
        "def choose(index: i32) -> i32:\n  return index\ndef choose(index: f32) -> i32:\n  return 1",
    );
    for source in [source, &overloaded] {
        for nested in [false, true] {
            let source = if nested {
                source
                    .replace(
                        "buffers:\n  clips: f32 {2}\ninit:\n  voices: Voice[2] = Voice(clip = clips[choose(0)])",
                        "proc Bank:\n  buffers:\n    clips: f32 {2}\n  init:\n    voices: Voice[2] = Voice(clip = clips[choose(0)])\n  sample:\n    out1 = 0.0\nbuffers:\n  clips: f32 {2}\ninit:\n  bank = Bank(clips = clips)",
                    )
                    .replace("out1 = voices[0]()", "out1 = step(bank.voices[0])")
            } else {
                source.to_owned()
            };
            crate::lower_program_to_validated_mir(&analyze(&source)).unwrap_or_else(|errors| {
                panic!("recipe calls should lower:\n{source}\n{errors:?}")
            });
        }
    }
}

#[test]
fn inherited_resource_helpers_follow_the_selected_compile_context() {
    let source = include_str!(
        "../../../../packages/onda_binaryen_web/test/fixtures/processor-inherited-resource-context.onda"
    );
    for source in [
        source.to_owned(),
        source.replace("out1 = step(bank.voice)", "out1 = bank.voice()"),
        source.replace("  out2 = clips[choose()][0]\n", ""),
    ] {
        let typed = analyze(&source);
        assert!(
            typed.defs.iter().any(|def| {
                def.name.starts_with("choose")
                    && def
                        .compile_context
                        .is_some_and(|context| context.options().sample_rate == 96_000.0)
            }),
            "inherited selectors need the sample context even without an explicit helper call"
        );
        crate::lower_program_to_validated_mir(&typed)
            .expect("inherited selectors use the selected context's call targets");
    }
}

#[test]
fn lexical_scalars_shadow_implicit_processor_members_in_authored_helpers() {
    let typed = analyze(
        r#"
proc Voice:
  sample:
    out1 = 9.0
def relay(value) -> f32:
  return value + 1.0
proc Bank:
  init:
    voice = Voice()
  sample:
    out1 = 0.0
def read(self: Bank, voice: f32) -> f32:
  return relay(voice)
init:
  bank = Bank()
sample:
  out1 = read(bank, 2.0)
"#,
    );
    crate::lower_program_to_validated_mir(&typed).expect("the lexical scalar remains a scalar");
    assert!(!typed
        .processor_views
        .function_params
        .keys()
        .any(|name| name.starts_with("relay.__onda_mono__")));
}

#[test]
fn scalar_parameters_shadow_implicit_processor_children_during_specialization() {
    let typed = analyze(
        r#"
proc Voice:
  sample:
    out1 = 9.0
def relay(value) -> f32:
  return value + 1.0
proc Bank:
  init:
    voice = Voice()
  def read(voice: f32) -> f32:
    return relay(voice)
  sample:
    out1 = read(2.0)
init:
  bank = Bank()
sample:
  out1 = bank()
"#,
    );
    crate::lower_program_to_validated_mir(&typed).expect("the lexical scalar remains a scalar");
    assert!(!typed
        .processor_views
        .function_params
        .keys()
        .any(|name| name.starts_with("relay.__onda_mono__")));
}

#[test]
fn init_view_alias_keeps_its_owner_shape_in_runtime_helpers() {
    let typed = analyze(&format!(
        r#"
{VOICE}
{}
def scalar_step(voice) -> f32:
  return voice()
init:
  bank = Bank()
  selector: i32 = 0
  selected = bank.voices[selector]
sample:
  out1 = scalar_step(selected)
"#,
        bank("Bank")
    ));
    let (_, params) = typed
        .processor_views
        .function_params
        .iter()
        .find(|(name, _)| name.starts_with("scalar_step.__onda_mono__"))
        .expect("specialized init view helper");
    assert_eq!(
        params["voice"].environments[0]
            .owner
            .as_ref()
            .unwrap()
            .owner,
        "Bank"
    );
}

#[test]
fn borrowed_helpers_share_owner_shape_and_retain_callback_adapters() {
    let typed = analyze(&format!(
        r#"
{VOICE}
{}
init:
  left = Bank()
  right = Bank()
sample:
  out1 = forward(left.voices, 0) + forward(right.voices, 1)
"#,
        bank("Bank")
    ));
    for helper in ["step", "forward"] {
        let variants = typed
            .processor_views
            .function_params
            .iter()
            .filter(|(name, _)| name.starts_with(&format!("{helper}.__onda_mono__")))
            .collect::<Vec<_>>();
        assert_eq!(variants.len(), 1, "one implementation per owner shape");
        let (function, params) = variants[0];
        let shape = &params["voices"];
        assert_eq!(shape.proc_name, "Voice");
        assert_eq!(shape.array_len(), Some(2));
        let projection = shape.environments[0].owner.as_ref().expect("owner route");
        assert_eq!(projection.owner, "Bank");
        assert_eq!(projection.edges[0].declaration, "voices");
        let target = typed
            .processor_views
            .operation_target_for(
                typed
                    .defs
                    .iter()
                    .find(|def| def.name == *function)
                    .unwrap()
                    .compile_context
                    .unwrap(),
                projection,
                &ProcessorOperation::Step,
            )
            .expect("callback adapter target");
        assert!(typed.defs.iter().any(|def| def.name == target));
    }
}

#[test]
fn branch_selected_owners_preserve_both_environment_shapes() {
    let typed = analyze(&format!(
        r#"
{VOICE}
{}
{}
init:
  left = LeftBank()
  right = RightBank()
  choose: bool = true
sample:
  if choose:
    selected = left.voices
  else:
    selected = right.voices
  out1 = forward(selected, 1)
"#,
        bank("LeftBank"),
        bank("RightBank"),
    ));
    let (_, params) = typed
        .processor_views
        .function_params
        .iter()
        .find(|(name, _)| name.starts_with("forward.__onda_mono__"))
        .expect("environment-specialized forwarding helper");
    let owners = params["voices"]
        .environments
        .iter()
        .map(|environment| environment.owner.as_ref().unwrap().owner.as_str())
        .collect::<Vec<_>>();
    assert_eq!(owners, ["LeftBank", "RightBank"]);
}

#[test]
fn selected_processor_parameters_retain_the_array_owner_environment() {
    let typed = analyze(&format!(
        r#"
{VOICE}
{}
def scalar_step(voice: Voice) -> f32:
  return voice()
init:
  bank = Bank()
sample:
  voice = bank.voices[1]
  out1 = scalar_step(voice)
"#,
        bank("Bank")
    ));
    let (_, params) = typed
        .processor_views
        .function_params
        .iter()
        .find(|(name, _)| name.starts_with("scalar_step.__onda_mono__"))
        .expect("selected processor specialization");
    let shape = &params["voice"];
    assert_eq!(shape.array_len(), None);
    assert_eq!(
        shape.environments[0]
            .owner
            .as_ref()
            .unwrap()
            .coordinate_arity(),
        1
    );
}

#[test]
fn environment_joins_are_canonical_and_keep_plain_alternatives() {
    let plain = ProcessorViewShape::plain("Voice".into(), Some(2));
    let mut owned = plain.clone();
    owned.environments.push(ProcessorEnvironmentShape {
        resource_sources: Vec::new(),
        state: None,
        owner_storage: None,
        owner: Some(ProcessorProjection {
            owner: "Bank".into(),
            edges: Vec::new(),
        }),
        resources: Vec::new(),
        outer: None,
    });
    assert_eq!(plain.join(&owned), owned.join(&plain));
    let joined = plain.join(&owned).unwrap();
    assert_eq!(joined.environments.len(), 2);
    assert!(joined
        .environments
        .iter()
        .any(|environment| environment.owner.is_none()));
}

#[test]
fn overload_selection_retains_explicit_processor_parameter_environments() {
    let typed = analyze(&format!(
        r#"
{VOICE}
{}
def overloaded(voice: Voice) -> f32:
  return voice()
def overloaded(value: f32) -> f32:
  return value + 10.0
init:
  bank = Bank()
sample:
  out1 = overloaded(bank.voices[1]) + overloaded(2.0)
"#,
        bank("Bank")
    ));
    let shapes = typed
        .processor_views
        .function_params
        .values()
        .filter_map(|params| params.get("voice"))
        .collect::<Vec<_>>();
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].proc_name, "Voice");
    assert_eq!(
        shapes[0].environments[0].owner.as_ref().unwrap().owner,
        "Bank"
    );
}
