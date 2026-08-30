#[test]

fn builtin_std_import_resolves_without_local_std_path() {
    let dir = mk_temp_dir("builtin_std_import");

    let main = dir.join("main.onda");

    let shadow_std_dir = dir.join("std");

    fs::create_dir_all(&shadow_std_dir).expect("create local std dir");

    fs::write(
        shadow_std_dir.join("osc.onda"),
        r#"

def broken() {

  return unknown_symbol

}

"#,
    )
    .expect("write local shadow std file");

    fs::write(
        &main,
        r#"

import std/osc

outs { out1 }

init { o = std::osc::Sine(freq = 220.0) }

sample { out1 = o() }

"#,
    )
    .expect("write main");

    let parsed = parse_program_file(&main).expect("parse program file");

    let _typed = analyze_with_options(
        parsed,
        AnalysisOptions {
            sample_rate: 48_000.0,

            block_size: 64,
        },
    )
    .expect("semantic analysis");

    fs::remove_dir_all(&dir).ok();
}

#[test]

fn struct_initialization_in_sample_is_rejected() {
    let parsed = parse_program(STRUCT_INIT_IN_SAMPLE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject struct ctor in sample"
    );
}

#[test]

fn data_read_write_clamps_and_truncates() {
    let frames = 8;

    let (mut instance, in_channels, out_channels) = compile_instance(DATA_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 12.0, 1e-6);
    }
}

#[test]

fn indexed_data_read_write_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(INDEXED_DATA_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]

fn indexed_access_supports_struct_field_data() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INDEXED_STRUCT_FIELD_DATA_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn indexed_access_supports_typed_local_array_in_def() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INDEXED_TYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn data_len_returns_data_capacity() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(DATA_LEN_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn data_len_supports_struct_data_field_receiver() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_LEN_STRUCT_FIELD_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 8.0, 1e-6);
    }
}

#[test]

fn data_len_rejects_non_data_receiver() {
    let parsed =
        parse_program(DATA_LEN_INVALID_RECEIVER_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_err(),
        "semantic analysis should reject x.len() for scalar x"
    );
}

#[test]

fn data_struct_elements_support_alias_field_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 1.5;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn struct_field_data_struct_elements_support_alias_field_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 3.0;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn init_struct_field_data_struct_elements_support_alias_field_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn def_struct_field_data_struct_elements_support_alias_field_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_FIELD_DATA_STRUCT_ELEM_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn def_struct_field_nested_data_struct_elements_support_alias_field_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        DEF_STRUCT_FIELD_NESTED_DATA_STRUCT_ELEM_ALIAS_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 7.0, 1e-6);
    }
}

#[test]

fn data_struct_inline_field_read_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_INLINE_FIELD_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 1.0, 1e-6);

    for sample in &output[1..] {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn data_struct_inline_array_field_read_compiles_and_runs() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_INLINE_ARRAY_FIELD_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    assert_near(output[0], 2.0, 1e-6);

    assert_near(output[1], 4.0, 1e-6);
}

#[test]

fn init_struct_inline_field_read_compiles_and_runs() {
    let frames = 2;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_STRUCT_INLINE_FIELD_READ_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 5.0, 1e-6);
    }
}

#[test]

fn data_struct_nested_data_fields_support_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = 1.0 + ((idx + 1) as f32) * 0.25;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn struct_field_data_struct_nested_data_fields_support_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(STRUCT_FIELD_DATA_STRUCT_NESTED_DATA_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 0.5;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn data_struct_nested_struct_data_fields_support_recursive_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 0.25;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn struct_field_data_struct_nested_struct_data_fields_support_recursive_alias_index_read_write() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        STRUCT_FIELD_DATA_STRUCT_NESTED_STRUCT_DATA_ALIAS_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for (idx, sample) in output.iter().enumerate() {
        let expected = ((idx + 1) as f32) * 1.0;

        assert_near(*sample, expected, 1e-6);
    }
}

#[test]

fn primitive_data_local_alias_binding_is_rejected() {
    let parsed = parse_program(DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_ok(),

        "semantic analysis should allow primitive array indexed reads as scalar copies via 'x = buf[i]'"

    );
}

#[test]

fn primitive_struct_field_data_local_alias_binding_is_rejected() {
    let parsed =
        parse_program(STRUCT_DATA_LOCAL_ALIAS_WRITEBACK_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(

        result.is_ok(),

        "semantic analysis should allow primitive struct-array indexed reads as scalar copies via 'x = v.delay[i]'"

    );
}

#[test]

fn init_array_index_scalar_copy_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(INIT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.5, 1e-6);
    }
}

#[test]

fn def_struct_array_index_scalar_copy_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(DEF_STRUCT_ARRAY_INDEX_SCALAR_COPY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn typed_local_array_declaration_in_sample_is_allowed() {
    let parsed = parse_program(DATA_INIT_IN_SAMPLE_ERROR_EXAMPLE).expect("parse should succeed");

    let result = analyze(parsed);

    assert!(
        result.is_ok(),
        "semantic analysis should allow primitive T[N] declarations in sample"
    );
}

#[test]

fn typed_local_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn untyped_local_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(UNTYPED_LOCAL_ARRAY_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn typed_local_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.5, 1e-6);
    }
}

