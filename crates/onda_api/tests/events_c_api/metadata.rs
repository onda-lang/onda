use super::*;

#[test]
fn c_api_event_metadata_queries_work() {
    unsafe {
        let program = compile_program(
            r#"
outs { out1 }
events {
  note_on(note: i32 = 60, vel: i32, accent: bool = true) {
    amp = f32(vel) / 127.0
  }
  set_curve(values: f32[2] = [0.25, 0.75]) {
    amp = values[0] + values[1]
  }
}
init { amp = 0.0 }
sample { out1 = amp }
"#,
        );

        assert_eq!(onda_event_count(program.0), 2);
        let note = &*onda_event_info(program.0, 0);
        assert_eq!(CStr::from_ptr(note.name).to_bytes(), b"note_on");
        assert_eq!(note.payload_bytes, 9);
        let schema: serde_json::Value =
            serde_json::from_str(CStr::from_ptr(note.schema_json).to_str().unwrap()).unwrap();
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 3);
        assert_eq!(params[0]["name"], "note");
        assert_eq!(params[0]["ty"]["encoding"], "i32");
        assert_eq!(params[0]["default"], "60");
        assert_eq!(params[1]["name"], "vel");
        assert!(params[1].get("default").is_none());
        assert_eq!(params[2]["ty"]["encoding"], "bool");
        assert_eq!(params[2]["default"], "true");
        let curve = &*onda_event_info(program.0, 1);
        assert_eq!(curve.payload_bytes, 8);
        let schema: serde_json::Value =
            serde_json::from_str(CStr::from_ptr(curve.schema_json).to_str().unwrap()).unwrap();
        assert_eq!(schema["params"][0]["ty"]["kind"], "array");
        assert_eq!(schema["params"][0]["ty"]["len"], 2);
        assert_eq!(
            schema["params"][0]["default"],
            serde_json::json!(["0.25", "0.75"])
        );
        assert!(onda_event_info(program.0, 2).is_null());
    }
}

#[test]
fn c_api_param_default_bytes_preserve_declared_types_and_arrays() {
    unsafe {
        let program = compile_program(
            r#"
params {
  gain: f32 = 0.25
  ratio: f64 = 0.5
  mode: i32 = 7
  wide: i64 = 1099511627776
  gate: bool = true
  curve: f32[2] = [0.25, 0.75]
}
outs { out1 }
sample { out1 = gain }
"#,
        );

        let expected = [
            0.25f32.to_ne_bytes().to_vec(),
            0.5f64.to_ne_bytes().to_vec(),
            7i32.to_ne_bytes().to_vec(),
            1_099_511_627_776i64.to_ne_bytes().to_vec(),
            vec![1],
            [0.25f32.to_ne_bytes(), 0.75f32.to_ne_bytes()].concat(),
        ];
        assert_eq!(onda_param_count(program.0), expected.len() as i32);
        for (index, expected) in expected.iter().enumerate() {
            let info = &*onda_param_info(program.0, index as i32);
            assert_eq!(info.has_default, 1);
            assert_eq!(info.byte_size, expected.len() as i32);
            assert_eq!(
                std::slice::from_raw_parts(info.default_bytes, info.default_byte_count as usize),
                expected
            );
        }
        assert_eq!((*onda_param_info(program.0, 5)).array_len, 2);
        assert!(onda_param_info(program.0, -1).is_null());
    }
}

