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

fn has_token_text_at_line(
    tokens: &[SemanticToken],
    source: &str,
    line_no: usize,
    needle: &str,
) -> bool {
    let line = source
        .lines()
        .nth(line_no)
        .expect("expected source line for token lookup");
    let start = line.find(needle).expect("expected token text on line");
    tokens.iter().any(|t| {
        t.line as usize == line_no && t.start as usize == start && t.length as usize == needle.len()
    })
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
            .any(|t| t.line == 8 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "top-level def should not see runtime param via source fallback: {gain_tokens:?}"
    );
    assert!(
        gain_tokens
            .iter()
            .any(|t| t.line == 12 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
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
fn semantic_tokens_mark_onda_params_as_parameters() {
    let source = "params:\n  gain = 0.5\nsample:\n  out1 = gain\n";
    let tokens = semantic_tokens_for_document(source, None);

    assert!(
        has_token(&tokens, 1, 2, 4, SEMANTIC_TOKEN_TYPE_PARAMETER),
        "tokens: {tokens:?}"
    );
    assert!(
        has_token(&tokens, 3, 9, 4, SEMANTIC_TOKEN_TYPE_PARAMETER),
        "tokens: {tokens:?}"
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
fn semantic_tokens_do_not_leak_event_locals_into_sample_blocks_for_simple_events_file() {
    let (path, source) = repo_source("examples/simple_events.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));

    let amp_tokens = find_tokens_named(&tokens, &source, "amp");
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
        !amp_tokens.iter().any(|token| token.line == 24),
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
            .any(|t| t.line == 14 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
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
        "    line: buffer[f32]\n",
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
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
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
        has_token(&tokens, 6, 4, 4, SEMANTIC_TOKEN_TYPE_PARAMETER),
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
fn semantic_tokens_mark_top_level_init_vars_in_buffer_looper_read() {
    let (path, source) = repo_source("examples/buffer_looper_read.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));

    let pos_tokens = find_tokens_named(&tokens, &source, "pos");
    assert!(!pos_tokens.is_empty(), "pos should be highlighted");
    assert!(
        pos_tokens
            .iter()
            .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "pos should be state everywhere: {pos_tokens:?}"
    );

    let rate_tokens = find_tokens_named(&tokens, &source, "rate");
    assert!(!rate_tokens.is_empty(), "rate should be highlighted");
    assert!(
        rate_tokens
            .iter()
            .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "rate should be parameter everywhere: {rate_tokens:?}"
    );

    let src_tokens = find_tokens_named(&tokens, &source, "src");
    assert!(!src_tokens.is_empty(), "src should be highlighted");
    assert!(
        src_tokens
            .iter()
            .all(|t| t.token_type == SEMANTIC_TOKEN_TYPE_PORT),
        "src should be port everywhere: {src_tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_top_level_block_locals_in_nested_sample_for_buffer_looper_read() {
    let (path, source) = repo_source("examples/buffer_looper_read.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));

    let frames_tokens = find_tokens_named(&tokens, &source, "frames");
    assert!(
        frames_tokens
            .iter()
            .any(|t| t.line == 12 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "top-level block local 'frames' should be highlighted in block: {frames_tokens:?}"
    );
    assert!(
        frames_tokens
            .iter()
            .any(|t| t.line == 23 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "top-level block local 'frames' should carry into nested sample: {frames_tokens:?}"
    );

    let chans_tokens = find_tokens_named(&tokens, &source, "chans");
    assert!(
        chans_tokens
            .iter()
            .any(|t| t.line == 17 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "top-level block local 'chans' should carry into nested sample: {chans_tokens:?}"
    );

    let speed_tokens = find_tokens_named(&tokens, &source, "speed");
    assert!(
        speed_tokens
            .iter()
            .any(|t| t.line == 22 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "top-level block local 'speed' should carry into nested sample: {speed_tokens:?}"
    );
}

#[test]
fn semantic_tokens_do_not_mark_import_path_segments_as_init_state_in_polyphonic_saw() {
    let (path, source) = repo_source("examples/polyphonic_saw.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));

    let osc_tokens = find_tokens_named(&tokens, &source, "osc");
    assert!(
        osc_tokens
            .iter()
            .all(|t| !(t.line == 0 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE)),
        "import path segment 'osc' should not be state on line 0: {osc_tokens:?}"
    );
    assert!(
        osc_tokens
            .iter()
            .any(|t| t.line > 0 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "proc init state 'osc' should still be highlighted as state in executable scopes: {osc_tokens:?}"
    );

    let env_tokens = find_tokens_named(&tokens, &source, "env");
    assert!(
        env_tokens
            .iter()
            .all(|t| !(t.line == 2 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE)),
        "import path segment 'env' should not be state on line 2: {env_tokens:?}"
    );
    assert!(
        env_tokens
            .iter()
            .any(|t| t.line > 2 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "proc init state 'env' should still be highlighted as state in executable scopes: {env_tokens:?}"
    );
}

#[test]
fn semantic_tokens_do_not_highlight_named_argument_labels_in_polyphonic_saw() {
    let (path, source) = repo_source("examples/polyphonic_saw.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));

    assert!(
        !has_token_text_at_line(&tokens, &source, 16, "freq"),
        "named arg label 'freq =' should not be highlighted"
    );
    assert!(
        !has_token_text_at_line(&tokens, &source, 17, "cutoff"),
        "named arg label 'cutoff =' should not be highlighted"
    );
    assert!(
        !has_token_text_at_line(&tokens, &source, 18, "decay_s"),
        "named arg label 'decay_s =' should not be highlighted"
    );
    assert!(
        !has_token_text_at_line(&tokens, &source, 18, "trigger"),
        "named arg label 'trigger =' should not be highlighted"
    );
    assert!(
        !has_token_text_at_line(&tokens, &source, 65, "freq_hz"),
        "event call named arg label 'freq_hz =' should not be highlighted"
    );
    assert!(
        !has_token_text_at_line(&tokens, &source, 66, "cutoff_hz"),
        "event call named arg label 'cutoff_hz =' should not be highlighted"
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
fn semantic_tokens_do_not_highlight_named_argument_labels_in_calls() {
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
        !has_token_text_at_line(&tokens, source, 7, "a"),
        "named arg label 'a =' should not be highlighted: {tokens:?}"
    );
    assert!(
        !has_token_text_at_line(&tokens, source, 7, "b"),
        "named arg label 'b =' should not be highlighted: {tokens:?}"
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
            .any(|t| t.line == 5 && t.token_type == SEMANTIC_TOKEN_TYPE_PARAMETER),
        "incomplete graph fallback should keep params visible: {gain_tokens:?}"
    );
}

#[test]
fn semantic_tokens_mark_proc_block_locals_in_nested_sample_for_std_osc() {
    let (path, source) = repo_source("stdlib/std/osc.onda");
    let tokens = semantic_tokens_for_document(&source, Some(&path));

    let incr_tokens = find_tokens_named(&tokens, &source, "incr");
    assert!(
        incr_tokens
            .iter()
            .any(|t| t.line == 11 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "proc block local 'incr' should be highlighted in block: {incr_tokens:?}"
    );
    assert!(
        incr_tokens
            .iter()
            .any(|t| t.line == 13 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "proc block local 'incr' should carry into nested sample: {incr_tokens:?}"
    );

    let dt_tokens = find_tokens_named(&tokens, &source, "dt");
    assert!(
        dt_tokens
            .iter()
            .any(|t| t.line == 58 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "proc block local 'dt' should be highlighted in block: {dt_tokens:?}"
    );
    assert!(
        dt_tokens
            .iter()
            .any(|t| t.line == 62 && t.token_type == SEMANTIC_TOKEN_TYPE_STATE),
        "proc block local 'dt' should carry into nested sample: {dt_tokens:?}"
    );
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
        !find_tokens_named(&tokens, &source, "tail").is_empty(),
        "tail should be highlighted"
    );

    let set_impulse = find_tokens_named(&tokens, &source, "set_impulse");
    assert!(!set_impulse.is_empty(), "set_impulse should be highlighted");
    let reset = find_tokens_named(&tokens, &source, "reset");
    assert!(!reset.is_empty(), "reset should be highlighted");
}