#[test]

fn untyped_local_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(UNTYPED_LOCAL_ARRAY_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.0, 1e-6);
    }
}

#[test]

fn typed_local_i32_array_declaration_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_I32_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 3.0, 1e-6);
    }
}

#[test]

fn typed_local_bool_array_declaration_in_def_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_BOOL_DEF_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }
}

#[test]

fn typed_local_array_initializer_in_sample_compiles_and_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TYPED_LOCAL_ARRAY_INIT_SAMPLE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 4.0, 1e-6);
    }
}

#[test]

fn top_level_param_array_defaults_and_set_param_slots_work() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_PARAM_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 1.0, 1e-6);
    }

    set_param_f32_array(&mut instance, "mix", &[1.5, 0.75]);

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in &output {
        assert_near(*sample, 2.25, 1e-6);
    }
}

#[test]

fn declared_param_metadata_reports_array_as_single_entry() {
    let frames = 4;

    let (instance, _in_channels, _out_channels) =
        compile_instance(TOP_LEVEL_PARAM_ARRAY_EXAMPLE, frames);

    assert_eq!(instance.param_count(), 1);

    assert_eq!(instance.param_index("mix"), Some(0));

    assert_eq!(instance.param_name(0), Some("mix"));

    assert_eq!(instance.param_type(0).as_deref(), Some("f32[2]"));

    assert_eq!(instance.param_type_bytes(0), Some(8));
}

#[test]

fn declared_io_metadata_reports_arrays_as_single_entries() {
    let frames = 4;

    let (instance, _in_channels, _out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);

    assert_eq!(instance.input_count(), 1);

    assert_eq!(instance.input_name(0), Some("in1"));

    assert_eq!(instance.input_type(0).as_deref(), Some("f32[2]"));

    assert_eq!(instance.input_type_bytes(0), Some(8));
}

#[test]

fn top_level_input_array_reads_indexed_channels() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_INPUT_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 1);

    let input = vec![
        1.0_f32, 0.5, //
        2.0, 1.0, //
        -1.0, 2.0, //
        0.25, -0.5,
    ];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [2.5_f32, 5.0, 0.0, 0.0];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn top_level_output_array_writes_indexed_channels() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(TOP_LEVEL_OUTPUT_ARRAY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 0.25, 1e-6);

        assert_near(output[base + 1], 0.75, 1e-6);
    }
}

#[test]

fn graph_implicitly_steps_proc_nodes_and_fanout_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_IMPLICIT_PROC_FANOUT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in 0..frames {
        let base = frame * out_channels;

        assert_near(output[base], 0.5, 1e-6);

        assert_near(output[base + 1], 0.5, 1e-6);
    }
}

#[test]

fn graph_delayed_feedback_persists_across_process_calls() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_DELAY_FEEDBACK_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut first = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut first, frames)
        .expect("first process should succeed");

    let expected_first = [1.0_f32, 2.0, 3.0, 4.0];

    for (sample, target) in first.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let mut second = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut second, frames)
        .expect("second process should succeed");

    let expected_second = [5.0_f32, 6.0, 7.0, 8.0];

    for (sample, target) in second.iter().zip(expected_second) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_sample_override_for_param_destinations_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PARAM_SAMPLE_OVERRIDE_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![0.1_f32, 0.2, 0.3, 0.4];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (sample, target) in output.iter().zip(input) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_fanout_destinations_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(GRAPH_FANOUT_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![1.0_f32, -0.5, 0.0, 2.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [0.5_f32, -0.25, 0.0, 1.0];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_proc_bundle_destinations_run_for_proc_and_proc_array_slot_sources() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_BUNDLE_FANOUT_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 4);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    let expected = [
        0.25_f32, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5, //
        0.25, 0.75, 0.5, 0.5,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_proc_array_indexed_param_destinations_and_output_sources_run() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) = compile_instance(
        GRAPH_PROC_ARRAY_PARAM_DEST_AND_OUTPUT_SOURCE_EXAMPLE,
        frames,
    );

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 3);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for frame in output.as_chunks::<3>().0 {
        assert_near(frame[0], 0.25, 1e-6);

        assert_near(frame[1], 0.75, 1e-6);

        assert_near(frame[2], 0.75, 1e-6);
    }
}

#[test]

