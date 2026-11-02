use super::*;

#[test]
fn enum_parameter_metadata_requires_its_derived_domain() {
    let mut program = empty_program();
    program.interface.params.push(Param {
        name: "mode".into(),
        ty: TypeId::new(0),
        default: ConstantValue::Scalar(ScalarValue::I32(0)),
        range: Some(crate::ValueRange {
            min: ScalarValue::I32(0),
            max: ScalarValue::I32(1),
        }),
        control: crate::ParamControl {
            step: Some(ScalarValue::I32(1)),
            step_count: Some(1),
            ..Default::default()
        },
        enumeration: Some(onda_processor_abi::EnumMetadata {
            name: "Mode".into(),
            members: ["A", "B"]
                .into_iter()
                .map(|name| onda_processor_abi::EnumMemberMetadata {
                    name: name.into(),
                    label: name.into(),
                })
                .collect(),
        }),
    });
    crate::validate(&program).unwrap();
    for control in [
        crate::ParamControl {
            step: Some(ScalarValue::I32(2)),
            ..program.interface.params[0].control.clone()
        },
        crate::ParamControl {
            curve: Some(2.0),
            ..program.interface.params[0].control.clone()
        },
        crate::ParamControl {
            unit: Some("Hz".into()),
            ..program.interface.params[0].control.clone()
        },
    ] {
        let mut invalid = program.clone();
        invalid.interface.params[0].control = control;
        let errors = crate::validate(&invalid).unwrap_err();
        assert!(
            errors
                .iter()
                .any(|error| error.message.contains("enum control domain")),
            "{errors:?}"
        );
    }
    program.interface.params[0].range.as_mut().unwrap().max = ScalarValue::I32(2);
    assert!(crate::validate(&program)
        .unwrap_err()
        .iter()
        .any(|error| error.message.contains("enum storage and range")));
}
