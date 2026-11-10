use super::*;

const SOURCE: &str = r#"
struct Note:
  gain: f64
  bins: i32[2]
delegate reported(enabled: bool, notes: Note[], last: i64, weights: f32[])
event report(enabled: bool, notes: Note[], last: i64, weights: f32[]):
  reported(enabled, notes, last, weights)
sample:
  out1 = 0.0
"#;

#[test]
fn c_api_layout_regions_encode_real_events_and_match_delegate_payloads() {
    unsafe {
        let program = compile_program(SOURCE);
        let event = &*onda_event_info(program.0, 0);
        let delegate = &*onda_delegate_info(program.0, 0);
        assert_eq!(event.region_count, 7);
        assert_eq!(delegate.region_count, event.region_count);
        for index in 0..event.tensor_count {
            let a = &*onda_payload_tensor_info(event.payload_plan, index);
            let b = &*onda_payload_tensor_info(delegate.payload_plan, index);
            assert_eq!(CStr::from_ptr(a.path), CStr::from_ptr(b.path));
            assert_eq!(a.parameter_index, b.parameter_index);
            assert_eq!(a.element_type, b.element_type);
            assert_eq!(a.is_slice, b.is_slice);
            assert_eq!(a.fixed_element_count, b.fixed_element_count);
            assert_eq!(a.shape_rank, b.shape_rank);
            assert_eq!(a.shape.is_null(), b.shape.is_null());
            if a.shape_rank > 0 {
                assert_eq!(
                    std::slice::from_raw_parts(a.shape, a.shape_rank as usize),
                    std::slice::from_raw_parts(b.shape, b.shape_rank as usize)
                );
            }
        }
        assert!(onda_payload_tensor_info(std::ptr::null(), 0).is_null());
        assert!(onda_payload_tensor_info(event.payload_plan, -1).is_null());
        assert!(onda_payload_tensor_info(delegate.payload_plan, delegate.tensor_count).is_null());
        let mut diag = empty_diag();
        let instance = InstanceHandle(initialized_instance(
            program.0,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut *diag,
        ));
        assert!(!instance.0.is_null(), "{}", diag_message(&diag));
        for lengths in [[2, 3], [0, 0]] {
            let mut layout = onda_payload_layout_t::default();
            assert_eq!(
                onda_payload_layout(
                    event.payload_plan,
                    lengths.as_ptr(),
                    2,
                    std::ptr::null_mut(),
                    0,
                    &mut layout
                ),
                0
            );
            let mut regions = vec![onda_payload_region_t::default(); layout.region_count as usize];
            assert_eq!(
                onda_payload_layout(
                    event.payload_plan,
                    lengths.as_ptr(),
                    2,
                    regions.as_mut_ptr(),
                    regions.len() as i32,
                    &mut layout
                ),
                0
            );
            assert_eq!(layout.region_count, 7);
            let expected = if lengths[0] == 2 {
                [
                    (0, 0, 1, 0, 1, 0),
                    (1, -1, 1, 1, 4, 4),
                    (1, 1, 2, 5, 16, 8),
                    (1, 2, 4, 21, 16, 24),
                    (2, 3, 1, 37, 8, 40),
                    (3, -1, 1, 45, 4, 48),
                    (3, 4, 3, 49, 12, 52),
                ]
            } else {
                [
                    (0, 0, 1, 0, 1, 0),
                    (1, -1, 1, 1, 4, 4),
                    (1, 1, 0, 5, 0, 8),
                    (1, 2, 0, 5, 0, 8),
                    (2, 3, 1, 5, 8, 8),
                    (3, -1, 1, 13, 4, 16),
                    (3, 4, 0, 17, 0, 20),
                ]
            };
            for (
                region,
                (
                    parameter_index,
                    tensor_index,
                    element_count,
                    payload_offset,
                    payload_bytes,
                    workspace_offset,
                ),
            ) in regions.iter().zip(expected)
            {
                assert_eq!(
                    *region,
                    onda_payload_region_t {
                        parameter_index,
                        tensor_index,
                        element_count,
                        payload_offset,
                        payload_bytes,
                        workspace_offset,
                    }
                );
            }
            let mut delegate_regions = regions.clone();
            let mut delegate_layout = onda_payload_layout_t::default();
            assert_eq!(
                onda_payload_layout(
                    delegate.payload_plan,
                    lengths.as_ptr(),
                    2,
                    delegate_regions.as_mut_ptr(),
                    7,
                    &mut delegate_layout
                ),
                0
            );
            assert_eq!(delegate_regions, regions);
            assert_eq!(delegate_layout, layout);

            let mut wire = vec![0; layout.payload_bytes as usize];
            for region in &regions {
                let bytes = &mut wire[region.payload_offset as usize
                    ..(region.payload_offset + region.payload_bytes) as usize];
                match region.tensor_index {
                    -1 => bytes.copy_from_slice(
                        &lengths[usize::from(region.parameter_index == 3)].to_le_bytes(),
                    ),
                    0 => bytes[0] = 1,
                    1 => bytes
                        .as_chunks_mut::<8>()
                        .0
                        .iter_mut()
                        .for_each(|b| b.copy_from_slice(&1.5_f64.to_le_bytes())),
                    2 => bytes
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .for_each(|b| b.copy_from_slice(&7_i32.to_le_bytes())),
                    3 => bytes.copy_from_slice(&9_007_199_254_740_993_i64.to_le_bytes()),
                    4 => bytes
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .for_each(|b| b.copy_from_slice(&0.25_f32.to_le_bytes())),
                    _ => unreachable!(),
                }
            }
            let mut decoded = vec![onda_payload_region_t::default(); regions.len()];
            let mut decoded_layout = onda_payload_layout_t::default();
            assert_eq!(
                onda_payload_layout_from_bytes(
                    delegate.payload_plan,
                    wire.as_ptr(),
                    wire.len() as i32,
                    decoded.as_mut_ptr(),
                    decoded.len() as i32,
                    &mut decoded_layout
                ),
                0
            );
            assert_eq!(decoded, regions);
            assert_eq!(decoded_layout, layout);
            assert_eq!(
                onda_payload_layout_from_bytes(
                    event.payload_plan,
                    wire.as_ptr(),
                    wire.len() as i32,
                    std::ptr::null_mut(),
                    0,
                    &mut decoded_layout
                ),
                0
            );
            assert_eq!(decoded_layout, layout);
            // Rejection must preserve every output, even for a late bad extent.
            let mut truncated = wire.clone();
            truncated.pop();
            let mut trailing = wire.clone();
            trailing.push(0);
            let mut negative = wire.clone();
            negative[1..5].copy_from_slice(&(-1_i32).to_le_bytes());
            let mut oversized = wire.clone();
            oversized[1..5].copy_from_slice(&i32::MAX.to_le_bytes());
            for (malformed, expected_status) in [
                (&truncated, ONDA_API_ERROR_INVALID_SHAPE),
                (&trailing, ONDA_API_ERROR_INVALID_SHAPE),
                (&negative, ONDA_API_ERROR_INVALID_SHAPE),
                (&oversized, ONDA_API_ERROR_SIZE_OVERFLOW),
            ] {
                let before = decoded.clone();
                assert_eq!(
                    onda_payload_layout_from_bytes(
                        delegate.payload_plan,
                        malformed.as_ptr(),
                        malformed.len() as i32,
                        decoded.as_mut_ptr(),
                        decoded.len() as i32,
                        &mut decoded_layout
                    ),
                    expected_status
                );
                assert_eq!(decoded, before);
                assert_eq!(decoded_layout, layout);
            }
            assert_eq!(
                onda_payload_layout_from_bytes(
                    event.payload_plan,
                    wire.as_ptr(),
                    wire.len() as i32,
                    decoded.as_mut_ptr(),
                    decoded.len() as i32 - 1,
                    &mut decoded_layout
                ),
                ONDA_API_ERROR_SIZE_MISMATCH
            );
            assert_eq!(decoded, regions);
            assert_eq!(decoded_layout, layout);
            let mut storage = [0_u8; 256];
            let mut batch = onda_delegate_batch_t {
                storage: storage.as_mut_ptr(),
                capacity_bytes: storage.len() as u32,
                used_bytes: 0,
                record_count: 0,
                overflow_count: 0,
            };
            let mut output = onda_execution_output_t {
                delegate_batch: &mut batch,
                print_batch: std::ptr::null_mut(),
            };
            assert_eq!(
                onda_trigger_event_by_index_checked(
                    instance.0,
                    0,
                    wire.as_ptr().cast(),
                    wire.len() as i32,
                    &mut output
                ),
                0
            );
            assert_eq!(batch.record_count, 1);
            assert_eq!(batch.overflow_count, 0);
            assert_eq!(
                &storage[ONDA_DELEGATE_RECORD_HEADER_SIZE as usize..batch.used_bytes as usize],
                wire
            );
        }
    }
}

