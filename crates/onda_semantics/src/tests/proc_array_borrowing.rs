use super::*;

const VOICE: &str = r#"
proc Voice:
  params:
    tag: i32 = 0
  delegate fired(value: i32)
  sample:
    fired(tag)
    out1 = f32(tag)
"#;

fn bank_source(helpers: &str, step: &str, subscriptions: bool) -> String {
    let whens = if subscriptions {
        r#"
  when voices[0].fired(value):
    total += value
  when voices[1].fired(value):
    total += value * 100
"#
    } else {
        ""
    };
    format!(
        r#"
{VOICE}
{helpers}
proc Bank:
  init:
    voices: Voice[2] = [Voice(tag = 2), Voice(tag = 7)]
    selector: i32 = 0
    total: i32 = 0
{whens}
  sample:
    {step}
    selector = 1 - selector
    out1 = f32(total)
init:
  bank = Bank()
sample:
  out1 = bank()
"#
    )
}

fn assert_owner_context_preserved(source: &str) {
    let typed = analyze_source(source);
    lower_program_to_optimized_mir(&typed)
        .expect("complete borrowed views preserve their owner callback environment");
}

#[test]
fn nested_proc_array_helpers_preserve_owner_delegate_context() {
    let helpers = r#"
def step(voices, index: i32) -> f32:
  return voices[index]()
def forward(voices, index: i32) -> f32:
  return step(voices, index)
"#;
    assert_owner_context_preserved(&bank_source(helpers, "forward(voices, selector)", true));
}

#[test]
fn nested_proc_element_helpers_preserve_owner_delegate_context() {
    let helpers = r#"
def step(voice) -> f32:
  return voice()
"#;
    assert_owner_context_preserved(&bank_source(helpers, "step(voices[selector])", true));
}

#[test]
fn helpers_preserve_indexed_child_calls_in_a_borrowed_owners_context() {
    let source = bank_source("", "voices[selector]()", true).replace(
        "init:\n  bank = Bank()\nsample:\n  out1 = bank()",
        r#"def child_step(bank: Bank, index: i32) -> f32:
  return bank.voices[index]()
init:
  bank = Bank()
sample:
  child_step(bank, 0)
  out1 = bank()"#,
    );
    assert_owner_context_preserved(&source);
}

#[test]
fn nested_proc_array_helpers_keep_context_free_delegate_borrowing() {
    let helpers = r#"
def step(voices, index: i32) -> f32:
  return voices[index]()
def forward(voices, index: i32) -> f32:
  return step(voices, index)
"#;
    let typed = analyze_source(&bank_source(helpers, "forward(voices, selector)", false));
    lower_program_to_optimized_mir(&typed)
        .expect("a delegate declaration alone does not require owner context");
}

#[test]
fn top_level_proc_array_helpers_keep_global_delegate_routes() {
    let source = format!(
        r#"
{VOICE}
def step(voices, index: i32) -> f32:
  return voices[index]()
def forward(voices, index: i32) -> f32:
  return step(voices, index)
init:
  voices: Voice[2] = [Voice(tag = 2), Voice(tag = 7)]
  total: i32 = 0
when voices[0].fired(value):
  total += value
when voices[1].fired(value):
  total += value * 100
sample:
  forward(voices, 0)
  out1 = f32(total)
"#
    );
    lower_program_to_optimized_mir(&analyze_source(&source))
        .expect("top-level delegate routes need no borrowed owner context");
}
