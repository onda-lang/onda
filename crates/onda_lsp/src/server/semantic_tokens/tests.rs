use std::fs;
use std::path::PathBuf;

use super::*;

fn find_tokens_named<'a>(
    tokens: &'a [SemanticToken],
    source: &str,
    name: &str,
) -> Vec<&'a SemanticToken> {
    tokens
        .iter()
        .filter(|t| {
            let line = t.line as usize;
            let col = t.start as usize;
            let len = t.length as usize;
            source.lines().nth(line).and_then(|l| l.get(col..col + len)) == Some(name)
        })
        .collect()
}

fn has_token(
    tokens: &[SemanticToken],
    line: u32,
    start: u32,
    length: u32,
    token_type: u32,
) -> bool {
    tokens.iter().any(|token| {
        token.line == line
            && token.start == start
            && token.length == length
            && token.token_type == token_type
    })
}

fn token_type_at_text_on_line(
    tokens: &[SemanticToken],
    source: &str,
    line_no: usize,
    needle: &str,
) -> Option<u32> {
    token_type_at_nth_text_on_line(tokens, source, line_no, needle, 0)
}

fn token_type_at_nth_text_on_line(
    tokens: &[SemanticToken],
    source: &str,
    line_no: usize,
    needle: &str,
    occurrence: usize,
) -> Option<u32> {
    let line = source
        .lines()
        .nth(line_no)
        .expect("expected source line for token lookup");
    let start = nth_match_start(line, needle, occurrence).expect("expected token text on line");
    tokens
        .iter()
        .find(|t| {
            t.line as usize == line_no
                && t.start as usize == start
                && t.length as usize == needle.len()
        })
        .map(|t| t.token_type)
}

fn nth_match_start(line: &str, needle: &str, occurrence: usize) -> Option<usize> {
    let mut offset = 0;
    for current in 0..=occurrence {
        let found = line.get(offset..)?.find(needle)? + offset;
        if current == occurrence {
            return Some(found);
        }
        offset = found + needle.len();
    }
    None
}

fn nth_line_containing(source: &str, needle: &str, occurrence: usize) -> usize {
    source
        .lines()
        .enumerate()
        .filter(|(_, line)| line.contains(needle))
        .nth(occurrence)
        .map(|(line_no, _)| line_no)
        .unwrap_or_else(|| panic!("expected occurrence {occurrence} of '{needle}'"))
}

fn line_containing(source: &str, needle: &str) -> usize {
    nth_line_containing(source, needle, 0)
}

fn repo_source(rel: &str) -> (PathBuf, String) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(rel);
    let source = fs::read_to_string(&path).expect("source should be readable");
    (path, source)
}

#[test]
fn reserved_words_include_singular_event_keyword() {
    assert!(is_reserved_word("event"));
    assert!(is_reserved_word("events"));
    assert!(is_reserved_word("delegate"));
    assert!(is_reserved_word("delegates"));
    assert!(is_reserved_word("when"));
    assert!(is_reserved_word("print"));
    assert!(is_reserved_word("private"));
    assert!(is_reserved_word("pin"));
}

#[test]
fn semantic_tokens_highlight_print_like_a_function_call() {
    let source = "init:\n  print(1, true)\nsample:\n  out1 = 0.0\n";
    for tokens in [
        semantic_tokens_for_document(source, None),
        semantic_tokens_for_document_source_only(source, None),
    ] {
        assert_eq!(
            token_type_at_text_on_line(&tokens, source, 1, "print"),
            Some(SEMANTIC_TOKEN_TYPE_FUNCTION),
        );
    }
}

#[test]
fn semantic_tokens_distinguish_events_and_delegates() {
    let source = concat!(
        "delegate finished(reason: i32)\n",
        "event trigger():\n",
        "  finished(7)\n",
        "when finished(reason):\n",
        "  out1 = f32(reason)\n",
        "sample:\n",
        "  out1 = 0.0\n",
    );
    let tokens = semantic_tokens_for_document(source, None);
    for line in [0, 2, 3] {
        assert_eq!(
            token_type_at_text_on_line(&tokens, source, line, "finished"),
            Some(SEMANTIC_TOKEN_TYPE_DELEGATE),
        );
    }
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 1, "trigger"),
        Some(SEMANTIC_TOKEN_TYPE_EVENT),
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 4, "reason"),
        Some(SEMANTIC_TOKEN_TYPE_PARAMETER),
    );
}

#[test]
fn semantic_tokens_mark_proc_delegate_calls_as_delegates() {
    let source = concat!(
        "proc Voice:\n",
        "  delegate finished(value: i64)\n",
        "  event trigger():\n",
        "    finished(0)\n",
        "  sample:\n",
        "    finished(0)\n",
        "    out1 = 0.0\n",
    );
    for tokens in [
        semantic_tokens_for_document(source, None),
        semantic_tokens_for_document_source_only(source, None),
    ] {
        for line in [1, 3, 5] {
            assert_eq!(
                token_type_at_text_on_line(&tokens, source, line, "finished"),
                Some(SEMANTIC_TOKEN_TYPE_DELEGATE),
            );
        }
        assert_eq!(
            token_type_at_text_on_line(&tokens, source, 1, "value"),
            Some(SEMANTIC_TOKEN_TYPE_PARAMETER),
        );
    }
}

#[test]
fn semantic_tokens_mark_child_when_targets_as_delegates() {
    let source = concat!(
        "proc Child:\n",
        "  delegate finished(reason: i32)\n",
        "  sample:\n",
        "    out1 = 0.0\n",
        "init:\n",
        "  child = Child()\n",
        "  seen = 0\n",
        "when child.finished(reason):\n",
        "  seen = reason\n",
        "sample:\n",
        "  out1 = child()\n",
    );
    let tokens = semantic_tokens_for_document(source, None);
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 7, "finished"),
        Some(SEMANTIC_TOKEN_TYPE_DELEGATE),
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 8, "reason"),
        Some(SEMANTIC_TOKEN_TYPE_PARAMETER),
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 8, "seen"),
        Some(SEMANTIC_TOKEN_TYPE_STATE),
    );
}