#[test]
fn c_api_control_outputs_metadata_and_readback_work() {
    unsafe {
        let frames = 512_i32;
        let program = compile_program(
            r#"
kouts {
  meter: f32
  flags: bool[2]
}

block {
  meter = 3.5
  flags[0] = true
  flags[1] = false
}
"#,
        );

        assert_eq!(onda_output_count(program.0), 0);
        assert_eq!(onda_control_output_count(program.0), 2);

        let meter_name = CStr::from_ptr(
            onda_control_output_info(program.0, 0)
                .as_ref()
                .map_or(std::ptr::null(), |info| info.name),
        )
        .to_string_lossy()
        .into_owned();
        let flags_name = CStr::from_ptr(
            onda_control_output_info(program.0, 1)
                .as_ref()
                .map_or(std::ptr::null(), |info| info.name),
        )
        .to_string_lossy()
        .into_owned();
        assert_eq!(meter_name, "meter");
        assert_eq!(flags_name, "flags");

        let flags_key = CString::new("flags").expect("name contains no NUL bytes");
        assert_eq!(onda_control_output_index(program.0, flags_key.as_ptr()), 1);
        assert_eq!(
            onda_control_output_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.element_type),
            0
        );
        assert_eq!(
            onda_control_output_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.byte_size),
            4
        );
        assert_eq!(
            onda_control_output_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.array_len),
            1
        );
        assert_eq!(
            onda_control_output_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.element_type),
            4
        );
        assert_eq!(
            onda_control_output_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.byte_size),
            2
        );
        assert_eq!(
            onda_control_output_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.array_len),
            2
        );
        assert!(
            onda_control_output_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.slot_offset)
                >= 0
        );
        assert!(
            onda_control_output_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.byte_offset)
                >= 0
        );

        let mut diag = empty_diag();
        let instance = initialized_instance(
            program.0,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut *diag,
        );
        assert!(
            !instance.is_null(),
            "instance create failed: {}",
            diag_message(&diag)
        );
        let instance = InstanceHandle(instance);

        assert_eq!(
            onda_process_checked_segment(
                instance.0,
                0,
                frames,
                ONDA_PROCESS_FULL_BLOCK,
                std::ptr::null_mut()
            ),
            0
        );

        let mut meter = [0_u8; 4];
        assert_eq!(
            onda_control_output_read_bytes(
                instance.0,
                0,
                meter.as_mut_ptr().cast::<c_void>(),
                meter.len() as i32,
            ),
            4
        );
        assert!((f32::from_ne_bytes(meter) - 3.5).abs() < 1e-6);

        let mut flags = [0_u8; 2];
        assert_eq!(
            onda_control_output_read_bytes(
                instance.0,
                1,
                flags.as_mut_ptr().cast::<c_void>(),
                flags.len() as i32,
            ),
            2
        );
        assert_eq!(flags, [1, 0]);
    }
}

#[test]
fn c_api_delegate_metadata_marks_offsets_after_slices_as_dynamic() {
    unsafe {
        let program = compile_program(
            r#"
delegate report(code: i32, one: i32[1], values: f32[], tag: i64)
sample:
  out1 = 0.0
"#,
        );

        let info = &*onda_delegate_info(program.0, 0);
        assert_eq!(info.payload_bytes, -1);
        assert_eq!(info.payload_min_bytes, 20);
        let schema: serde_json::Value =
            serde_json::from_str(CStr::from_ptr(info.schema_json).to_str().unwrap()).unwrap();
        let params = schema["params"].as_array().unwrap();
        assert_eq!(params.len(), 4);
        assert_eq!(params[0]["ty"]["encoding"], "i32");
        assert_eq!(params[1]["ty"]["kind"], "array");
        assert_eq!(params[1]["ty"]["len"], 1);
        assert_eq!(params[2]["ty"]["kind"], "slice");
        assert_eq!(params[2]["ty"]["element"]["encoding"], "f32");
        assert_eq!(params[3]["ty"]["encoding"], "i64");
        let mut layout = onda_payload_layout_t::default();
        assert_eq!(
            onda_payload_layout(
                info.payload_plan,
                [3].as_ptr(),
                1,
                std::ptr::null_mut(),
                0,
                &mut layout,
            ),
            0
        );
        assert_eq!(layout.payload_bytes, 32);
    }
}

