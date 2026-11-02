#![cfg(feature = "llvm-orc")]
use onda_codegen_llvm::{
    jit_program_from_optimized_mir_with_options, MirCompileOptions, TargetOptLevel,
};

#[test]
fn enum_tuple_messages_clamp_ordinals_and_forward_through_delegates() {
    let typed = onda_semantics::analyze(
        onda_frontend::parse_program(include_str!(
            "../../../packages/onda_binaryen_web/test/fixtures/enums-tuples.onda"
        ))
        .unwrap(),
    )
    .unwrap();
    let program = onda_semantics::lower_program_to_optimized_mir(&typed).unwrap();
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let jit = jit_program_from_optimized_mir_with_options(
            program.clone(),
            MirCompileOptions {
                opt_level,
                ..Default::default()
            },
        )
        .unwrap();
        let params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        for (ordinal, expected) in [(i32::MIN, 7.0), (i32::MAX, 9.0)] {
            let payload: Vec<_> = [ordinal, 7]
                .into_iter()
                .flat_map(i32::to_le_bytes)
                .collect();
            unsafe {
                jit.trigger_event_by_index_unchecked(
                    &mut state,
                    &params,
                    0,
                    &payload,
                    &[],
                    &[],
                    &[],
                    &[],
                    None,
                )
                .unwrap();
                let mut output = [f32::NAN; 4];
                jit.process_checked(
                    &mut state,
                    &params,
                    0,
                    4,
                    3,
                    &[],
                    &[output.as_mut_ptr().cast()],
                    &[],
                    &[],
                    &[],
                    &[],
                    None,
                )
                .unwrap();
                assert_eq!(output, [expected; 4], "{opt_level:?}");
            }
        }
    }
}

#[test]
fn enum_metadata_and_raw_boundaries_match_at_both_optimization_levels() {
    let typed = onda_semantics::analyze_with_options(
        onda_frontend::parse_program(include_str!(
            "../../../packages/onda_binaryen_web/test/fixtures/enums.onda"
        ))
        .unwrap(),
        onda_semantics::AnalysisOptions {
            sample_rate: 48000.0,
            block_size: 4,
        },
    )
    .unwrap();
    let program = onda_semantics::lower_program_to_optimized_mir(&typed).unwrap();
    for opt_level in [TargetOptLevel::O0, TargetOptLevel::O3] {
        let jit = jit_program_from_optimized_mir_with_options(
            program.clone(),
            MirCompileOptions {
                opt_level,
                ..Default::default()
            },
        )
        .unwrap();
        let enumeration = jit.param_descriptor(0).unwrap().enumeration().unwrap();
        assert_eq!(enumeration.name, "Mode");
        assert_eq!(enumeration.members[0].label, "Low Pass");
        assert_eq!(enumeration.members[2].label, "Band-pass");
        assert_eq!(
            jit.param_descriptor(1)
                .unwrap()
                .param_domain()
                .unwrap()
                .step_count(),
            Some(0)
        );
        let schema = jit.event_descriptor(0).unwrap().schema();
        let serialized = format!("{schema:?}");
        assert!(serialized.contains("Band-pass"));
        let mut params = jit.default_param_bytes();
        let mut state = jit.initialize_state(&params).unwrap();
        let payload: Vec<_> = [i32::MAX, i32::MIN, 99]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect();
        unsafe {
            jit.trigger_event_by_index_unchecked(
                &mut state,
                &params,
                0,
                &payload,
                &[],
                &[],
                &[],
                &[],
                None,
            )
            .unwrap();
            jit.trigger_event_by_index_unchecked(
                &mut state,
                &params,
                1,
                &99_i32.to_le_bytes(),
                &[],
                &[],
                &[],
                &[],
                None,
            )
            .unwrap();
        }
        for (ordinal, expected) in [(i64::MIN, 0.0), (i64::MAX, 2.0)] {
            for index in 0..2 {
                let offset = jit.param_descriptor(index).unwrap().byte_offset();
                params[offset..offset + 4].copy_from_slice(&99_i32.to_ne_bytes());
            }
            let offset = jit.param_descriptor(2).unwrap().byte_offset();
            params[offset..offset + 8].copy_from_slice(&ordinal.to_ne_bytes());
            let offset = jit.param_descriptor(3).unwrap().byte_offset();
            params[offset..offset + 4].copy_from_slice(&99_i32.to_ne_bytes());
            params[offset + 4..offset + 8].copy_from_slice(&(-99_i32).to_ne_bytes());
            let mut outputs = [[f32::NAN; 4]; 9];
            let pointers = outputs
                .iter_mut()
                .map(|out| out.as_mut_ptr().cast())
                .collect::<Vec<_>>();
            unsafe {
                jit.process_checked(
                    &mut state,
                    &params,
                    0,
                    4,
                    3,
                    &[],
                    &pointers,
                    &[],
                    &[],
                    &[],
                    &[],
                    None,
                )
                .unwrap();
            }
            assert_eq!(
                outputs,
                [2.0, 2.0, 0.0, 2.0, 2.0, 0.0, expected, 2.0, 0.0].map(|value| [value; 4]),
                "{opt_level:?}"
            );
        }
        let mut snapshot = vec![0; jit.state_size_bytes()];
        jit.write_state_snapshot(&state, &mut snapshot).unwrap();
        for entry in jit
            .state_entries()
            .iter()
            .filter(|entry| entry.integer_range().is_some())
        {
            for (index, value) in snapshot
                [entry.byte_offset()..entry.byte_offset() + entry.byte_size()]
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .enumerate()
            {
                value.copy_from_slice(
                    &(if index % 2 == 0 { 99_i32 } else { -99_i32 }).to_le_bytes(),
                );
            }
        }
        unsafe {
            jit.restore_state_snapshot(
                &params,
                &mut state,
                &snapshot,
                onda_codegen_llvm::BufferDescriptorTables::new(&[], &[], &[], &[]),
            )
            .unwrap();
        }
        jit.write_state_snapshot(&state, &mut snapshot).unwrap();
        for entry in jit
            .state_entries()
            .iter()
            .filter(|entry| entry.integer_range().is_some())
        {
            for (index, value) in snapshot
                [entry.byte_offset()..entry.byte_offset() + entry.byte_size()]
                .as_chunks::<4>()
                .0
                .iter()
                .enumerate()
            {
                assert_eq!(
                    i32::from_le_bytes(*value),
                    if index % 2 == 0 { 2 } else { 0 }
                );
            }
        }
    }
}