#[test]
fn source_fallback_preserves_when_bindings_and_owner_state() {
    let source = concat!(
        "outs:\n",
        "  out1\n",
        "init:\n",
        "  seen = 0\n",
        "delegate finished(reason: i32)\n",
        "when finished(reason):\n",
        "  seen = reason\n",
        "  broken =\n",
    );
    assert!(parse_program(source).is_err());
    let tokens = semantic_tokens_for_document(source, None);
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 6, "seen"),
        Some(SEMANTIC_TOKEN_TYPE_STATE),
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 6, "reason"),
        Some(SEMANTIC_TOKEN_TYPE_PARAMETER),
    );
}

#[test]
fn source_fallback_preserves_proc_when_bindings_and_owner_entries() {
    let source = concat!(
        "proc Voice:\n",
        "  outs:\n",
        "    out1\n",
        "  init:\n",
        "    seen = 0\n",
        "  delegate finished(reason: i32)\n",
        "  when finished(reason):\n",
        "    seen = reason\n",
        "    out1 = f32(reason)\n",
        "    broken =\n",
    );
    assert!(parse_program(source).is_err());
    let tokens = semantic_tokens_for_document(source, None);
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 7, "seen"),
        Some(SEMANTIC_TOKEN_TYPE_STATE),
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 8, "out1"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 8, "reason"),
        Some(SEMANTIC_TOKEN_TYPE_PARAMETER),
    );
}

#[test]
fn builtin_host_sample_rate_alias_is_semantic_constant() {
    let source = "sample:\n  out1 = HOST_SR\n";
    let tokens = semantic_tokens_for_document(source, None);
    let host_tokens = find_tokens_named(&tokens, source, "HOST_SR");

    assert!(
        host_tokens
            .iter()
            .any(|token| token.token_type == SEMANTIC_TOKEN_TYPE_ENUM_MEMBER),
        "HOST_SR should use the shared builtin constant catalog: {host_tokens:?}"
    );
}

#[test]
fn source_declaration_scope_excludes_scoped_symbols() {
    let source = concat!(
        "proc Env:\n",
        "  init:\n",
        "    phase = 0.0\n",
        "\n",
        "  event note_on(freq = 440.0):\n",
        "    local = freq\n",
        "    phase = local\n",
    );
    let mut scope = SemanticScope::default();
    collect_source_declaration_symbols(source, &mut scope);

    assert_eq!(
        scope.imported_token_type_for("Env"),
        Some(SEMANTIC_TOKEN_TYPE_TYPE)
    );
    assert_eq!(scope.token_type_for_source_fallback("note_on"), None);
    assert_eq!(scope.token_type_for_source_fallback("freq"), None);
    assert_eq!(scope.token_type_for_source_fallback("local"), None);
    assert_eq!(scope.token_type_for_source_fallback("phase"), None);
}

#[test]
fn collect_const_names_finds_declarations() {
    let source = "const GAIN = 0.5\nsample:\n  const MIX = GAIN\n";
    let names = collect_const_names(source);
    assert!(names.contains("GAIN"));
    assert!(names.contains("MIX"));
    assert_eq!(names.len(), 2);
}