#[test]
fn c_api_event_tensor_metadata_flattens_deep_structs_and_arrays() {
    unsafe {
        let program = compile_program(
            r#"
struct Vec2:
  x: f32
  y: f32
struct Trail:
  points: Vec2[4]
struct Body:
  position: Vec2
  trail: Trail
  mass: f32
outs:
  out1
event inspect(bodies: Body[], fixed: Body[2]):
  held = 0.0
init:
  held = 0.0
sample:
  out1 = held
"#,
        );
        let expected = [
            ("bodies.position.x", 0, 1, 1, &[][..]),
            ("bodies.position.y", 0, 1, 1, &[][..]),
            ("bodies.trail.points.x", 0, 1, 4, &[4][..]),
            ("bodies.trail.points.y", 0, 1, 4, &[4][..]),
            ("bodies.mass", 0, 1, 1, &[][..]),
            ("fixed.position.x", 1, 0, 2, &[2][..]),
            ("fixed.position.y", 1, 0, 2, &[2][..]),
            ("fixed.trail.points.x", 1, 0, 8, &[2, 4][..]),
            ("fixed.trail.points.y", 1, 0, 8, &[2, 4][..]),
            ("fixed.mass", 1, 0, 2, &[2][..]),
        ];
        assert_eq!(
            onda_event_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.tensor_count),
            expected.len() as i32
        );
        for (tensor_index, (path, parameter, is_slice, elements, shape)) in
            expected.into_iter().enumerate()
        {
            let info = onda_payload_tensor_info(
                (*onda_event_info(program.0, 0)).payload_plan,
                tensor_index as i32,
            );
            assert!(!info.is_null());
            let info = &*info;
            assert_eq!(CStr::from_ptr(info.path).to_str().unwrap(), path);
            assert_eq!(info.parameter_index, parameter);
            assert_eq!(info.element_type, ONDA_PRIMITIVE_F32);
            assert_eq!(info.is_slice, is_slice);
            assert_eq!(info.fixed_element_count, elements);
            assert_eq!(info.shape_rank, shape.len() as i32);
            let actual_shape = if info.shape_rank == 0 {
                assert!(info.shape.is_null());
                &[][..]
            } else {
                std::slice::from_raw_parts(info.shape, info.shape_rank as usize)
            };
            assert_eq!(actual_shape, shape);
        }
    }
}

#[test]
fn c_api_structured_message_schemas_and_dynamic_sizes_are_reflectable() {
    unsafe {
        let program = compile_program(
            r#"
struct Note:
  gain: f64
  mode: i32
delegate reported(notes: Note[], tail: f64)
event report(notes: Note[], tail: f64):
  reported(notes, tail)
sample:
  out1 = 0.0
"#,
        );

        assert_eq!(
            onda_event_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.payload_bytes),
            -1
        );
        assert_eq!(
            onda_event_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.payload_min_bytes),
            12
        );
        assert_eq!(
            onda_delegate_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.payload_bytes),
            -1
        );
        assert_eq!(
            onda_delegate_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.payload_min_bytes),
            12
        );

        let event_schema = CStr::from_ptr(
            onda_event_info(program.0, 0)
                .as_ref()
                .map_or(std::ptr::null(), |info| info.schema_json),
        )
        .to_str()
        .expect("event schema is UTF-8");
        let event_schema: serde_json::Value =
            serde_json::from_str(event_schema).expect("event schema is JSON");
        assert_eq!(event_schema["params"][0]["name"], "notes");
        assert_eq!(event_schema["params"][0]["ty"]["kind"], "slice");
        assert_eq!(event_schema["params"][0]["ty"]["element"]["name"], "Note");
        assert_eq!(
            event_schema["params"][0]["ty"]["element"]["fields"][0]["name"],
            "gain"
        );
        assert_eq!(event_schema["params"][1]["ty"]["encoding"], "f64");

        let delegate_schema = CStr::from_ptr(
            onda_delegate_info(program.0, 0)
                .as_ref()
                .map_or(std::ptr::null(), |info| info.schema_json),
        )
        .to_str()
        .expect("delegate schema is UTF-8");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(delegate_schema)
                .expect("delegate schema is JSON"),
            event_schema
        );

        let lengths = [2_i32];
        let mut layout = onda_payload_layout_t::default();
        for info in [
            onda_event_info(program.0, 0),
            onda_delegate_info(program.0, 0),
        ] {
            assert_eq!(
                onda_payload_layout(
                    (*info).payload_plan,
                    lengths.as_ptr(),
                    1,
                    std::ptr::null_mut(),
                    0,
                    &mut layout,
                ),
                0
            );
            assert_eq!((layout.payload_bytes, layout.workspace_bytes), (36, 40));
        }
        let before = layout;
        assert_eq!(
            onda_payload_layout(
                (*onda_event_info(program.0, 0)).payload_plan,
                [-1].as_ptr(),
                1,
                std::ptr::null_mut(),
                0,
                &mut layout,
            ),
            ONDA_API_ERROR_INVALID_SHAPE
        );
        assert_eq!(layout, before);
        assert!(onda_event_info(program.0, 1)
            .as_ref()
            .map_or(std::ptr::null(), |info| info.schema_json)
            .is_null());
        assert!(onda_delegate_info(program.0, -1)
            .as_ref()
            .map_or(std::ptr::null(), |info| info.schema_json)
            .is_null());
    }
}

