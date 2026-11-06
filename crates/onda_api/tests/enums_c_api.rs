use onda::*;
use std::ffi::{CStr, CString};

#[test]
fn enum_json_queries_expose_choices_without_changing_scalar_storage() {
    unsafe {
        let source = CString::new(
            r#"
enum Mode:
  LowPass
  HighPass {label = "High-pass"}
params:
  mode = Mode::HighPass
  gain = 0.5
outs:
  choice: i32
kouts:
  selected: Mode
init:
  current: Mode
event set_mode(value: Mode = Mode::LowPass):
  current = value
block:
  selected = mode
  sample:
    choice = i32(current)
"#,
        )
        .unwrap();
        let options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48000.0,
            block_size: 4,
            const_inputs: std::ptr::null(),
            const_input_count: 0,
        };
        let mut diagnostic: onda_diag_t = std::mem::zeroed();
        let raw = onda_compile(source.as_ptr(), &options, &mut diagnostic);
        assert!(
            !raw.is_null(),
            "{}",
            if diagnostic.message.is_null() {
                "compile failed"
            } else {
                CStr::from_ptr(diagnostic.message).to_str().unwrap()
            }
        );
        struct Program(*mut onda_program);
        impl Drop for Program {
            fn drop(&mut self) {
                unsafe {
                    onda_program_destroy(self.0);
                }
            }
        }
        let program = Program(raw);
        onda_diag_dispose(&mut diagnostic);
        for query in [onda_param_info, onda_control_output_info] {
            let info = &*query(program.0, 0);
            assert_eq!(
                info.enumeration_json,
                (*query(program.0, 0)).enumeration_json
            );
            let metadata: serde_json::Value =
                serde_json::from_str(CStr::from_ptr(info.enumeration_json).to_str().unwrap())
                    .unwrap();
            assert_eq!(metadata["name"], "Mode");
            assert_eq!(metadata["members"][0]["label"], "Low Pass");
            assert_eq!(metadata["members"][1]["label"], "High-pass");
        }
        assert!((*onda_param_info(program.0, 1)).enumeration_json.is_null());
        assert!(onda_param_info(program.0, -1).is_null());
        let schema: serde_json::Value = serde_json::from_str(
            CStr::from_ptr(
                onda_event_info(program.0, 0)
                    .as_ref()
                    .map_or(std::ptr::null(), |info| info.schema_json),
            )
            .to_str()
            .unwrap(),
        )
        .unwrap();
        assert_eq!(schema["params"][0]["ty"]["encoding"], "i32");
        assert_eq!(
            schema["params"][0]["ty"]["enumeration"]["members"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}

#[test]
fn enum_compile_constants_reuse_i32_inputs_and_export_choice_metadata() {
    unsafe {
        let source = CString::new(
            r#"
enum Mode:
  LowPass
  HighPass
  BandPass
const Default: Mode = Mode::HighPass
config const Selected: Mode = Default
config const Choices: Mode[2] = [Mode::LowPass, Default]
params:
  selected = Selected
sample:
  out1 = i32(selected) + i32(Choices[0]) + i32(Choices[1])
"#,
        )
        .unwrap();
        let selected = 99_i32;
        let choices = [-99_i32, 99];
        let selected_name = CString::new("Selected").unwrap();
        let choices_name = CString::new("Choices").unwrap();
        let inputs = [
            onda_compile_const_input_t {
                name_utf8: selected_name.as_ptr(),
                element_type: ONDA_PRIMITIVE_I32,
                is_array: 0,
                element_count: 1,
                values: (&selected as *const i32).cast(),
            },
            onda_compile_const_input_t {
                name_utf8: choices_name.as_ptr(),
                element_type: ONDA_PRIMITIVE_I32,
                is_array: 1,
                element_count: 2,
                values: choices.as_ptr().cast(),
            },
        ];
        let mut options = onda_compile_options_t {
            fast_math: 0,
            sample_rate: 48000.0,
            block_size: 4,
            const_inputs: inputs.as_ptr(),
            const_input_count: inputs.len(),
        };
        let mut diagnostic: onda_diag_t = std::mem::zeroed();
        let constants = onda_inspect_compile_constants(source.as_ptr(), &options, &mut diagnostic);
        assert!(
            !constants.is_null(),
            "{}",
            if diagnostic.message.is_null() {
                "inspection failed"
            } else {
                CStr::from_ptr(diagnostic.message).to_str().unwrap()
            }
        );
        assert_eq!(onda_compile_constants_count(constants), 2);
        for index in 0..2 {
            let descriptor = &*onda_compile_constants_at(constants, index);
            assert_eq!(descriptor.input.element_type, ONDA_PRIMITIVE_I32);
            let metadata: serde_json::Value = serde_json::from_str(
                CStr::from_ptr(descriptor.enumeration_json_utf8)
                    .to_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(metadata["name"], "Mode");
            assert_eq!(metadata["members"].as_array().unwrap().len(), 3);
        }
        let selection = &*onda_compile_constants_at(constants, 0);
        assert_eq!(*selection.input.values.cast::<i32>(), 2);
        let values = &*onda_compile_constants_at(constants, 1);
        assert_eq!(
            std::slice::from_raw_parts(values.input.values.cast::<i32>(), 2),
            [0, 2]
        );
        options.const_inputs = onda_compile_constants_inputs(constants);
        let program = onda_compile(source.as_ptr(), &options, &mut diagnostic);
        assert!(!program.is_null());
        assert_eq!(
            onda_param_info(program, 0)
                .as_ref()
                .map_or(f64::NAN, |info| info.default_f64),
            2.0
        );
        onda_program_destroy(program);
        onda_compile_constants_destroy(constants);
        onda_diag_dispose(&mut diagnostic);
    }
}