#[test]
fn semantic_tokens_do_not_expose_top_level_runtime_via_source_scope_fallback() {
    let source = concat!(
        "params:\n",
        "  gain = 1.0\n",
        "outs:\n",
        "  out1\n",
        "init:\n",
        "  phase = 0.0\n",
        "\n",
        "def helper(x):\n",
        "  y = x + 1.0\n",
        "  return y\n",
        "\n",
        "sample:\n",
        "  out1 = phase * gain\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let gain_tokens = find_tokens_named(&tokens, source, "gain");
    assert!(
        !gain_tokens
            .iter()
            .any(|t| t.line == 8 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "top-level def should not see runtime param via source fallback: {gain_tokens:?}"
    );
    assert!(
        gain_tokens
            .iter()
            .any(|t| t.line == 12 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "sample should still see top-level runtime param: {gain_tokens:?}"
    );

    let phase_tokens = find_tokens_named(&tokens, source, "phase");
    assert!(
        !phase_tokens
            .iter()
            .any(|t| t.line == 8 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "top-level def should not see runtime state via source fallback: {phase_tokens:?}"
    );
    assert!(
        phase_tokens
            .iter()
            .any(|t| t.line == 12 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "sample should still see top-level runtime state: {phase_tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_const_declarations_and_uses() {
    let source = "const GAIN = 0.5\nsample:\n  out1 = GAIN\n";
    let tokens = semantic_tokens_for_document(source, None);
    assert!(
        tokens
            .iter()
            .any(|token| token.token_type == SEMANTIC_TOKEN_TYPE_ENUM_MEMBER),
        "tokens: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|token| token.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "tokens: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_proc_ports_and_local_variables() {
    let source = "proc Mix:\n  ins:\n    dry\n    fb\n\n  sample:\n    out1 = (dry + fb) * 0.5\n\ninit:\n  mix = Mix()\n\ngraph:\n  in1 >> mix.dry\n";
    let tokens = semantic_tokens_for_document(source, None);

    assert!(has_token(&tokens, 2, 4, 3, SEMANTIC_TOKEN_TYPE_PORT));
    assert!(has_token(&tokens, 3, 4, 2, SEMANTIC_TOKEN_TYPE_PORT));
    assert!(has_token(&tokens, 6, 12, 3, SEMANTIC_TOKEN_TYPE_PORT));
    assert!(has_token(&tokens, 6, 18, 2, SEMANTIC_TOKEN_TYPE_PORT));
    assert!(has_token(&tokens, 9, 2, 3, SEMANTIC_TOKEN_TYPE_STATE));
    assert!(
        has_token(&tokens, 12, 9, 3, SEMANTIC_TOKEN_TYPE_STATE),
        "tokens: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_local_variable_uses_in_sample_blocks() {
    let source = "proc Saturate:\n  sample:\n    x = in1\n    out1 = x - (x * x * x) * 0.1\n";
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        tokens
            .iter()
            .any(|token| token.length == 1 && token.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
        "tokens: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_onda_params_as_ports() {
    let source = "params:\n  gain = 0.5\nsample:\n  out1 = gain\n";
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 1, 2, 4, SEMANTIC_TOKEN_TYPE_PORT),
        "tokens: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 3, 9, 4, SEMANTIC_TOKEN_TYPE_PORT),
        "tokens: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_runtime_symbols_shadow_namespace_names_in_std_filter() {
    let (path, source) = repo_source("stdlib/std/filter.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));
    let mode_namespace = line_containing(&source, "  namespace mode:");
    let one_pole_param = line_containing(&source, "mode: i32 = mode::ONE_POLE_LOWPASS");
    let one_pole_read = line_containing(&source, "mode == mode::ONE_POLE_HIGHPASS");
    let svf_param = line_containing(&source, "mode: i32 = mode::SVF_LOWPASS");
    let svf_read = line_containing(&source, "mode == mode::SVF_LOWPASS");

    assert_eq!(
        token_type_at_text_on_line(&tokens, &source, mode_namespace, "mode"),
        Some(SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "mode namespace declaration should remain a namespace"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, &source, one_pole_param, "mode"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "OnePole param declaration should be a port"
    );
    assert_eq!(
        token_type_at_nth_text_on_line(&tokens, &source, one_pole_param, "mode", 1),
        Some(SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "OnePole default should keep mode:: as namespace"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, &source, one_pole_read, "mode"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "OnePole mode read should be a port"
    );
    assert_eq!(
        token_type_at_nth_text_on_line(&tokens, &source, one_pole_read, "mode", 1),
        Some(SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "OnePole qualified mode:: use should be namespace"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, &source, svf_param, "mode"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "Svf param declaration should be a port"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, &source, svf_read, "mode"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "Svf mode read should be a port"
    );
    assert_eq!(
        token_type_at_nth_text_on_line(&tokens, &source, svf_read, "mode", 1),
        Some(SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "Svf qualified mode:: use should be namespace"
    );
}

#[test]
fn semantic_tokens_do_not_leak_event_locals_into_sample_blocks() {
    let source = concat!(
        "import std/osc\n",
        "\n",
        "outs:\n",
        "  out1\n",
        "\n",
        "init:\n",
        "  freq_state = 220.0\n",
        "  amp_state = 0.0\n",
        "  gate = false\n",
        "  osc = std::osc::Sine(freq = 220.0)\n",
        "\n",
        "events:\n",
        "  note_on(freq_hz = 440.0, amp = 1.0):\n",
        "    freq_state = freq_hz\n",
        "    amp_state = amp\n",
        "    gate = true\n",
        "\n",
        "  note_off():\n",
        "    gate = false\n",
        "\n",
        "sample:\n",
        "  osc.freq = freq_state\n",
        "\n",
        "  out1 = 0.0\n",
        "  if (gate):\n",
        "    out1 = osc() * amp\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let amp_tokens = find_tokens_named(&tokens, source, "amp");
    assert!(
        amp_tokens
            .iter()
            .any(|token| token.line == 12 && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "event param declaration should still be highlighted: {amp_tokens:?}"
    );
    assert!(
        amp_tokens
            .iter()
            .any(|token| token.line == 14 && token.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "event param use should still be highlighted: {amp_tokens:?}"
    );
    assert!(
        !amp_tokens.iter().any(|token| token.line == 25),
        "sample block should not highlight event-local amp: {amp_tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_singular_event_symbols_in_incomplete_file() {
    let source = concat!(
        "init:\n",
        "  gate = 0.0\n",
        "\n",
        "event note_on(freq = 440.0):\n",
        "  gate = freq +\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let gate_tokens = find_tokens_named(&tokens, source, "gate");
    assert!(
        gate_tokens
            .iter()
            .any(|t| t.line == 4 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "singular event body should still see init state in fallback: {gate_tokens:?}"
    );
    let freq_tokens = find_tokens_named(&tokens, source, "freq");
    assert!(
        freq_tokens
            .iter()
            .any(|t| t.line == 4 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "singular event body should highlight params in fallback: {freq_tokens:?}"
    );
}

#[test]
fn semantic_tokens_do_not_expose_top_level_runtime_scope_inside_top_level_defs() {
    let source = concat!(
        "outs:\n",
        "  out1\n",
        "params:\n",
        "  gain = 1.0\n",
        "buffers:\n",
        "  buf: f32[]\n",
        "init:\n",
        "  state = 0.0\n",
        "\n",
        "def leak(x):\n",
        "  y = x + in1 + out1 + gain + state + buf[0]\n",
        "  return y\n",
        "\n",
        "sample:\n",
        "  out1 = state + gain\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let x_tokens = find_tokens_named(&tokens, source, "x");
    assert!(
        x_tokens
            .iter()
            .any(|t| t.line == 9 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "top-level def param x should be highlighted inside def body"
    );
    let y_tokens = find_tokens_named(&tokens, source, "y");
    assert!(
        y_tokens
            .iter()
            .any(|t| t.line == 10 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
        "top-level def local y should be highlighted inside def body"
    );
    assert!(
        !find_tokens_named(&tokens, source, "in1")
            .iter()
            .any(|t| t.line == 10),
        "implicit top-level input should not be highlighted inside top-level def"
    );
    for name in ["out1", "gain", "state", "buf"] {
        assert!(
            !find_tokens_named(&tokens, source, name)
                .iter()
                .any(|t| t.line == 10),
            "{name} should not be highlighted inside top-level def"
        );
    }
    assert!(
        find_tokens_named(&tokens, source, "state")
            .iter()
            .any(|t| t.line == 14 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "top-level state should still be highlighted in sample"
    );
    assert!(
        find_tokens_named(&tokens, source, "gain")
            .iter()
            .any(|t| t.line == 14 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "top-level param should still be highlighted in sample"
    );
}

#[test]
fn semantic_tokens_do_not_bleed_between_top_level_defs() {
    let source = concat!(
        "def first(x):\n",
        "  local = x\n",
        "  return local\n",
        "\n",
        "def second():\n",
        "  return x + local\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        find_tokens_named(&tokens, source, "x")
            .iter()
            .any(|t| t.line == 1 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "first def param x should be highlighted in first body"
    );
    assert!(
        find_tokens_named(&tokens, source, "local")
            .iter()
            .any(|t| t.line == 2 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
        "first def local should be highlighted in first body"
    );
    assert!(
        !find_tokens_named(&tokens, source, "x")
            .iter()
            .any(|t| t.line == 5),
        "first def param should not bleed into second def"
    );
    assert!(
        !find_tokens_named(&tokens, source, "local")
            .iter()
            .any(|t| t.line == 5),
        "first def local should not bleed into second def"
    );
}

#[test]
fn semantic_tokens_do_not_bleed_between_struct_methods() {
    let source = concat!(
        "struct Box:\n",
        "  value: f32 = 0.0\n",
        "\n",
        "  def set(self, x):\n",
        "    tmp = x\n",
        "    self.value = tmp\n",
        "\n",
        "  def get(self):\n",
        "    return x + tmp + self.value\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        find_tokens_named(&tokens, source, "x")
            .iter()
            .any(|t| t.line == 4 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "method param x should be highlighted in set body"
    );
    assert!(
        find_tokens_named(&tokens, source, "tmp")
            .iter()
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
        "method local tmp should be highlighted in set body"
    );
    assert!(
        !find_tokens_named(&tokens, source, "x")
            .iter()
            .any(|t| t.line == 8),
        "set param x should not bleed into get"
    );
    assert!(
        !find_tokens_named(&tokens, source, "tmp")
            .iter()
            .any(|t| t.line == 8),
        "set local tmp should not bleed into get"
    );
}

#[test]
fn semantic_tokens_mark_self_and_self_fields_distinctly() {
    let source = concat!(
        "struct Box:\n",
        "  value: f32 = 0.0\n",
        "\n",
        "  def set(self, x):\n",
        "    self.value = x\n",
        "    tmp = self.value\n",
    );
    let tokens = semantic_tokens_for_document(source, None);
    let source_only_tokens = semantic_tokens_for_document_source_only(source, None);

    assert!(
        has_token(&tokens, 1, 2, 5, SEMANTIC_TOKEN_TYPE_STATE),
        "struct field declaration should use the state token: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 3, 10, 4, SEMANTIC_TOKEN_TYPE_PORT),
        "self in method params should use the port token: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 4, 4, 4, SEMANTIC_TOKEN_TYPE_PORT),
        "self receiver should use the port token: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 4, 9, 5, SEMANTIC_TOKEN_TYPE_STATE),
        "self.value field write should use the state token: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 5, 15, 5, SEMANTIC_TOKEN_TYPE_STATE),
        "self.value field read should use the state token: {tokens:?}"
    );
    assert!(
        has_token(&source_only_tokens, 1, 2, 5, SEMANTIC_TOKEN_TYPE_STATE),
        "source-only struct field declaration should use the state token: {source_only_tokens:?}"
    );
    assert!(
        has_token(&source_only_tokens, 4, 9, 5, SEMANTIC_TOKEN_TYPE_STATE),
        "source-only self.value should use the state token: {source_only_tokens:?}"
    );
}

#[test]
fn semantic_tokens_keep_proc_runtime_scope_but_not_proc_local_def_locals() {
    let source = concat!(
        "proc Reader:\n",
        "  ins:\n",
        "    in1\n",
        "  outs:\n",
        "    out1\n",
        "  params:\n",
        "    gain = 1.0\n",
        "  buffers:\n",
        "    line: buffer<f32>\n",
        "  init:\n",
        "    state = 0.0\n",
        "\n",
        "  def helper(x):\n",
        "    tmp = x + in1 + gain + state + line[0]\n",
        "    return tmp\n",
        "\n",
        "  sample:\n",
        "    out1 = helper(0.0)\n",
        "    out1 = tmp\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        find_tokens_named(&tokens, source, "x")
            .iter()
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "proc-local def param should be highlighted"
    );
    assert!(
        find_tokens_named(&tokens, source, "tmp")
            .iter()
            .any(|t| t.line == 14 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
        "proc-local def local should be highlighted"
    );
    assert!(
        find_tokens_named(&tokens, source, "in1")
            .iter()
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "proc input should be highlighted inside proc-local def"
    );
    assert!(
        find_tokens_named(&tokens, source, "gain")
            .iter()
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "proc param should be highlighted inside proc-local def"
    );
    assert!(
        find_tokens_named(&tokens, source, "state")
            .iter()
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "proc state should be highlighted inside proc-local def"
    );
    assert!(
        find_tokens_named(&tokens, source, "line")
            .iter()
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "proc buffer should be highlighted inside proc-local def"
    );
    assert!(
        !find_tokens_named(&tokens, source, "tmp")
            .iter()
            .any(|t| t.line == 18),
        "proc-local def local should not bleed into sample"
    );
}

#[test]
fn semantic_tokens_mark_init_vars_in_proc_defs_events_and_sample() {
    let source = concat!(
        "proc Conv:\n",
        "  init:\n",
        "    delay: f32[100]\n",
        "    write: i32 = 0\n",
        "\n",
        "  def clear():\n",
        "    delay[:] = 0.0\n",
        "    write = 0\n",
        "\n",
        "  events:\n",
        "    reset():\n",
        "      clear()\n",
        "      write = 0\n",
        "\n",
        "  sample:\n",
        "    delay[write] = in1\n",
        "    write = write + 1\n",
        "    out1 = delay[0]\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        tokens
            .iter()
            .any(|t| t.line == 6 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
        "delay in def: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 7 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
        "write in def: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 12 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
        "write in events: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 15 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
        "delay in sample: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 15 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
        "write in sample delay[write]: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_work_inside_namespace() {
    let source = concat!(
        "namespace test::ns:\n",
        "  const SIZE = 10\n",
        "\n",
        "  proc Foo:\n",
        "    init:\n",
        "      buf: f32[SIZE]\n",
        "      pos: i32 = 0\n",
        "\n",
        "    def clear():\n",
        "      buf[:] = 0.0\n",
        "      pos = 0\n",
        "\n",
        "    sample:\n",
        "      buf[pos] = in1\n",
        "      pos = pos + 1\n",
        "      out1 = buf[0]\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        tokens
            .iter()
            .any(|t| t.line == 9 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 3),
        "buf in def: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 3),
        "pos in sample: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_use_namespace_targets_as_namespaces() {
    let source = concat!(
        "namespace sc:\n",
        "  namespace SinOsc:\n",
        "    const A = 1\n",
        "\n",
        "use sc\n",
        "use sc::SinOsc\n",
        "use sc::SinOsc as Sine\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 0, 10, 2, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "namespace declaration should mark sc as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 4, 4, 2, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "use target should mark sc as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 5, 8, 6, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "qualified use target should mark SinOsc as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 6, 18, 4, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "use alias should be highlighted as a namespace: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_use_namespace_targets_in_source_fallback() {
    let source = concat!(
        "use sc\n",
        "use dsp::Osc\n",
        "pub use fx::Delay\n",
        "use ugens::Saw as Saw\n",
    );
    let tokens = semantic_tokens_for_document_source_only(source, None);

    assert!(
        has_token(&tokens, 0, 4, 2, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "source fallback should mark single use target as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 1, 4, 3, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "source fallback should mark qualified use root as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 1, 9, 3, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "source fallback should mark qualified use leaf as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 2, 8, 2, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "source fallback should mark pub use root as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 3, 4, 5, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "source fallback should mark aliased use target root as namespace: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 3, 18, 3, SEMANTIC_TOKEN_TYPE_NAMESPACE),
        "source fallback should mark alias name as namespace: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_proc_section_declarations() {
    let source = concat!(
        "proc Loop:\n",
        "  ins:\n",
        "    input\n",
        "  outs:\n",
        "    wet\n",
        "  params:\n",
        "    rate = 1.0\n",
        "  buffers:\n",
        "    buf: f32[]\n",
        "  init:\n",
        "    pos = 0.0\n",
        "  sample:\n",
        "    wet = input\n",
        "    pos = pos + rate\n",
        "    out1 = buf.readL(0, pos)\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 2, 4, 5, SEMANTIC_TOKEN_TYPE_PORT),
        "input decl: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 4, 4, 3, SEMANTIC_TOKEN_TYPE_PORT),
        "wet decl: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 6, 4, 4, SEMANTIC_TOKEN_TYPE_PORT),
        "rate decl: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 8, 4, 3, SEMANTIC_TOKEN_TYPE_PORT),
        "buf decl: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 10, 4, 3, SEMANTIC_TOKEN_TYPE_STATE),
        "pos decl: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_keep_private_params_registered() {
    let source = concat!(
        "proc Filter:\n",
        "  params:\n",
        "    private cutoff = 1000.0\n",
        "    private coeffs: f32[2] = [0.5, 0.25]\n",
        "  sample:\n",
        "    out1 = cutoff + coeffs[0]\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 5, 11, 6, SEMANTIC_TOKEN_TYPE_PORT),
        "proc code should still resolve the private scalar param name: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 5, 20, 6, SEMANTIC_TOKEN_TYPE_PORT),
        "proc code should still resolve the private array param name: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_register_private_param_in_incomplete_proc_params() {
    let source = concat!(
        "proc Filter:\n",
        "  params:\n",
        "    private cutoff =\n",
        "  sample:\n",
        "    out1 = cutoff + private\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 4, 11, 6, SEMANTIC_TOKEN_TYPE_PORT),
        "source fallback should register the private param name, not 'private': {tokens:?}"
    );
}

#[test]
fn semantic_tokens_do_not_register_top_level_private_param_fallback() {
    let source = concat!(
        "params:\n",
        "  private gain = 1.0\n",
        "sample:\n",
        "  out1 = gain\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        !has_token(&tokens, 3, 9, 4, SEMANTIC_TOKEN_TYPE_PARAMETER),
        "invalid top-level private params should not register a fallback param: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_register_pinned_state_in_incomplete_init() {
    let source = concat!(
        "init:\n",
        "  pin prepared =\n",
        "sample:\n",
        "  out1 = prepared\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 3, 9, 8, SEMANTIC_TOKEN_TYPE_STATE),
        "source fallback should register the pinned state name, not 'pin': {tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_proc_output_in_else_branch() {
    let source = concat!(
        "proc Svf:\n",
        "  params:\n",
        "    mode: i32 = 0\n",
        "\n",
        "  sample:\n",
        "    if (mode <= 0):\n",
        "      out1 = 0.0\n",
        "    elif (mode == 1):\n",
        "      out1 = 1.0\n",
        "    else:\n",
        "      out1 = 2.0\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 10, 6, 4, SEMANTIC_TOKEN_TYPE_PORT),
        "else-branch proc output should be highlighted as port: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_do_not_mark_nested_init_locals_as_state() {
    let source = concat!(
        "outs:\n",
        "  out1\n",
        "init:\n",
        "  voices = 0.0\n",
        "  for i in 0..4:\n",
        "    h = f32(i + 1)\n",
        "    voices = h\n",
        "sample:\n",
        "  out1 = voices\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let h_tokens = find_tokens_named(&tokens, source, "h");
    assert!(
        h_tokens
            .iter()
            .any(|t| t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
        "nested init local should be highlighted as local variable: {h_tokens:?}"
    );
    assert!(
        h_tokens
            .iter()
            .all(|t| t.token_type != SEMANTIC_TOKEN_TYPE_STATE),
        "nested init local should not be highlighted as state: {h_tokens:?}"
    );

    let voices_tokens = find_tokens_named(&tokens, source, "voices");
    assert!(
        voices_tokens
            .iter()
            .any(|t| t.line == 3 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "top-level init assignment should still be state: {voices_tokens:?}"
    );
    assert!(
        voices_tokens
            .iter()
            .any(|t| t.line == 8 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "sample use should still see top-level init state: {voices_tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_named_argument_labels_as_state_in_calls() {
    let source = concat!(
        "def foo(a = 0.0, b = 0.0):\n",
        "  return a + b\n",
        "\n",
        "outs:\n",
        "  out1\n",
        "\n",
        "event bang():\n",
        "  out1 = foo(a = 1.0, b = 2.0)\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 7, 13, 1, SEMANTIC_TOKEN_TYPE_STATE),
        "named arg label 'a =' should use the state token: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 7, 22, 1, SEMANTIC_TOKEN_TYPE_STATE),
        "named arg label 'b =' should use the state token: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_distinguish_parameter_domain_fields_and_scale_values() {
    let source = concat!(
        "params:\n",
        "  cutoff = 440.0 {min = 20, max = 20000, scale = log, unit = \"Hz\"}\n",
        "  gain = 1.0 {0, 2, linear}\n",
        "outs:\n",
        "  out1\n",
        "sample:\n",
        "  out1 = log(gain)\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    for field in ["min", "max", "scale", "unit"] {
        assert_eq!(
            token_type_at_text_on_line(&tokens, source, 1, field),
            Some(SEMANTIC_TOKEN_TYPE_STATE),
            "parameter domain field '{field}' should match named-argument highlighting"
        );
    }
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 1, "log"),
        Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER)
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 2, "linear"),
        Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER)
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 6, "log"),
        Some(SEMANTIC_TOKEN_TYPE_FUNCTION),
        "log should remain a function in expression context"
    );
}

#[test]
fn semantic_tokens_distinguish_binding_range_fields_and_modes() {
    let source = concat!(
        "sample:\n",
        "  wrapped: i32 = 0 {range = 0..16, mode = wrap}\n",
        "  clamped: i32 = 0 {16, clamp}\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    for field in ["range", "mode"] {
        assert_eq!(
            token_type_at_text_on_line(&tokens, source, 1, field),
            Some(SEMANTIC_TOKEN_TYPE_STATE),
            "binding range field '{field}' should match named-field highlighting"
        );
    }
    assert_eq!(
        token_type_at_nth_text_on_line(&tokens, source, 1, "wrap", 1),
        Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER)
    );
    assert_eq!(
        token_type_at_nth_text_on_line(&tokens, source, 2, "clamp", 1),
        Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER)
    );
}

#[test]
fn semantic_tokens_mark_block_pre_state_in_nested_sample() {
    let source = concat!(
        "outs:\n",
        "  out1\n",
        "block:\n",
        "  acc = 0.0\n",
        "  sample:\n",
        "    out1 = acc\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let acc_tokens = find_tokens_named(&tokens, source, "acc");
    assert!(
        acc_tokens
            .iter()
            .any(|t| t.line == 3 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "block-pre declaration should be highlighted as state: {acc_tokens:?}"
    );
    assert!(
        acc_tokens
            .iter()
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "nested sample should see block-pre carried state: {acc_tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_block_symbols_in_incomplete_file() {
    let source = concat!(
        "outs:\n",
        "  out1\n",
        "block:\n",
        "  acc = 0.0\n",
        "  sample:\n",
        "    out1 = acc +\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let acc_tokens = find_tokens_named(&tokens, source, "acc");
    assert!(
        acc_tokens
            .iter()
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "incomplete block fallback should carry block-pre state into nested sample: {acc_tokens:?}"
    );
    let out_tokens = find_tokens_named(&tokens, source, "out1");
    assert!(
        out_tokens
            .iter()
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "incomplete block fallback should still highlight outputs: {out_tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_graph_symbols_in_incomplete_file() {
    let source = concat!(
        "params:\n",
        "  gain = 1.0\n",
        "outs:\n",
        "  out1\n",
        "graph:\n",
        "  in1 >> out1 + gain\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let in_tokens = find_tokens_named(&tokens, source, "in1");
    assert!(
        in_tokens
            .iter()
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "incomplete graph fallback should keep implicit inputs as ports: {in_tokens:?}"
    );
    let out_tokens = find_tokens_named(&tokens, source, "out1");
    assert!(
        out_tokens
            .iter()
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "incomplete graph fallback should keep declared outputs as ports: {out_tokens:?}"
    );
    let gain_tokens = find_tokens_named(&tokens, source, "gain");
    assert!(
        gain_tokens
            .iter()
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "incomplete graph fallback should keep params visible: {gain_tokens:?}"
    );
}

#[test]
fn semantic_tokens_keep_graph_delay_identifiers_semantic() {
    let source = concat!(
        "const N = 1\n",
        "outs:\n",
        "  out1\n",
        "graph:\n",
        "  in1 >>[N] out1\n",
        "  in1 >>[2] out1\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 0, "N"),
        Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER),
        "const declaration should keep const highlighting"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 4, "N"),
        Some(SEMANTIC_TOKEN_TYPE_ENUM_MEMBER),
        "graph delay const should keep const highlighting"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 5, "2"),
        Some(SEMANTIC_TOKEN_TYPE_NUMBER),
        "graph delay literal should be supplied as a number token"
    );
}

#[test]
fn semantic_tokens_mark_graph_instance_endpoints_as_ports() {
    let source = concat!(
        "namespace std::delay:\n",
        "  proc Delay<T>:\n",
        "    ins<T> 1\n",
        "    outs<T> 1\n",
        "    params<T>:\n",
        "      delay_s = 0.1\n",
        "      feedback = 0.0\n",
        "      mix = 1.0\n",
        "    sample:\n",
        "      out1 = in1\n",
        "\n",
        "namespace std::filter:\n",
        "  proc OnePole<T>:\n",
        "    ins<T> 1\n",
        "    outs<T> 1\n",
        "    params<T>:\n",
        "      cutoff = 1000.0\n",
        "    sample:\n",
        "      out1 = in1\n",
        "\n",
        "params:\n",
        "  drive = 0.2\n",
        "\n",
        "init:\n",
        "  smear_a = std::delay::Delay<f64>(delay_s = 0.031, feedback = 0.78, mix = 1.0)\n",
        "  tone_a = std::filter::OnePole<f64>(cutoff = 4200.0)\n",
        "\n",
        "graph:\n",
        "  drive >> smear_a.delay_s\n",
        "  smear_a.out1 >> tone_a.in1\n",
        "  tone_a.cutoff >> out1\n",
    );
    let tokens = semantic_tokens_for_document(source, None);
    let delay_ctor_line = source
        .lines()
        .position(|line| line.contains("std::delay::Delay"))
        .expect("expected delay constructor line");
    let filter_ctor_line = source
        .lines()
        .position(|line| line.contains("std::filter::OnePole"))
        .expect("expected filter constructor line");
    let drive_line = source
        .lines()
        .position(|line| line.contains("drive >>"))
        .expect("expected drive graph line");
    let signal_line = source
        .lines()
        .position(|line| line.contains("smear_a.out1"))
        .expect("expected signal graph line");
    let cutoff_line = source
        .lines()
        .position(|line| line.contains("tone_a.cutoff"))
        .expect("expected cutoff graph line");

    assert_eq!(
        token_type_at_text_on_line(&tokens, source, delay_ctor_line, "delay_s"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "proc constructor param label delay_s should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, delay_ctor_line, "feedback"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "proc constructor param label feedback should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, delay_ctor_line, "mix"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "proc constructor param label mix should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, filter_ctor_line, "cutoff"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "proc constructor param label cutoff should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, drive_line, "drive"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "standalone graph param reads should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, signal_line, "out1"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "proc output endpoint smear_a.out1 should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, drive_line, "delay_s"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "imported proc param endpoint smear_a.delay_s should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, signal_line, "in1"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "imported proc input endpoint tone_a.in1 should use the port token"
    );
    assert_eq!(
        token_type_at_text_on_line(&tokens, source, cutoff_line, "cutoff"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "imported proc param endpoint tone_a.cutoff should use the port token"
    );
}

#[test]
fn semantic_tokens_mark_named_instance_params_as_ports() {
    let source = concat!(
        "proc Osc:\n",
        "  params:\n",
        "    freq = 440.0\n",
        "  sample:\n",
        "    out1 = freq\n",
        "\n",
        "init:\n",
        "  osc = Osc()\n",
        "\n",
        "graph:\n",
        "  osc.freq >> out1\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert_eq!(
        token_type_at_text_on_line(&tokens, source, 10, "freq"),
        Some(SEMANTIC_TOKEN_TYPE_PORT),
        "declared instance param endpoint osc.freq should use the port token"
    );
}

#[test]
fn semantic_tokens_mark_proc_state_and_hook_state_for_std_osc() {
    let (path, source) = repo_source("stdlib/std/osc.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));
    let incr_lines = [
        line_containing(&source, "incr = 0.0"),
        line_containing(&source, "incr = freq / SR"),
        line_containing(&source, "phase += incr"),
    ];
    for line_no in incr_lines {
        assert_eq!(
            token_type_at_text_on_line(&tokens, &source, line_no, "incr"),
            Some(SEMANTIC_TOKEN_TYPE_STATE),
            "Phasor state 'incr' should remain state in init, hook, and sample"
        );
    }

    let dt_lines = [
        nth_line_containing(&source, "dt = 0.0", 0),
        nth_line_containing(&source, "dt = freq / SR", 0),
        line_containing(&source, "poly_blep<T>(phase, dt)"),
    ];
    for line_no in dt_lines {
        assert_eq!(
            token_type_at_text_on_line(&tokens, &source, line_no, "dt"),
            Some(SEMANTIC_TOKEN_TYPE_STATE),
            "Saw state 'dt' should remain state in init, hook, and sample"
        );
    }
}

#[test]
fn semantic_tokens_highlight_for_loop_references() {
    let source = concat!(
        "namespace test::ns:\n",
        "  const SIZE = 10\n",
        "\n",
        "  proc Foo:\n",
        "    init:\n",
        "      count: i32 = 0\n",
        "\n",
        "    sample:\n",
        "      for i in 0..SIZE:\n",
        "        count = count + 1\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        tokens.iter().any(|t| t.line == 8
            && t.token_type == SEMANTIC_TOKEN_TYPE_ENUM_MEMBER
            && t.length == 4),
        "SIZE in for loop: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 9 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE && t.length == 5),
        "count in for body: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 8 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE && t.length == 1),
        "i in for loop: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_highlight_stepped_for_loop_references_in_incomplete_file() {
    let source = concat!(
        "outs:\n",
        "  out1\n",
        "sample:\n",
        "  for i @ 2 in 0..8:\n",
        "    out1 = i +\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        tokens
            .iter()
            .any(|t| t.line == 3 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE && t.length == 1),
        "stepped loop variable in for header should be highlighted by source fallback: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 4 && t.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE && t.length == 1),
        "stepped loop variable in incomplete body should remain visible: {tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 4 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT && t.length == 4),
        "output port should remain visible in incomplete stepped loop body: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_do_not_infer_locals_from_indexed_assignment_targets() {
    let source = concat!(
        "outs:\n",
        "  out1\n",
        "sample:\n",
        "  arr[0] = 1.0\n",
        "  out1 = arr +\n",
    );
    let tokens = semantic_tokens_for_document(source, None);

    let arr_tokens = find_tokens_named(&tokens, source, "arr");
    assert!(
        arr_tokens.is_empty(),
        "indexed assignment target should not manufacture a fallback local: {arr_tokens:?}"
    );
    assert!(
        tokens
            .iter()
            .any(|t| t.line == 4 && t.token_type == SEMANTIC_TOKEN_TYPE_PORT && t.length == 4),
        "declared output should still be highlighted: {tokens:?}"
    );
}

#[test]
fn semantic_tokens_work_for_convolution_onda() {
    let (path, source) = repo_source("stdlib/std/convolution.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));

    assert!(
        !find_tokens_named(&tokens, &source, "delay").is_empty(),
        "delay should be highlighted"
    );
    assert!(
        !find_tokens_named(&tokens, &source, "write").is_empty(),
        "write should be highlighted"
    );
    assert!(
        !find_tokens_named(&tokens, &source, "active_taps").is_empty(),
        "active_taps should be highlighted"
    );

    let fft_tokens = find_tokens_named(&tokens, &source, "FFTSize");
    assert!(!fft_tokens.is_empty(), "FFTSize should be highlighted");
    assert!(
        fft_tokens
            .iter()
            .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_ENUM_MEMBER),
        "FFTSize should be const: {fft_tokens:?}"
    );

    let t_tokens = find_tokens_named(&tokens, &source, "T");
    assert!(!t_tokens.is_empty(), "T should be highlighted");
    assert!(
        t_tokens
            .iter()
            .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_TYPE),
        "T should be type: {t_tokens:?}"
    );

    let clear_state = find_tokens_named(&tokens, &source, "clear_state");
    assert!(!clear_state.is_empty(), "clear_state should be highlighted");
    assert!(
        clear_state
            .iter()
            .any(|t| t.token_type == SEMANTIC_TOKEN_TYPE_FUNCTION),
        "clear_state should be function: {clear_state:?}"
    );

    assert!(
        !find_tokens_named(&tokens, &source, "td").is_empty(),
        "td should be highlighted"
    );
    assert!(
        !find_tokens_named(&tokens, &source, "final").is_empty(),
        "final should be highlighted"
    );

    let set_impulse = find_tokens_named(&tokens, &source, "set_impulse");
    assert!(!set_impulse.is_empty(), "set_impulse should be highlighted");
    let reset = find_tokens_named(&tokens, &source, "reset");
    assert!(!reset.is_empty(), "reset should be highlighted");
}

#[test]
fn semantic_tokens_index_top_level_and_proc_task_scopes() {
    let source = r#"init:
  top_state = 1
task prepare_top():
  top_local = top_state
  yield
block:
  await prepare_top()
  sample:
    out1 = 0.0

proc Loader:
  init:
    proc_state = 1
  task prepare_proc():
    proc_local = proc_state
    yield
  block:
    await prepare_proc()
    sample:
      out1 = 0.0
"#;
    let tokens = semantic_tokens_for_document(source, None);

    for name in ["prepare_top", "prepare_proc"] {
        let task_tokens = find_tokens_named(&tokens, source, name);
        assert!(
            task_tokens.len() >= 2
                && task_tokens
                    .iter()
                    .all(|token| token.token_type == SEMANTIC_TOKEN_TYPE_FUNCTION),
            "task declarations and await sites should be functions: {task_tokens:?}"
        );
    }
    for name in ["top_state", "proc_state"] {
        let state_tokens = find_tokens_named(&tokens, source, name);
        assert!(
            state_tokens
                .iter()
                .any(|token| token.token_type == SEMANTIC_TOKEN_TYPE_STATE),
            "tasks should inherit owner state symbols: {state_tokens:?}"
        );
    }
    for name in ["top_local", "proc_local"] {
        let local_tokens = find_tokens_named(&tokens, source, name);
        assert!(
            local_tokens
                .iter()
                .any(|token| token.token_type == SEMANTIC_TOKEN_TYPE_VARIABLE),
            "task-local assignments should be variables: {local_tokens:?}"
        );
    }
}

#[test]
fn source_fallback_indexes_standalone_and_grouped_tasks() {
    let source = r#"init:
  state = 1
tasks:
  grouped():
    grouped_local = state
    yield
task standalone():
  standalone_local = state
  yield
"#;
    let fallback = build_source_scope_index(source);

    assert_eq!(
        fallback.token_type_for("grouped", 4, 4),
        Some(SEMANTIC_TOKEN_TYPE_FUNCTION)
    );
    assert_eq!(
        fallback.token_type_for("standalone", 7, 4),
        Some(SEMANTIC_TOKEN_TYPE_FUNCTION)
    );
    assert_eq!(
        fallback.token_type_for("state", 4, 20),
        Some(SEMANTIC_TOKEN_TYPE_STATE)
    );
    assert_eq!(
        fallback.token_type_for("grouped_local", 4, 4),
        Some(SEMANTIC_TOKEN_TYPE_VARIABLE)
    );
    assert_eq!(
        fallback.token_type_for("standalone_local", 7, 4),
        Some(SEMANTIC_TOKEN_TYPE_VARIABLE)
    );
}