#[test]
fn c_api_parameter_unit_query_distinguishes_absence_from_invalid_index() {
    unsafe {
        let program = compile_program(
            r#"
params {
  cutoff = 440.0 {20, 20000, log, "Hz"}
  gain = 1.0 {0, 2, curve = -4}
}
outs { out1 }
sample { out1 = cutoff * gain }
"#,
        );

        assert_eq!(
            CStr::from_ptr((*onda_param_info(program.0, 0)).unit).to_bytes(),
            b"Hz"
        );
        assert!((*onda_param_info(program.0, 1)).unit.is_null());
        assert!(onda_param_info(program.0, 2).is_null());
        assert_eq!(
            onda_param_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.has_curve),
            0
        );
        assert!(onda_param_info(program.0, 0)
            .as_ref()
            .map_or(f64::NAN, |info| info.curve)
            .is_nan());
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.has_curve),
            1
        );
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(f64::NAN, |info| info.curve),
            -4.0
        );
        assert_eq!(
            onda_param_info(program.0, 2)
                .as_ref()
                .map_or(-1, |info| info.has_curve),
            -1
        );
        assert!(onda_param_info(program.0, 2)
            .as_ref()
            .map_or(f64::NAN, |info| info.curve)
            .is_nan());
        assert_eq!(
            onda_param_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.scale),
            ONDA_PARAM_SCALE_LOG
        );
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.scale),
            ONDA_PARAM_SCALE_LINEAR
        );
        assert_eq!(
            onda_param_info(program.0, 2)
                .as_ref()
                .map_or(-1, |info| info.scale),
            -1
        );
    }
}