fn graph_array_expressions_run_element_wise() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_EXPR_EXAMPLE, frames);

    assert_eq!(in_channels, 4);

    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 4.0, 2.0, 8.0, //
        2.0, 5.0, 4.0, 10.0, //
        3.0, 6.0, 6.0, 12.0, //
        4.0, 7.0, 8.0, 14.0,
    ];

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        1.0_f32 * 0.5 + 2.0 * 0.25,
        4.0 * 0.5 + 8.0 * 0.25,
        2.0 * 0.5 + 4.0 * 0.25,
        5.0 * 0.5 + 10.0 * 0.25,
        3.0 * 0.5 + 6.0 * 0.25,
        6.0 * 0.5 + 12.0 * 0.25,
        4.0 * 0.5 + 8.0 * 0.25,
        7.0 * 0.5 + 14.0 * 0.25,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_array_delays_persist_and_shift_element_wise() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_DELAY_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 2);

    let first_input = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    let mut first_output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &first_input, &mut first_output, frames)
        .expect("first process should succeed");

    let expected_first = [
        0.0_f32, 0.0, //
        1.0, 10.0, //
        2.0, 20.0, //
        3.0, 30.0,
    ];

    for (sample, target) in first_output.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let second_input = vec![
        5.0_f32, 50.0, //
        6.0, 60.0, //
        7.0, 70.0, //
        8.0, 80.0,
    ];

    let mut second_output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &second_input, &mut second_output, frames)
        .expect("second process should succeed");

    let expected_second = [
        4.0_f32, 40.0, //
        5.0, 50.0, //
        6.0, 60.0, //
        7.0, 70.0,
    ];

    for (sample, target) in second_output.iter().zip(expected_second) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_scalar_broadcast_to_array_outputs_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_ARRAY_BROADCAST_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 2);

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 0.25, 1e-6);
    }
}

#[test]

fn graph_receiver_delay_runs_as_one_sample_delay() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_RECEIVER_DELAY_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut first = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut first, frames)
        .expect("first process should succeed");

    let expected_first = [0.0_f32, 1.0, 1.0, 1.0];

    for (sample, target) in first.iter().zip(expected_first) {
        assert_near(*sample, target, 1e-6);
    }

    let mut second = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut second, frames)
        .expect("second process should succeed");

    for sample in second {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]

fn graph_slice_sources_route_runtime_channels() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_SLICE_SOURCE_EXAMPLE, frames);

    assert_eq!(in_channels, 4);

    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 10.0, 100.0, 1000.0, //
        2.0, 20.0, 200.0, 2000.0, //
        3.0, 30.0, 300.0, 3000.0, //
        4.0, 40.0, 400.0, 4000.0,
    ];

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        10.0_f32, 100.0, //
        20.0, 200.0, //
        30.0, 300.0, //
        40.0, 400.0,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn proc_local_graphs_compile_and_run_through_top_level_graphs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_LOCAL_GRAPH_EXAMPLE, frames);

    assert_eq!(in_channels, 2);

    assert_eq!(out_channels, 2);

    let input = vec![
        1.0_f32, 10.0, //
        2.0, 20.0, //
        3.0, 30.0, //
        4.0, 40.0,
    ];

    let mut output = vec![0.0_f32; frames * out_channels];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [
        10.0_f32, 1.0, //
        20.0, 2.0, //
        30.0, 3.0, //
        40.0, 4.0,
    ];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_scalar_broadcast_to_proc_input_arrays_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_INPUT_ARRAY_BROADCAST_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]

fn graph_scalar_broadcast_to_proc_param_arrays_runs() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_PARAM_ARRAY_BROADCAST_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 1.0, 1e-6);
    }
}

#[test]

fn graph_proc_named_ports_accept_numbered_aliases() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_NAMED_PORT_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]

fn graph_top_level_named_io_accept_numbered_aliases() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_TOP_LEVEL_NAMED_IO_ALIAS_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![0.25_f32, -0.5, 1.0, 0.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    for (sample, target) in output.iter().zip(input) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_top_level_io_is_inferred_from_graph_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_TOP_LEVEL_IO_INFERENCE_EXAMPLE, frames);

    assert_eq!(in_channels, 1);

    assert_eq!(out_channels, 1);

    let input = vec![0.5_f32, -0.25, 0.0, 1.0];

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &input, &mut output, frames)
        .expect("process should succeed");

    let expected = [0.25_f32, -0.125, 0.0, 0.5];

    for (sample, target) in output.iter().zip(expected) {
        assert_near(*sample, target, 1e-6);
    }
}

#[test]

fn graph_proc_io_is_inferred_from_proc_graph_block() {
    let frames = 4;

    let (mut instance, in_channels, out_channels) =
        compile_instance(GRAPH_PROC_IO_INFERENCE_EXAMPLE, frames);

    assert_eq!(in_channels, 0);

    assert_eq!(out_channels, 1);

    let mut output = vec![0.0_f32; frames];

    process_interleaved(&mut instance, &[], &mut output, frames).expect("process should succeed");

    for sample in output {
        assert_near(sample, 0.75, 1e-6);
    }
}

#[test]

fn graph_proc_custom_io_names_require_declarations() {
    let parsed = parse_program(GRAPH_PROC_CUSTOM_IO_NAMES_REQUIRE_DECLS_ERROR_EXAMPLE)
        .expect("parse should succeed");

    let errs = analyze(parsed).expect_err("undeclared custom graph proc IO should fail");

    assert!(
        errs.iter()
            .any(|d| d.message.contains("not a declared output"))
            && errs.iter().any(|d| d.message.contains("unknown endpoint")),
        "expected graph undeclared-io diagnostic, got {errs:?}"
    );
}
