use super::*;

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
        assert_eq!(shape.len, Some(2));
        let projection = shape.environments[0].owner.as_ref().expect("owner route");
        assert_eq!(projection.owner, "Bank");
        assert_eq!(projection.edges[0].declaration, "voices");
        let target = typed
            .processor_views
            .operation_target_for(function, projection, &ProcessorOperation::Step)
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
    assert_eq!(shape.len, None);
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