#[test]
fn c_api_numeric_param_array_queries_share_the_element_control_domain() {
    unsafe {
        let program = compile_program(
            r#"
params:
  ratios: f64[2] = [1.0, 2.0] {0.5, 4.0, log, "x"}
sample:
  out1 = f32(ratios[0] + 10.0 * ratios[1])
"#,
        );

        assert_eq!(
            onda_param_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.scale),
            ONDA_PARAM_SCALE_LOG
        );
        assert_eq!(
            onda_param_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.has_curve),
            0
        );
        assert!(onda_param_info(program.0, 0)
            .as_ref()
            .map_or(f64::NAN, |info| info.curve)
            .is_nan());
        assert_eq!(
            CStr::from_ptr((*onda_param_info(program.0, 0)).unit).to_bytes(),
            b"x"
        );
        assert_eq!(
            onda_param_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.has_step),
            0
        );
        assert_eq!(
            onda_param_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.step_count),
            0
        );
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.step_count),
            -1
        );
        assert_eq!(onda_param_normalized_to_plain(program.0, 0, 0.0), 0.5);
        assert_eq!(onda_param_plain_to_normalized(program.0, 0, 0.5), 0.0);

        let mut diag = empty_diag();
        let instance = initialized_instance(
            program.0,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut *diag,
        );
        assert!(!instance.is_null(), "{}", diag_message(&diag));
        let instance = InstanceHandle(instance);
        let mut output = [0.0_f32; 512];
        assert_eq!(
            onda_bind_output(
                instance.0,
                0,
                output.as_mut_ptr().cast(),
                std::mem::size_of_val(&output) as i32,
            ),
            0
        );
        assert_eq!(onda_set_param_element_plain_f64(instance.0, 0, 0, 9.0), 0);
        assert_eq!(onda_set_param_element_normalized(instance.0, 0, 1, 0.0), 0);
        assert_eq!(
            onda_process_checked_segment(
                instance.0,
                0,
                1,
                ONDA_PROCESS_FULL_BLOCK,
                std::ptr::null_mut()
            ),
            0
        );
        assert_eq!(output[0], 9.0);

        let raw = 3.0_f64.to_ne_bytes();
        assert_eq!(
            onda_set_param_element_by_index(
                instance.0,
                0,
                0,
                raw.as_ptr().cast(),
                raw.len() as i32,
            ),
            0
        );
        assert_eq!(
            onda_process_checked_segment(
                instance.0,
                0,
                1,
                ONDA_PROCESS_FULL_BLOCK,
                std::ptr::null_mut()
            ),
            0
        );
        assert_eq!(output[0], 8.0);

        assert_eq!(
            onda_set_param_element_plain_f64(instance.0, 0, -1, 1.0),
            ONDA_API_ERROR_INVALID_ARGUMENT
        );
        assert_eq!(
            onda_set_param_element_normalized(instance.0, 0, 2, 1.0),
            ONDA_API_ERROR_INVALID_SHAPE
        );
    }
}

#[test]
fn c_api_parameter_conversions_support_boolean_thresholds() {
    unsafe {
        let program = compile_program(
            r#"
params {
  gate: bool = false
  flags: bool[1] = [false]
}
outs { out1 }
sample { out1 = 0.0 }
"#,
        );

        assert_eq!(onda_param_normalized_to_plain(program.0, 0, 0.49), 0.0);
        assert_eq!(onda_param_normalized_to_plain(program.0, 0, 0.5), 1.0);
        assert_eq!(onda_param_plain_to_normalized(program.0, 0, -1.0), 0.0);
        assert_eq!(onda_param_plain_to_normalized(program.0, 0, 0.5), 1.0);
        assert_eq!(onda_param_normalized_to_plain(program.0, 0, f64::NAN), 0.0);

        assert_eq!(onda_param_normalized_to_plain(program.0, 1, 0.49), 0.0);
        assert_eq!(onda_param_normalized_to_plain(program.0, 1, 0.5), 1.0);
        assert_eq!(onda_param_plain_to_normalized(program.0, 1, -1.0), 0.0);
        assert_eq!(onda_param_plain_to_normalized(program.0, 1, 0.5), 1.0);
        assert!(onda_param_plain_to_normalized(program.0, 2, 1.0).is_nan());
        assert_eq!(
            onda_param_info(program.0, 0)
                .as_ref()
                .map_or(-1, |info| info.scale),
            ONDA_PARAM_SCALE_NONE
        );
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.scale),
            ONDA_PARAM_SCALE_NONE
        );
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.has_curve),
            0
        );
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.has_step),
            0
        );
        assert_eq!(
            onda_param_info(program.0, 1)
                .as_ref()
                .map_or(-1, |info| info.step_count),
            0
        );
    }
}