#[test]
fn c_api_layout_failures_preserve_all_outputs() {
    unsafe {
        let program = compile_program(SOURCE);
        let plan = (*onda_event_info(program.0, 0)).payload_plan;
        let sentinel = onda_payload_layout_t {
            payload_bytes: 71,
            workspace_bytes: 73,
            region_count: 79,
        };
        let region = onda_payload_region_t {
            tensor_index: 101,
            ..Default::default()
        };
        for (lengths, expected) in [
            (vec![], ONDA_API_ERROR_INVALID_SHAPE),
            (vec![2], ONDA_API_ERROR_INVALID_SHAPE),
            (vec![2, 3, 4], ONDA_API_ERROR_INVALID_SHAPE),
            (vec![-1, 3], ONDA_API_ERROR_INVALID_SHAPE),
            (vec![2, -1], ONDA_API_ERROR_INVALID_SHAPE),
            (vec![i32::MAX, 0], ONDA_API_ERROR_SIZE_OVERFLOW),
            (vec![2, i32::MAX], ONDA_API_ERROR_SIZE_OVERFLOW),
        ] {
            let mut layout = sentinel;
            let mut regions = [region; 7];
            assert_eq!(
                onda_payload_layout(
                    plan,
                    lengths.as_ptr(),
                    lengths.len() as i32,
                    regions.as_mut_ptr(),
                    7,
                    &mut layout
                ),
                expected
            );
            assert_eq!(layout, sentinel);
            assert_eq!(regions, [region; 7]);
        }
        let lengths = [2, 3];
        let mut layout = sentinel;
        let mut regions = [region; 7];
        for (plan, lengths, count, capacity, expected) in [
            (
                std::ptr::null(),
                lengths.as_ptr(),
                2,
                7,
                ONDA_API_ERROR_INVALID_ARGUMENT,
            ),
            (
                plan,
                std::ptr::null(),
                2,
                7,
                ONDA_API_ERROR_INVALID_ARGUMENT,
            ),
            (
                plan,
                lengths.as_ptr(),
                -1,
                7,
                ONDA_API_ERROR_INVALID_ARGUMENT,
            ),
            (
                plan,
                lengths.as_ptr(),
                2,
                -1,
                ONDA_API_ERROR_INVALID_ARGUMENT,
            ),
            (plan, lengths.as_ptr(), 2, 6, ONDA_API_ERROR_SIZE_MISMATCH),
        ] {
            assert_eq!(
                onda_payload_layout(
                    plan,
                    lengths,
                    count,
                    regions.as_mut_ptr(),
                    capacity,
                    &mut layout
                ),
                expected
            );
            assert_eq!(layout, sentinel);
            assert_eq!(regions, [region; 7]);
        }
        assert_eq!(
            onda_payload_layout(
                plan,
                lengths.as_ptr(),
                2,
                regions.as_mut_ptr(),
                7,
                std::ptr::null_mut()
            ),
            ONDA_API_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(
            onda_payload_layout(
                plan,
                lengths.as_ptr(),
                2,
                std::ptr::null_mut(),
                7,
                &mut layout
            ),
            ONDA_API_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(layout, sentinel);
        assert_eq!(regions, [region; 7]);
    }
}

#[test]
fn c_api_empty_message_layout_needs_no_storage() {
    unsafe {
        let program = compile_program("event tick() {}\nsample { out1 = 0.0 }\n");
        let plan = (*onda_event_info(program.0, 0)).payload_plan;
        let mut layout = onda_payload_layout_t::default();
        assert_eq!(
            onda_payload_layout(
                plan,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut layout
            ),
            0
        );
        assert_eq!(layout, onda_payload_layout_t::default());
        assert_eq!(
            onda_payload_layout_from_bytes(
                plan,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &mut layout
            ),
            0
        );
        assert_eq!(layout, onda_payload_layout_t::default());
        assert!(onda_payload_tensor_info(plan, 0).is_null());
    }
}
